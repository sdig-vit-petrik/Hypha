//! Cycle registration: creation, binding, commit and destruction.

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::source::HyphaInputSource;
pub(crate) use super::state::{Cycle, CycleOutput, CycleStep};
use super::state::{CycleId, CycleSnapshots, CycleStepHandler};
use super::types::{
    HyphaCycleBindingOptions, HyphaCycleBindingView, HyphaCycleInputMode, HyphaCycleOutputFailure,
};

use crate::introspection::{IntrospectionRow, Row, input_mode, output_failure};
use crate::publisher::HyphaPublisher;
use crate::status::Error;
use crate::waitset::HyphaWaitSet;

/// Creates an uncommitted cycle `name`, owned by `set`, with rate `period_nanos`
/// (0: a triggered cycle), and returns its id. `adopt` runs only once nothing can refuse
/// the create, so a refused create leaves the caller's context to the caller.
///
/// # Errors
/// `InvalidArgument` for an empty name, a name another cycle of `set` holds, a period
/// above `i64::MAX` or no step callback; `InvalidState` while `set` runs.
pub fn cycle_create(
    set: &HyphaWaitSet,
    name: &str,
    period_nanos: u64,
    adopt: Option<impl FnOnce(CycleId, &Arc<CycleSnapshots>) -> Box<dyn CycleStepHandler>>,
) -> Result<CycleId, Error> {
    crate::last_error::clear();
    let period_nanos = i64::try_from(period_nanos).map_err(|_| Error::InvalidArgument)?;
    let Some(adopt) = adopt.filter(|_| !name.is_empty()) else {
        return Err(Error::InvalidArgument);
    };
    let mut cycles = set.cycles_mut()?;
    if cycles.iter().any(|cycle| cycle.name() == name) {
        return Err(Error::InvalidArgument);
    }
    let id = set.take_cycle_id()?;
    let node = set.node();
    let snapshots =
        Arc::new(CycleSnapshots { name: name.to_owned(), period_nanos, ..Default::default() });
    // Nothing refuses the create from here: the caller's context changes owner.
    let step = CycleStep { callback: Some(adopt(id, &snapshots)), ..CycleStep::default() };
    let cycle = Cycle {
        id,
        snapshots,
        introspection: IntrospectionRow::new(
            &node.introspection,
            Row::Cycle(hypha_core::introspection::CycleRow {
                name: name.to_owned(),
                period_nanos: u64::try_from(period_nanos).ok().filter(|period| *period > 0),
                inputs: 0,
                outputs: 0,
                triggers: Vec::new(),
            }),
        ),
        step: RefCell::new(step),
    };
    cycles.push(cycle);
    Ok(id)
}

/// The capture options of one binding.
fn capture_options(
    selected: &HyphaCycleBindingOptions,
) -> Result<hypha_cycle::BindingOptions, Error> {
    let (mode, share) = match HyphaCycleInputMode::try_from(selected.mode) {
        Ok(HyphaCycleInputMode::Latest) => {
            (hypha_cycle::CaptureMode::Latest, selected.max_samples_per_iteration)
        }
        Ok(HyphaCycleInputMode::Queued) => {
            (hypha_cycle::CaptureMode::Queued, selected.max_samples_per_iteration)
        }
        // Newest only: a share of one; the older pending samples are work-limit drops. A
        // trigger takes one sample per step whatever its share; the schedule passes the
        // older ones.
        Ok(HyphaCycleInputMode::Newest) if selected.max_samples_per_iteration > 0 => {
            (hypha_cycle::CaptureMode::Queued, 1)
        }
        Ok(HyphaCycleInputMode::Newest) | Err(()) => return Err(Error::InvalidArgument),
    };
    Ok(hypha_cycle::BindingOptions {
        mode,
        // The silence limit comes from the port's supervision, never from here.
        silence_nanos: 0,
        max_samples_per_iteration: share,
        overflow_policy: hypha_cycle::QueueOverflowPolicy::DropOldest,
    })
}

