//! The cycle timer callback: capture, step, validate and dispatch.

use std::cell::RefCell;
use std::sync::Arc;
use std::time::Instant;

use hypha_cycle::{
    BindingCapture, CycleHealth, CycleRuntime, CycleTime, EmissionBudget, OutputError,
    StagedMessage, counts_losses, is_short_history,
};

use super::image::{EmissionView, ImageView, SupervisionCopy, binding_report};
use super::registration::{Cycle, CycleOutput, CycleStep};
use super::source::{CycleInputSource, DrainClock};
use super::state::CycleStepHandler;
use super::types::{
    HyphaCycleBindingHealth, HyphaCycleBindingView, HyphaCycleHealth, HyphaCycleHeldReport,
    HyphaCycleTime, HyphaSupervisionReport,
};
use crate::loan::{HyphaLoan, take_sequence};
use crate::status::Error;
use crate::status::HyphaStatus;
use crate::waitset::{
    HYPHA_ACTION_CONTINUE, HYPHA_ACTION_FAIL, HYPHA_ACTION_STOP, HyphaCallbackResult,
};

/// What the step returned.
#[derive(Default)]
struct StepOutcome {
    answer: Option<HyphaCallbackResult>,
    /// Host monotonic duration of the SDK step call, when it was called.
    duration_nanos: Option<u64>,
}

struct StepState<'a> {
    step: &'a mut dyn CycleStepHandler,
    inputs: &'a [Arc<CycleInputSource>],
    input_options: &'a [super::HyphaCycleBindingOptions],
    supervision: &'a mut [RefCell<SupervisionCopy>],
    transforms: &'a mut [super::transforms::TfInput],
    cycle_name: &'a str,
    /// The cycle's outputs. A reserve takes its loan from the output's publisher.
    outputs: &'a [CycleOutput],
    outcome: &'a mut StepOutcome,
    is_replay: bool,
    /// The `WaitSet` of the cycle: a failure the application requested holds the step's outputs.
    waitset: &'a crate::waitset::HyphaWaitSet,
    /// The loans the step holds, by token (`emission_lend`); empty between steps.
    lent: &'a mut Vec<Option<super::image::LentMessage>>,
}

/// Drain each physical source of `cycle` once into its history for `clock`, for the
/// dispatcher: before a step it cuts itself, and on every turn under simulated time.
/// `InvalidState` inside a step.
pub(crate) fn drain_inputs(cycle: &Cycle, clock: DrainClock) -> Result<(), Error> {
    drain_sources(&cycle.step_ref()?.inputs, clock)
}

/// Check the stream of each input of `cycle` that has a subscriber, at host time
/// `now_nanos`, from the housekeeping of a turn: never inside its step. A cycle whose step
/// runs is skipped. On simulated time (`replay`), warn too of an input with a second
/// publisher, at most once per second per input.
pub(crate) fn check_input_publishers(cycle: &Cycle, now_nanos: u64, replay: bool) {
    let Ok(step) = cycle.step_ref() else { return };
    for source in &step.inputs {
        let Some(subscriber) = source.subscriber.as_deref() else { continue };
        subscriber.check_no_publisher_warning(now_nanos);
        if replay {
            warn_of_several_publishers(source, subscriber);
        }
    }
}

/// Warn when the replay input `source` has more than one live publisher: the order of
/// their samples depends on turn timing (`replay-flow-control.md` section 6.7).
fn warn_of_several_publishers(source: &CycleInputSource, subscriber: &crate::HyphaSubscriber) {
    let mut fidelity = crate::locked(&source.fidelity);
    if !fidelity.publishers_due(Instant::now()) {
        return;
    }
    match crate::subscriber::subscriber_publisher_count(subscriber) {
        Ok(count) => {
            if let Some(text) = fidelity.record_publishers(&source.topic, count) {
                log::warn!(target: "hypha", "hypha: {text}");
            }
        }
        Err(error) => {
            log::warn!(target: "hypha", "hypha: cannot count the publishers of {}: {error:?}", source.topic);
        }
    }
}

