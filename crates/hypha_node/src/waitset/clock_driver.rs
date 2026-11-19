//! A cycle that drives `/clock` for a simulator: once every participant acknowledged the
//! current value, the cycle steps at the next value and only then publishes `/clock` for it.

use std::time::{Duration, Instant};

use hypha_runtime::clock_barrier::{DriverAction, DriverOptions, DriverSchedule, clock_payload};

use super::dispatch::{CallbackDispatcher, fail_with, record_action};
use super::registrations::{Request, attach};
use super::{AttachmentId, HyphaAttachmentId, invoke_cycle};
use super::{Progression, ReceiveFailure, Target};
use crate::cycle::{Cycle, CycleId, DriverSetup};
use crate::publisher::HyphaPublisher;
use crate::status::Error;
use crate::subscriber::HyphaSubscriber;

/// How often the driver looks for acknowledgements, as `hypha_play` does.
pub(super) const ACK_POLL_INTERVAL: Duration = Duration::from_millis(1);

/// The registration state of one driver cycle. Dropping it turns simulated time off.
pub(crate) struct DriverCycle {
    pub(super) cycle: CycleId,
    schedule: DriverSchedule,
    clock: HyphaPublisher,
    progress: Box<HyphaSubscriber>,
}

impl Drop for DriverCycle {
    fn drop(&mut self) {
        crate::clock::clock_disable_sim_time();
    }
}

/// Enable simulated time at the driver's start value, open its `/clock` publisher and
/// `/clock_progress` input, and attach it with a 1 ms wake. On failure nothing stays
/// attached and simulated time is off again.
pub(crate) fn attach_driver_cycle(
    set: &super::HyphaWaitSet,
    cycle: &Cycle,
    setup: &DriverSetup,
) -> Result<HyphaAttachmentId, Error> {
    let registration = cycle;
    let factor = setup.options.real_time_factor;
    let options = DriverOptions {
        start_nanos: setup.options.start_nanos,
        period_nanos: registration.period_nanos(),
        timeout: Duration::from_nanos(setup.options.timeout_nanos),
        real_time_factor: (factor > 0.0).then_some(factor),
        expected: setup.expected.clone(),
    };
    let schedule = match DriverSchedule::new(options, Instant::now()) {
        Ok(schedule) => schedule,
        Err(reason) => {
            log::error!(target: "hypha", "hypha: clock driver '{}' is refused: {reason}", registration.name());
            return Err(Error::InvalidArgument);
        }
    };
    let node = set.node();
    // One clock per bus: another process that publishes `/clock` would fight this one.
    let probe =
        crate::subscriber::create_sim_clock_subscriber(&node.transport, &node.introspection)?;
    let publishers = probe.port.publisher_count().map_err(Error::from)?;
    drop(probe);
    if publishers > 0 {
        log::error!(target: "hypha", "hypha: clock driver '{}' is refused: another process already publishes /clock in this domain", registration.name());
        return Err(Error::InvalidState);
    }
    let publisher_settings = crate::publisher::PublisherSettings {
        latch: false,
        loaned_samples: 0,
        declaration: crate::qos::Declaration::undeclared(
            hypha_core::port::PayloadCapacity::Growable,
        ),
    };
    let clock = crate::publisher::open_publisher(
        node,
        "/clock",
        "rosgraph_msgs/Clock",
        crate::clock::CLOCK_MD5,
        "time clock",
        publisher_settings,
    )?;
    let progress =
        crate::subscriber::create_progress_subscriber(&node.transport, &node.introspection)?;
    crate::clock::clock_enable_sim_time(setup.options.start_nanos)?;
    // From here a failure drops the driver, which turns simulated time off again.
    let driver = Box::new(DriverCycle { cycle: cycle.id, schedule, clock, progress });
    attach(set, Request::Driver(driver))
}