/// Binds `source` to the uncommitted cycle `cycle` of `set` with `options`; returns the
/// binding index.
///
/// # Errors
/// `InvalidArgument` for a cycle `set` does not hold, a source of another `WaitSet` or
/// an unknown mode; `InvalidState` for a committed cycle or a running `WaitSet`; a
/// refusal of the cycle runtime.
pub fn cycle_bind_input(
    set: &HyphaWaitSet,
    cycle: CycleId,
    input: &HyphaInputSource,
    selected: &HyphaCycleBindingOptions,
) -> Result<u32, Error> {
    let cycle = set.cycle(cycle)?;
    bind_input(set, &cycle, input, selected)
}

pub(crate) fn bind_input(
    set: &HyphaWaitSet,
    cycle: &Cycle,
    input: &HyphaInputSource,
    selected: &HyphaCycleBindingOptions,
) -> Result<u32, Error> {
    let selected = *selected;
    let mut registration = cycle.configurable(set)?;
    if !Cycle::owns_source(set, &input.inner) {
        return Err(Error::InvalidArgument);
    }
    let options = capture_options(&selected)?;
    if options.mode == hypha_cycle::CaptureMode::Queued {
        input.inner.queued_bindings.fetch_add(1, Ordering::AcqRel);
        input.inner.reserve_arrivals(selected.max_samples_per_iteration);
    }
    let binding = match input.inner.history.as_ref() {
        Some(history) => registration.runtime.add_binding(Arc::clone(history), options),
        None => registration.runtime.add_disabled_binding(options),
    };
    let index = binding.map_err(Error::from_code)?;
    registration.inputs.push(Arc::clone(&input.inner));
    crate::locked(&cycle.snapshots.bindings).push(HyphaCycleBindingView::default());
    registration.input_options.push(selected);
    registration.port_names.push(String::new());
    let role = if selected.optional == 0 {
        hypha_core::introspection::InputRole::Required
    } else {
        hypha_core::introspection::InputRole::Optional
    };
    let subscriber_row = input.inner.subscriber.as_deref().map(|s| s.introspection.id());
    registration.input_rows.push(IntrospectionRow::new(
        &set.node().introspection,
        Row::InputBinding(hypha_core::introspection::InputBindingRow {
            cycle: cycle.name().to_owned(),
            index,
            topic: input.inner.topic.clone(),
            subscriber: subscriber_row,
            role,
            mode: input_mode(&selected),
            supervision: None,
        }),
    ));
    cycle.introspection.set_cycle_bindings(registration.inputs.len(), registration.outputs.len());
    Ok(index)
}

/// Binds the publisher `owned` to the uncommitted cycle `cycle` of `set`; returns the
/// binding index. The cycle owns the publisher from then on. A refusal gives the
/// publisher back with the error.
///
/// # Errors
/// `InvalidArgument` for a cycle `set` does not hold, a zero message count, an unknown
/// policy or a publisher of another node; `ResourceLimit` for a publisher with fewer loans than the count;
/// `InvalidState` for a publisher another cycle owns, a committed cycle or a running
/// `WaitSet`; a refusal of the cycle runtime.
pub fn cycle_bind_output(
    set: &HyphaWaitSet,
    cycle: CycleId,
    owned: Box<HyphaPublisher>,
    policy: u32,
    max_messages_per_step: u32,
) -> Result<u32, (Error, Box<HyphaPublisher>)> {
    let cycle = match set.cycle(cycle) {
        Ok(cycle) => cycle,
        Err(error) => return Err((error, owned)),
    };
    let cycle: &Cycle = &cycle;
    let slot = match reserve_output(set, cycle, &owned, policy, max_messages_per_step) {
        Ok(slot) => slot,
        Err(error) => return Err((error, owned)),
    };
    let OutputSlot { mut registration, index, policy } = slot;
    owned.introspection.bind_output(hypha_core::introspection::CycleOutputRow {
        cycle: cycle.name().to_owned(),
        failure: output_failure(policy),
    });
    registration.outputs.push(CycleOutput { publisher: owned });
    cycle.introspection.set_cycle_bindings(registration.inputs.len(), registration.outputs.len());
    Ok(index)
}