/// Drain each physical source once into its history.
///
/// Several bindings and cycles can share a source, whose history is their common snapshot.
fn drain_sources(inputs: &[Arc<CycleInputSource>], clock: DrainClock) -> Result<(), Error> {
    for (index, source) in inputs.iter().enumerate() {
        let already_drained = inputs
            .get(..index)
            .is_some_and(|previous| previous.iter().any(|item| Arc::ptr_eq(item, source)));
        if !already_drained {
            source.drain(clock)?;
        }
    }
    Ok(())
}

/// One timer firing of a committed cycle.
///
/// A replay step finds its inputs drained by the dispatcher and its data-time cut in
/// `input_cut`. `set` is the `WaitSet` that owns `cycle`.
pub(crate) fn cycle_timer_tick(
    cycle: &Cycle,
    set: &crate::waitset::HyphaWaitSet,
    missed_periods: u64,
) -> HyphaCallbackResult {
    // A tick inside a tick, or inside a configuration call, is an invalid state.
    let Ok(mut state) = cycle.step_mut() else {
        return HyphaCallbackResult::fail(HyphaStatus::InvalidState as i32);
    };
    let state: &mut CycleStep = &mut state;
    let Some(step) = state.callback.as_deref_mut() else {
        return HyphaCallbackResult::fail(HyphaStatus::InvalidState as i32);
    };
    let replay_cut = state.input_cut.take();
    // A triggered step finds its inputs drained by the dispatcher.
    let trigger_step = state.trigger_step.take();
    if replay_cut.is_none()
        && trigger_step.is_none()
        && let Err(status) = drain_sources(&state.inputs, DrainClock::Host)
    {
        return HyphaCallbackResult::fail(status as i32);
    }
    let is_replay = replay_cut.is_some();
    let Ok(replay_cut_nanos) = replay_cut.map(u64::try_from).transpose() else {
        return HyphaCallbackResult::fail(HyphaStatus::InvalidState as i32);
    };
    let outputs: &[CycleOutput] = &state.outputs;
    let runtime = &mut state.runtime;
    // One reading of the clock for the whole step. A replay step reads its deadline,
    // because the clock has not moved while the catch-up steps run.
    let stamp = replay_cut.or_else(|| message_stamp(hypha_time::is_sim_time()));
    // A wall clock before the Unix epoch gives no stamp, and the step's stamp is invalid.
    let step_nanos = stamp.unwrap_or(0);
    // Every line about this step carries the step's time.
    let _step_time = hypha_time::StepTimeGuard::hold(step_nanos);
    let timing = CycleTime {
        scheduled_nanos: state.scheduled_nanos,
        capture_steady_nanos: hypha_time::steady_now().nanos(),
        missed_periods,
        replay_cut_nanos,
        trigger_step,
        ..CycleTime::default()
    };
    let inputs = &state.inputs;
    let input_options = &state.input_options;
    let supervision = &mut state.supervision;
    let emissions = &mut state.emissions;
    let cycle_name = cycle.name();
    let mut step_outcome = StepOutcome::default();
    let mut step_state = StepState {
        step,
        inputs,
        input_options,
        supervision,
        transforms: &mut state.transforms,
        cycle_name,
        outputs,
        outcome: &mut step_outcome,
        is_replay,
        waitset: set,
        lent: &mut state.lent,
    };

    let mut send_stamps: Option<(u64, u64)> = None;
    let result = runtime.run_iteration_with_emissions(
        timing,
        || step_nanos,
        stamp,
        emissions,
        |captures, time, emissions, budget| {
            run_step(captures, time, emissions, budget, &mut step_state)
        },
        |index, loan| send_output(outputs, index, loan, step_nanos, &mut send_stamps),
    );

    // A replay step that lost samples is not faithful, so it fails, after its
    // supervision and health are applied. Outside the lockstep nothing paces the inputs,
    // and a loss is only warned of, as in a ROS playback.
    let fidelity = match (replay_cut_nanos, replay_losses(timing, set)) {
        (Some(cut), Some(losses)) => {
            report_replay_fidelity(inputs, input_options, runtime, cut, losses)
        }
        _ => Ok(()),
    };
    if step_outcome.answer.is_some() {
        apply_rejections(cycle_name, inputs, &state.port_names, runtime, supervision);
        apply_supervision(&state.input_rows, &state.input_options, runtime, supervision);
        apply_clears(cycle_name, inputs, &state.port_names, runtime, supervision);
        for input in &mut state.transforms {
            input.apply_clear(cycle_name);
        }
    }
    if let Some(duration_nanos) = step_outcome.duration_nanos {
        // An overrun is a host-time fact: under simulated time a slow step slows the clock.
        let host_rate = !hypha_time::is_sim_time() && !is_replay;
        let period = if host_rate { u64::try_from(cycle.period_nanos()).unwrap_or(0) } else { 0 };
        runtime.record_step(duration_nanos, period);
        crate::locked(&cycle.snapshots.step_times)
            .record(duration_nanos, u64::try_from(timing.capture_steady_nanos).unwrap_or(0));
    }
    log_judgement_events(cycle_name, inputs, &state.port_names, input_options, runtime);
    store_health(cycle, inputs, input_options, runtime, is_replay);
    match (result, fidelity) {
        (Err(status), _) => HyphaCallbackResult::fail(status),
        (Ok(()), Err(status)) => HyphaCallbackResult::fail(status as i32),
        (Ok(()), Ok(())) => step_outcome.answer.unwrap_or_else(HyphaCallbackResult::continue_run),
    }
}