impl CallbackDispatcher<'_> {
    /// One turn of the driver cycle `attachment`: read the reports, then step, resend,
    /// wait or fail as its schedule says.
    pub(super) fn run_driver(&mut self, attachment: AttachmentId) -> Progression {
        self.driver_turn(attachment)
            .unwrap_or_else(|status| fail_with(&mut self.failure, attachment, status))
    }

    fn driver_turn(&mut self, attachment: AttachmentId) -> Result<Progression, Error> {
        let node = self.node;
        let health = self.health;
        let set = self.set;
        let id = self
            .registrations
            .iter()
            .find(|registration| registration.id == attachment)
            .and_then(|registration| match registration.target {
                Target::Driver(ref state) => Some(state.cycle),
                _ => None,
            })
            .ok_or(Error::InvalidState)?;
        let cycle: &Cycle = self.cycle(id)?;
        let state = self
            .registrations
            .iter_mut()
            .find(|registration| registration.id == attachment)
            .and_then(|registration| match registration.target {
                Target::Driver(ref mut state) => Some(&mut **state),
                _ => None,
            })
            .ok_or(Error::InvalidState)?;
        let own = node.name.as_str();
        let limit = state.progress.port.buffer_size().saturating_add(1);
        crate::cycle::drain_at_most(limit, || match state.progress.receive_validated() {
            Ok(Some(sample)) => {
                let report = hypha_runtime::upstream_reports::decode_report(own, sample.payload())
                    .map_err(|_| Error::CorruptPayload)?;
                if let Some((node, report)) = report {
                    state.schedule.observe(
                        &node,
                        report.clock_nanos,
                        report.sent_nanos,
                        Instant::now(),
                    );
                }
                Ok(crate::cycle::DrainStep::Taken)
            }
            Err(ReceiveFailure::Refused(_)) => Ok(crate::cycle::DrainStep::Taken),
            Ok(None) | Err(ReceiveFailure::Transport(_)) => Ok(crate::cycle::DrainStep::Empty),
        })?;
        match state.schedule.next(Instant::now()) {
            DriverAction::Wait => Ok(Progression::Continue),
            DriverAction::Exhausted => {
                log::error!(target: "hypha", "hypha: clock driver '{}': the next clock value is not representable", cycle.name());
                Err(Error::InvalidState)
            }
            DriverAction::Resend { value } => {
                publish_clock(&state.clock, value)?;
                state.schedule.sent(value, 0, Instant::now());
                Ok(Progression::Continue)
            }
            DriverAction::Fail { silent, last_clock_nanos } => {
                let last = last_clock_nanos.map_or_else(
                    || "it never reported".to_owned(),
                    |nanos| format!("its last clock was {nanos} ns"),
                );
                log::error!(target: "hypha", "hypha: clock driver '{}': participant '{}' did not acknowledge: {last}", cycle.name(), silent.join("', '"));
                Err(Error::Timeout)
            }
            DriverAction::Step { deadline } => {
                crate::clock::clock_set_sim_time(deadline)?;
                crate::cycle::drain_inputs(cycle)?;
                {
                    let mut step = cycle.step_mut()?;
                    step.scheduled_nanos = deadline;
                    step.input_cut = Some(deadline);
                }
                let result = invoke_cycle(health, cycle, set, 0);
                if let Ok(mut step) = cycle.step_mut() {
                    step.input_cut = None;
                }
                let decision = record_action(node, &mut self.failure, attachment, result);
                if decision != Progression::Continue {
                    return Ok(decision);
                }
                let floor = u64::try_from(hypha_time::steady_now().nanos())
                    .map_err(|_| Error::InvalidState)?;
                publish_clock(&state.clock, deadline)?;
                state.schedule.sent(deadline, floor, Instant::now());
                Ok(Progression::Continue)
            }
        }
    }
}

/// Publish `/clock = nanos`.
fn publish_clock(clock: &HyphaPublisher, nanos: i64) -> Result<(), Error> {
    let payload = clock_payload(nanos).ok_or(Error::InvalidState)?;
    crate::publisher::send_frame_with(clock, payload.len(), |buffer| {
        buffer.copy_from_slice(&payload);
        Ok(())
    })
    .map_err(Error::from_status)
}