struct OutputSlot<'cycle> {
    registration: std::cell::RefMut<'cycle, CycleStep>,
    index: u32,
    policy: HyphaCycleOutputFailure,
}

fn reserve_output<'cycle>(
    set: &HyphaWaitSet,
    cycle: &'cycle Cycle,
    endpoint: &HyphaPublisher,
    policy: u32,
    max_messages_per_step: u32,
) -> Result<OutputSlot<'cycle>, Error> {
    let mut registration = cycle.configurable(set)?;

    if max_messages_per_step == 0 {
        return Err(Error::InvalidArgument);
    }
    let policy = HyphaCycleOutputFailure::try_from(policy).map_err(|()| Error::InvalidArgument)?;
    if endpoint.owner_node != set.node_address() {
        return Err(Error::InvalidArgument);
    }
    // A step may reserve up to `max_messages_per_step` loans at once.
    let holds_enough_loans = usize::try_from(max_messages_per_step)
        .is_ok_and(|needed| endpoint.loaned_samples >= needed);
    if !holds_enough_loans {
        return Err(Error::ResourceLimit);
    }
    if endpoint.cycle_owned.swap(true, Ordering::AcqRel) {
        return Err(Error::InvalidState);
    }
    let core_policy = match policy {
        HyphaCycleOutputFailure::FailRun => hypha_cycle::OutputFailurePolicy::FailRun,
        HyphaCycleOutputFailure::DropPoolExhausted => {
            hypha_cycle::OutputFailurePolicy::DropPoolExhausted
        }
    };
    let added = registration.runtime.add_output(
        core_policy,
        endpoint.max_payload_bytes,
        max_messages_per_step,
    );
    let index = added.map_err(|status| {
        endpoint.cycle_owned.store(false, Ordering::Release);
        Error::from_code(status)
    })?;
    Ok(OutputSlot { registration, index, policy })
}

/// Adds the queued input `binding_index` to the triggers of the uncommitted, period-less
/// cycle `cycle` of `set`: the cycle runs one step per sample of any trigger. A repeat
/// adds nothing.
///
/// # Errors
/// `InvalidArgument` for a cycle `set` does not hold, an unknown, latest or unwired
/// binding, or one whose history another trigger reads; `ResourceLimit` past the most
/// triggers a cycle has; `InvalidState` for a committed cycle or a running `WaitSet`.
pub fn cycle_set_trigger(
    set: &HyphaWaitSet,
    cycle: CycleId,
    binding_index: u32,
) -> Result<(), Error> {
    let cycle = set.cycle(cycle)?;
    let mut registration = cycle.configurable(set)?;
    let index = usize::try_from(binding_index).map_err(|_| Error::InvalidArgument)?;
    registration.runtime.set_trigger(index).map_err(Error::from_code)
}

/// Captures the latest input `binding_index` of the uncommitted cycle `cycle` of `set`
/// on the host clock.
///
/// # Errors
/// `InvalidArgument` for a cycle `set` does not hold, an unknown or queued binding;
/// `InvalidState` for a committed cycle or a running `WaitSet`.
pub fn cycle_set_host_clock(
    set: &HyphaWaitSet,
    cycle: CycleId,
    binding_index: u32,
) -> Result<(), Error> {
    let cycle = set.cycle(cycle)?;
    let mut registration = cycle.configurable(set)?;
    let index = usize::try_from(binding_index).map_err(|_| Error::InvalidArgument)?;
    registration.runtime.set_host_clock(index).map_err(Error::from_code)
}