/// One tick of the committed cycle `cycle` of `set`, without a `WaitSet` run. Not part
/// of the C ABI: `hypha_ffi` wraps it for its tests and its cycle benchmark.
#[doc(hidden)]
pub fn cycle_tick(
    set: &crate::waitset::HyphaWaitSet,
    cycle: super::CycleId,
    missed_periods: u64,
) -> HyphaCallbackResult {
    let Ok(held) = set.cycle(cycle) else {
        return HyphaCallbackResult::fail(HyphaStatus::InvalidState as i32);
    };
    cycle_timer_tick(&held, set, missed_periods)
}

/// Call the SDK step and turn its answer into the emissions to dispatch.
///
///
/// A latched emission error wins over a successful callback result. A callback
/// that stops the run or continues with a status authorizes no publication.
fn run_step(
    captures: &[BindingCapture],
    time: CycleTime,
    emissions: &mut Vec<StagedMessage<Option<HyphaLoan>>>,
    budget: &mut EmissionBudget<'_>,
    state: &mut StepState<'_>,
) -> Result<(), i32> {
    // The TF inputs take the transforms of this capture before the step looks them up.
    for input in state.transforms.iter_mut() {
        input
            .ingest(state.cycle_name, captures, replay_losses(time, state.waitset))
            .map_err(|status| status as i32)?;
    }
    let mut emission = EmissionView {
        emissions,
        outputs: state.outputs,
        budget,
        open: false,
        error: None,
        lent: &mut *state.lent,
    };
    let image = ImageView::new(
        captures,
        state.inputs,
        state.input_options,
        state.supervision,
        state.transforms,
        state.is_replay,
    );
    let abi_time = abi_time(time);
    let called_at = std::time::Instant::now();
    let answer = state.step.step(
        (&raw const image).cast(),
        &raw const abi_time,
        std::ptr::from_mut(&mut emission).cast(),
    );
    let duration = called_at.elapsed();
    state.outcome.answer = Some(answer);
    state.outcome.duration_nanos = Some(u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX));
    // A reservation the callback did not commit fails the step; a loan it did not send
    // goes back unsent.
    if emission.open && emission.error.is_none() {
        emission.error = Some(Error::InvalidState);
    }
    emission.lent.clear();
    if state.waitset.failure_requested() {
        emission.emissions.clear();
        return Err(HyphaStatus::ApplicationFailed as i32);
    }
    if let Some(error) = emission.error {
        return Err(error as i32);
    }
    match answer.action {
        HYPHA_ACTION_CONTINUE if answer.status == 0 => Ok(()),
        HYPHA_ACTION_CONTINUE | HYPHA_ACTION_STOP => {
            emission.emissions.clear();
            Ok(())
        }
        HYPHA_ACTION_FAIL if answer.status == 0 => Err(HyphaStatus::Backend as i32),
        HYPHA_ACTION_FAIL => Err(answer.status),
        _ => Err(HyphaStatus::InvalidArgument as i32),
    }
}

/// Apply the samples the finished step rejected as undecodable, and log the undecodable
/// line for the first such sample of an input, then at most once per 10 s.
fn apply_rejections(
    cycle_name: &str,
    inputs: &[Arc<CycleInputSource>],
    port_names: &[String],
    runtime: &mut CycleRuntime,
    copies: &mut [RefCell<SupervisionCopy>],
) {
    for (index, copy) in copies.iter_mut().enumerate() {
        let (rejected, newly) = copy.get_mut().take_rejected();
        if rejected.is_empty() {
            continue;
        }
        for sequence in rejected {
            runtime.reject_sample(index, sequence);
        }
        let Some(source) = inputs.get(index) else {
            continue;
        };
        let count = source.undecodable.fetch_add(newly, std::sync::atomic::Ordering::AcqRel);
        let count = count.wrapping_add(newly);
        let mut logged = crate::locked(&source.undecodable_logged);
        let now = std::time::Instant::now();
        let due = logged.is_none_or(|at| now.duration_since(at) >= UNDECODABLE_LOG_INTERVAL);
        let log_now = newly > 0 && due;
        if !log_now {
            continue;
        }
        *logged = Some(now);
        drop(logged);
        let port = port_names.get(index).map_or("", String::as_str);
        let subject = input_subject(cycle_name, port, &source.topic);
        let data_type = source.subscriber.as_deref().map_or("", |s| s.data_type.as_str());
        log::warn!(target: "hypha", "hypha: {subject}: a sample did not decode as {data_type}; dropped ({count} so far)");
    }
}

/// The least time between two undecodable lines of one input.
const UNDECODABLE_LOG_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);

/// The name of one input in a log line.
fn input_subject(cycle_name: &str, port: &str, topic: &str) -> String {
    if port.is_empty() {
        format!("cycle '{cycle_name}' input ({topic})")
    } else {
        format!("cycle '{cycle_name}' input '{port}' ({topic})")
    }
}

fn apply_supervision(
    rows: &[crate::introspection::IntrospectionRow],
    options: &[super::types::HyphaCycleBindingOptions],
    runtime: &mut CycleRuntime,
    updates: &mut [RefCell<SupervisionCopy>],
) {
    for (index, copy) in updates.iter_mut().enumerate() {
        let update = copy.get_mut().update;
        if let Some(value) = update.alive {
            runtime.apply_alive(index, value);
        }
        if let Some(ignored) = update.ignored {
            let before = runtime
                .binding_capture(index)
                .and_then(|capture| capture.supervision())
                .map(hypha_cycle::Supervision::ignored);
            runtime.set_ignored(index, ignored);
            if before == Some(ignored) {
                continue;
            }
            super::introspection::refresh_input_row(rows, options, runtime, index);
        }
    }
}

/// Clear each input the finished step asked to clear, and log it once per clear.
fn apply_clears(
    cycle_name: &str,
    inputs: &[Arc<CycleInputSource>],
    port_names: &[String],
    runtime: &mut CycleRuntime,
    copies: &mut [RefCell<SupervisionCopy>],
) {
    for (index, copy) in copies.iter_mut().enumerate() {
        if !std::mem::take(&mut copy.get_mut().update.clear) {
            continue;
        }
        let Ok(dropped) = runtime.clear_binding(index) else {
            continue;
        };
        let topic = inputs.get(index).map_or("", |source| source.topic.as_str());
        let port = port_names.get(index).map_or("", String::as_str);
        let subject = input_subject(cycle_name, port, topic);
        log::info!(target: "hypha", "hypha: {subject} cleared: {dropped} pending sample(s) dropped");
    }
}