/// Marks the cycle `cycle` of `set` class-based, so its `[cycle]` and input tables
/// override code defaults.
///
/// # Errors
/// `InvalidArgument` for a cycle `set` does not hold; `InvalidState` for a committed
/// cycle or a running `WaitSet`.
pub fn cycle_set_class_based(
    set: &HyphaWaitSet,
    cycle: CycleId,
    class_based: bool,
) -> Result<(), Error> {
    set.cycle(cycle)?.configurable(set)?.class_based = class_based;
    Ok(())
}

/// Commits the cycle `cycle` of `set`: attaches its timer, triggers or clock driver. A
/// refusal with a reason records it for `hypha_last_error`.
///
/// # Errors
/// `InvalidState` for a cycle on a CLI tool node, a second clock owner, a committed
/// cycle or a running `WaitSet`; `InvalidArgument` for a cycle `set` does not hold, a
/// refused `[cycle]` key or a rate and trigger combination; an attach error.
pub fn cycle_commit(set: &HyphaWaitSet, cycle: CycleId) -> Result<(), Error> {
    crate::last_error::clear();
    let cycle = set.cycle(cycle)?;
    let cycle: &Cycle = &cycle;
    let mut registration = cycle.configurable(set)?;
    let node = set.node();
    // A cycle reports its replay progress under the node name, which every
    // process of a CLI tool holds.
    if hypha_core::port::is_cli_node_name(&node.name) {
        return Err(crate::last_error::refuse(
            Error::InvalidState,
            &format!(
                "cycle '{}' is refused: the CLI tool node '{}' runs no cycle",
                cycle.name(),
                node.name
            ),
        ));
    }
    if registration.class_based {
        apply_cycle_config(&mut registration, node)?;
    }
    let silences = trigger_silences(cycle, &registration)?;
    let driver = registration.driver.is_some();
    check_clock_owner(node, cycle, driver)?;
    // A downstream can see a replay cycle wait for its first clock, and the cycle cuts
    // its inputs by stamp.
    if node.on_sim_time() {
        crate::node_progress::ensure_publisher(node).map_err(Error::from_status)?;
        for source in &registration.inputs {
            source.replayed.store(true, Ordering::Release);
        }
    }
    let step: &mut CycleStep = &mut registration;
    step.supervision.resize_with(step.inputs.len(), RefCell::default);
    // The step must not be borrowed across the attach.
    let attach = match silences {
        None if driver => Attach::Driver(registration.driver.clone().ok_or(Error::InvalidState)?),
        Some(silences) => Attach::Triggered {
            silences,
            newest: newest_triggers(step)?,
            listeners: trigger_listeners(step)?,
        },
        None => Attach::Timer,
    };
    drop(registration);
    let attached = match attach {
        Attach::Driver(setup) => {
            crate::waitset::attach_driver_cycle(set, cycle, &setup).map(|id| (id, Vec::new()))
        }
        Attach::Triggered { silences, newest, listeners } => {
            crate::waitset::attach_triggered_cycle(set, cycle, silences, newest, listeners)
        }
        Attach::Timer => crate::waitset::attach_cycle_timer(set, cycle).map(|id| (id, Vec::new())),
    };
    let (attachment, trigger_attachments) = attached?;
    let mut registration = cycle.step_mut()?;
    registration.attachment = attachment;
    registration.trigger_attachments = trigger_attachments;
    node.committed_cycles.fetch_add(1, Ordering::AcqRel);
    node.drives_clock.fetch_or(driver, Ordering::AcqRel);
    let triggers = registration.runtime.triggers().iter();
    let topics = triggers.filter_map(|&index| registration.inputs.get(index));
    cycle.introspection.set_cycle_triggers(topics.map(|source| source.topic.clone()).collect());
    cycle.snapshots.committed.store(true, Ordering::Release);
    Ok(())
}

enum Attach {
    Timer,
    Triggered {
        silences: Vec<i64>,
        newest: hypha_cycle::TriggerPorts,
        listeners: Vec<Vec<hypha_iceoryx2::Iox2Listener>>,
    },
    Driver(super::DriverSetup),
}