/// Log each judgement change of a wired input in the finished step, once.
fn log_judgement_events(
    cycle_name: &str,
    inputs: &[Arc<CycleInputSource>],
    port_names: &[String],
    input_options: &[super::HyphaCycleBindingOptions],
    runtime: &mut CycleRuntime,
) {
    for (index, source) in inputs.iter().enumerate() {
        let events = runtime.take_judgement_events(index);
        let no_event = events.iter().all(Option::is_none);
        if no_event || source.subscriber.is_none() {
            continue;
        }
        let port = port_names.get(index).map_or("", String::as_str);
        let subject = input_subject(cycle_name, port, &source.topic);
        let advisory = input_options.get(index).is_some_and(|options| options.optional != 0);
        for event in events.into_iter().flatten() {
            let (level, text) = hypha_cycle::judgement_line(&subject, event, advisory);
            log::log!(target: "hypha", level, "hypha: {text}");
        }
    }
}

/// Report the replay losses of one finished step, one line per topic, and fail
/// the step when it lost any sample and `losses` says so.
///
/// A transport sequence gap counts once per source, however many cycles read it. The
/// failure does not wait for the rate-limited log line.
fn report_replay_fidelity(
    inputs: &[Arc<CycleInputSource>],
    input_options: &[super::types::HyphaCycleBindingOptions],
    runtime: &CycleRuntime,
    cut_nanos: u64,
    losses: Losses,
) -> Result<(), Error> {
    let now = Instant::now();
    let mut lost_any = false;
    for (index, source) in inputs.iter().enumerate() {
        let capture = runtime.binding_capture(index);
        // Data on another clock is captured in host time: no replay losses.
        if capture.is_some_and(BindingCapture::host_clock) {
            continue;
        }
        let lost = capture.map_or(0, |capture| capture.lost_since_capture);
        let missing_total = source.missing_sequences();
        let mut fidelity = crate::locked(&source.fidelity);
        let (new_losses, line) = fidelity.record(&source.topic, lost, missing_total, now);
        // A queued input with nothing new before the cut is a sparse input: only a
        // latest binding is warned of.
        let latest = input_options.get(index).is_some_and(|options| {
            options.mode == super::types::HyphaCycleInputMode::Latest as u32
        });
        let history = source.history.as_ref().filter(|_| latest);
        let short_warning = history.and_then(|history| {
            let history = crate::locked(history);
            let samples = history.samples();
            let sent = samples.iter().map(|sample| sample.framework_sent_nanos);
            is_short_history(history.newest_evicted_sent_nanos(), sent, cut_nanos)
                .then(|| {
                    fidelity.record_short_history(&source.topic, samples.len(), cut_nanos, now)
                })
                .flatten()
        });
        drop(fidelity);
        lost_any |= new_losses > 0;
        match (line, losses) {
            (Some(text), Losses::Fail) => log::error!(target: "hypha", "hypha: {text}"),
            (Some(text), Losses::Warn) => {
                log::warn!(target: "hypha", "hypha: {text}; outside the replay lockstep the run goes on");
            }
            (None, _) => {}
        }
        if let Some(text) = short_warning {
            log::warn!(target: "hypha", "hypha: {text}");
        }
    }
    if lost_any && losses == Losses::Fail { Err(Error::SequenceGap) } else { Ok(()) }
}

/// What lost input samples do to the run of a step at `time` of a cycle of `set`: none
/// for a step that is no replay step.
fn replay_losses(time: CycleTime, set: &crate::waitset::HyphaWaitSet) -> Option<Losses> {
    time.replay_cut_nanos?;
    Some(if set.node().open_loop { Losses::Warn } else { Losses::Fail })
}

/// What a replay step's lost input samples do to its run.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Losses {
    /// In the replay lockstep: the step is not faithful and fails the run.
    Fail,
    /// In an open-loop playback (`HYPHA_REPLAY_LOCKSTEP=0`): warned of.
    Warn,
}

/// Warn, rate-limited per topic, when a replay step cannot admit samples stamped more
/// than `bound_nanos` past `cut_nanos`.
pub(crate) fn report_time_base(cycle: &Cycle, cut_nanos: i64, bound_nanos: u64) {
    let Ok(cut) = u64::try_from(cut_nanos) else {
        return;
    };
    let Ok(step) = cycle.step_ref() else {
        return;
    };
    let now = Instant::now();
    for (index, source) in step.inputs.iter().enumerate() {
        // Data on another clock is stamped on that clock: no time-base warning.
        if step.runtime.binding_capture(index).is_some_and(BindingCapture::host_clock) {
            continue;
        }
        let Some(history) = source.history.as_ref() else {
            continue;
        };
        let ahead = crate::locked(history).stamped_beyond(cut, bound_nanos);
        let mut fidelity = crate::locked(&source.fidelity);
        let line = fidelity.record_time_base(&source.topic, ahead, bound_nanos, now);
        drop(fidelity);
        if let Some(text) = line {
            log::warn!(target: "hypha", "hypha: {text}");
        }
    }
}

/// Send one staged message on its output, in dispatch order.
///
/// The frame takes its sequence number when it is sent, so a message that is never
/// dispatched leaves no gap. A message whose loan was refused at reserve takes its
/// sequence number here, and the subscriber counts that loss.
fn send_output(
    outputs: &[CycleOutput],
    index: u32,
    loan: Option<HyphaLoan>,
    step_nanos: i64,
    stamps: &mut Option<(u64, u64)>,
) -> Result<(), OutputError> {
    let output = usize::try_from(index)
        .ok()
        .and_then(|position| outputs.get(position))
        .ok_or(OutputError::Failure(HyphaStatus::InvalidArgument as i32))?;
    let Some(mut loan) = loan else {
        take_sequence(&output.publisher.next_sequence);
        return Err(OutputError::PoolExhausted);
    };
    let (steady_stamp, framework_stamp) = if let Some(cached) = *stamps {
        cached
    } else {
        let steady = u64::try_from(hypha_time::steady_now().nanos())
            .map_err(|_| OutputError::Failure(HyphaStatus::InvalidState as i32))?;
        // A replay cut is refused before the step when negative; the clock reading is not.
        let framework = u64::try_from(step_nanos)
            .map_err(|_| OutputError::Failure(HyphaStatus::InvalidState as i32))?;
        let resolved = (steady, framework);
        *stamps = Some(resolved);
        resolved
    };
    loan.stamp_transport_sent(steady_stamp);
    loan.stamp_framework_sent(framework_stamp);
    loan.send_frame().map_err(|error| classify_output_error(HyphaStatus::from(error)))
}

fn classify_output_error(status: HyphaStatus) -> OutputError {
    if status == HyphaStatus::PoolExhausted {
        OutputError::PoolExhausted
    } else {
        OutputError::Failure(status as i32)
    }
}

/// Framework time under simulated time, else Unix wall time. `None` when the
/// wall clock reads before the Unix epoch.
fn message_stamp(simulated_time: bool) -> Option<i64> {
    if simulated_time {
        Some(hypha_time::now().nanos())
    } else {
        hypha_time::try_wall_now().map(hypha_time::Time::nanos)
    }
}

fn abi_time(time: CycleTime) -> HyphaCycleTime {
    HyphaCycleTime {
        scheduled_nanos: time.scheduled_nanos,
        callback_start_nanos: time.callback_start_nanos,
        capture_steady_nanos: time.capture_steady_nanos,
        elapsed_nanos: time.elapsed_nanos,
        missed_periods: time.missed_periods,
        has_elapsed: u8::from(time.has_elapsed),
        message_stamp_nanos: time.message_stamp_nanos,
        message_stamp_valid: u8::from(time.message_stamp_valid),
    }
}