/// The listeners of each trigger of `registration`, in port order: one per stream it
/// reads. A wildcard subscriber has none: the periodic wake serves it.
fn trigger_listeners(
    registration: &CycleStep,
) -> Result<Vec<Vec<hypha_iceoryx2::Iox2Listener>>, Error> {
    registration
        .runtime
        .triggers()
        .iter()
        .map(|&index| {
            let source = registration.inputs.get(index);
            let subscriber = source.and_then(|source| source.subscriber.as_deref());
            subscriber.ok_or(Error::InvalidState)?.port.open_listeners().map_err(Error::from)
        })
        .collect()
}

/// Refuse a commit that would put two clocks in play: a driver in a process already on
/// simulated time or on a node with other cycles, and any cycle on a node with a driver.
fn check_clock_owner(
    node: &crate::node::HyphaNode,
    registration: &Cycle,
    driver: bool,
) -> Result<(), Error> {
    let reason = if driver && node.on_sim_time() {
        "the process already runs on simulated time"
    } else if driver && node.committed_cycles.load(Ordering::Acquire) > 0 {
        "its node already runs another cycle"
    } else if node.drives_clock.load(Ordering::Acquire) {
        "its node drives /clock and runs no other cycle"
    } else {
        return Ok(());
    };
    Err(crate::last_error::refuse(
        Error::InvalidState,
        &format!("cycle '{}' is refused: {reason}", registration.name()),
    ))
}

/// The silence limit of each trigger in nanoseconds, in port order, for a triggered
/// cycle; `None` for a rate cycle. A cycle with both a rate and a trigger, with neither,
/// or with a trigger that has no positive silence limit is refused, and the reason names
/// that trigger.
fn trigger_silences(cycle: &Cycle, registration: &CycleStep) -> Result<Option<Vec<i64>>, Error> {
    let name = cycle.name();
    let refuse = |reason: &str| {
        crate::last_error::refuse(
            Error::InvalidArgument,
            &format!("cycle '{name}' is refused: {reason}"),
        )
    };
    let triggers = registration.runtime.triggers();
    if triggers.is_empty() {
        if cycle.period_nanos() == 0 {
            return Err(refuse("it has neither a rate nor a trigger"));
        }
        return Ok(None);
    }
    if cycle.period_nanos() != 0 {
        return Err(refuse("a rate and a trigger exclude each other"));
    }
    let silence_of = |index: usize| {
        let capture = registration.runtime.binding_capture(index);
        capture
            .and_then(hypha_cycle::BindingCapture::supervision)
            .map(|supervision| supervision.limits().silence_nanos)
            .and_then(|nanos| i64::try_from(nanos).ok())
            .filter(|nanos| *nanos > 0)
            .ok_or_else(|| {
                let input = input_name(registration, index);
                refuse(&format!(
                    "its trigger input {input} needs a positive silence limit (silence_ms)"
                ))
            })
    };
    triggers.iter().map(|&index| silence_of(index)).collect::<Result<_, _>>().map(Some)
}

/// The trigger ports of `registration` bound newest only, by port.
fn newest_triggers(registration: &CycleStep) -> Result<hypha_cycle::TriggerPorts, Error> {
    let newest = |index: &usize| {
        registration
            .input_options
            .get(*index)
            .is_some_and(|options| options.mode == HyphaCycleInputMode::Newest as u32)
    };
    let triggers = registration.runtime.triggers().iter().enumerate();
    let ports = triggers.filter(|(_, index)| newest(index)).map(|(port, _)| port);
    hypha_cycle::TriggerPorts::of(ports).ok_or(Error::InvalidState)
}

/// Input `index` of `registration` in a refusal: `'<port>'`, or `(<topic>)` without a table.
fn input_name(registration: &CycleStep, index: usize) -> String {
    if let Some(port) = registration.port_names.get(index).filter(|port| !port.is_empty()) {
        return format!("'{port}'");
    }
    let topic = registration.inputs.get(index).map_or("", |source| source.topic.as_str());
    format!("({topic})")
}