fn abi_health(health: CycleHealth) -> HyphaCycleHealth {
    HyphaCycleHealth {
        iterations: health.iterations,
        missed_periods: health.missed_periods,
        callback_failures: health.callback_failures,
        output_attempts: health.output_attempts,
        output_successes: health.output_successes,
        output_failures: health.output_failures,
        output_pool_drops: health.output_pool_drops,
        observed_framework_nanos: health.observed_framework_nanos,
        observed_steady_nanos: health.observed_steady_nanos,
        steps: health.steps,
        last_step_nanos: health.last_step_nanos,
        max_step_nanos: health.max_step_nanos,
        overruns: health.overruns,
    }
}

fn store_health(
    cycle: &Cycle,
    inputs: &[Arc<CycleInputSource>],
    input_options: &[super::HyphaCycleBindingOptions],
    runtime: &CycleRuntime,
    is_replay: bool,
) {
    let mut health = crate::locked(&cycle.snapshots.health);
    *health = abi_health(runtime.health());
    let mut counts = crate::locked(&cycle.snapshots.input_counts);
    counts.clear();
    for input in inputs {
        let subscriber = input.subscriber.as_deref();
        counts.push(super::state::InputCounts {
            rejected: subscriber.map_or(0, crate::subscriber::HyphaSubscriber::rejected_frames),
            undecodable: input.undecodable.load(std::sync::atomic::Ordering::Acquire),
        });
    }
    drop(counts);
    let mut watches = crate::locked(&cycle.snapshots.watches);
    if watches.len() != inputs.len() {
        watches.clear();
        watches.extend(inputs.iter().map(|input| {
            input.subscriber.as_deref().and_then(|subscriber| subscriber.watch.clone())
        }));
    }
    drop(watches);
    let mut bindings = crate::locked(&cycle.snapshots.bindings);
    for (index, item) in bindings.iter_mut().enumerate() {
        let Some(capture) = runtime.binding_capture(index) else {
            continue;
        };
        let input = inputs.get(index).map(Arc::as_ref);
        let options = input_options.get(index);
        *item = binding_view(capture, input, options, is_replay, capture.supervision());
    }
}

/// Everything one binding reports at its latest capture. `supervision` is the state
/// to report: the runtime's after a step, the step's own copy inside it.
pub(super) fn binding_view(
    capture: &BindingCapture,
    input: Option<&CycleInputSource>,
    options: Option<&super::HyphaCycleBindingOptions>,
    is_replay: bool,
    supervision: Option<&hypha_cycle::Supervision>,
) -> HyphaCycleBindingView {
    let wired = input.is_some_and(|source| source.subscriber.is_some());
    let advisory = options.is_some_and(|options| options.optional != 0);
    let counts = wired && !advisory;
    // A transport gap is a lost sample only for a binding that needs every one,
    // or any binding in replay.
    let transport_gaps = if counts_losses(capture.mode(), is_replay && !capture.host_clock()) {
        input.map_or(0, CycleInputSource::missing_sequences)
    } else {
        0
    };
    let held = hypha_cycle::held_report(capture);
    HyphaCycleBindingView {
        health: HyphaCycleBindingHealth {
            capture: binding_report(capture),
            overrun_total: capture.counters.overrun_total.wrapping_add(transport_gaps),
            work_limit_drops_total: capture.counters.work_limit_drops_total,
            age_rejections_total: capture.counters.age_rejections_total,
            latest_unavailable_iterations: capture.counters.latest_unavailable_iterations,
            usable_to_stale_transitions: capture.counters.usable_to_stale_transitions,
            cleared_total: capture.counters.cleared_total,
        },
        held: HyphaCycleHeldReport {
            received_total: held.received_total,
            delivered_total: held.delivered_total,
            has_held: u8::from(held.held_sequence.is_some()),
            held_sequence: held.held_sequence.unwrap_or(0),
        },
        supervision: supervision.map_or_else(HyphaSupervisionReport::default, |state| {
            HyphaSupervisionReport { faults: state.faults(), ignored: u8::from(state.ignored()) }
        }),
        ok: u8::from(capture.ok(counts, supervision)),
        age_nanos: capture.age_nanos,
    }
}