/// Apply the `[cycle]` table of a class-based cycle. `rate_hz` is only checked here: the
/// SDK passes the period to `hypha_cycle_create`.
fn apply_cycle_config(
    registration: &mut CycleStep,
    node: &crate::node::HyphaNode,
) -> Result<(), Error> {
    use hypha_core::introspection::ConfigSource;
    use hypha_core::parameters::ParamValue;

    for key in node.parameters.names() {
        let Some(suffix) = key.strip_prefix("cycle.") else {
            continue;
        };
        let Some(value) = node.parameters.value(key) else {
            return Err(Error::InvalidArgument);
        };
        if suffix == "rate_hz" {
            let positive = match value {
                ParamValue::Int(rate) => *rate > 0,
                ParamValue::Float(rate) => rate.is_finite() && *rate > 0.0,
                _ => false,
            };
            if !positive {
                return Err(refuse_cycle_key(key, value, "must be a positive number"));
            }
            continue;
        }
        if suffix != "arrival_budget" {
            return Err(refuse_cycle_key(
                key,
                value,
                "is not a cycle key; the keys are rate_hz and arrival_budget",
            ));
        }
        let ParamValue::Int(budget) = value else {
            return Err(refuse_cycle_key(key, value, "must be a whole number"));
        };
        let positive_budget = usize::try_from(*budget).ok().filter(|budget| *budget > 0);
        let Some(budget) = positive_budget else {
            return Err(refuse_cycle_key(key, value, "must be a positive whole number"));
        };
        registration.runtime.set_arrival_budget(Some(budget)).map_err(Error::from_code)?;
        crate::param_topic::record_key_row(
            node,
            key.to_owned(),
            value.to_string(),
            None,
            ConfigSource::Parameter,
        );
    }
    Ok(())
}

/// Log and record the refused `[cycle]` key `key`, and return the status.
fn refuse_cycle_key(key: &str, value: &hypha_core::parameters::ParamValue, reason: &str) -> Error {
    crate::last_error::refuse(
        Error::InvalidArgument,
        &format!("the key {key} has value {value}, which {reason}"),
    )
}

/// Destroys the cycle `cycle` of `set`. The id names no cycle afterwards.
///
/// # Errors
/// `InvalidState` while the `WaitSet` runs; `InvalidArgument` for a cycle the
/// `WaitSet` does not hold; a detach error.
pub fn cycle_destroy(set: &HyphaWaitSet, cycle: CycleId) -> Result<(), Error> {
    if set.is_running() {
        return Err(Error::InvalidState);
    }
    // Every refusal comes before the first change, so no cycle is left without its timer.
    drop(set.cycles_mut()?);
    let (attachments, committed_driver) = {
        let held = set.cycle(cycle)?;
        let step = held.step_ref()?;
        let listeners = step.trigger_attachments.iter().copied();
        let attachments: Vec<_> = std::iter::once(step.attachment).chain(listeners).collect();
        (attachments, step.driver.is_some())
    };
    // Each detached registration leaves the record, so a refused destroy can be repeated.
    for attachment in attachments.into_iter().filter(|id| *id != 0) {
        crate::waitset::waitset_detach(set, attachment)?;
        let held = set.cycle(cycle)?;
        if let Ok(mut step) = held.step_mut() {
            if step.attachment == attachment {
                step.attachment = 0;
            } else {
                step.trigger_attachments.retain(|id| *id != attachment);
            }
        }
    }
    if set.cycle(cycle)?.is_committed() {
        let node = set.node();
        node.committed_cycles.fetch_sub(1, Ordering::AcqRel);
        if committed_driver {
            node.drives_clock.store(false, Ordering::Release);
        }
    }
    // The cycle leaves the list before its drop releases its context.
    let removed = {
        let mut cycles = set.cycles_mut()?;
        let index =
            cycles.iter().position(|held| held.id == cycle).ok_or(Error::InvalidArgument)?;
        cycles.remove(index)
    };
    drop(removed);
    Ok(())
}
