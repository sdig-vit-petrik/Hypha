//! Replay cycles on simulated time: the steps of one timer turn.
//!
//! A replay cycle runs one step per expired deadline. The dispatcher drains its inputs
//! once, then cuts each step at its deadline.

use std::time::Instant;

use super::dispatch::CallbackDispatcher;
use super::upstream::UpstreamHold;
use super::{AttachmentId, HyphaStatus, Progression, Registration, Target, invoke_cycle};
use crate::cycle::{Cycle, CycleId, TIME_BASE_BOUND_PERIODS};
use crate::status::Error;
use hypha_runtime::replay_steps::{Step, window_steps};
use hypha_runtime::upstream_hold::{Admit, Limit, Window, nth_deadline};

/// The cycle that a timer registration steps, and the upstream hold of its replay steps.
///
/// The registration owns the hold: no C call on the cycle reaches it during a step.
pub(super) struct CycleTimer {
    pub(super) cycle: CycleId,
    pub(super) upstream: UpstreamHold,
    /// Admitted deadlines a turn's slice did not reach, run first on the next turn.
    pub(super) deferred: Option<Window>,
}

impl CycleTimer {
    /// The timer of `cycle`, which holds no deadline yet.
    pub(super) fn new(cycle: CycleId) -> Box<Self> {
        Box::new(Self { cycle, upstream: UpstreamHold::default(), deferred: None })
    }
}

/// The most steps a replay cycle runs in one `WaitSet` turn.
pub(super) const MAX_STEPS_PER_TURN: u64 = 1024;

/// The steady time after which a turn starts no further step of a replay cycle. At
/// least one step runs per turn.
pub(super) const STEP_SLICE_BUDGET: std::time::Duration = std::time::Duration::from_millis(50);

/// The expired deadlines of one replay cycle: `periods` of them, from `first_nanos`
/// in steps of `period_nanos`.
#[derive(Clone, Copy)]
pub(super) struct ReplayDue {
    pub(super) attachment: AttachmentId,
    pub(super) cycle: CycleId,
    pub(super) first_nanos: i64,
    pub(super) period_nanos: i64,
    pub(super) periods: u64,
}

impl CallbackDispatcher<'_> {
    /// Run a replay cycle's expired deadlines, one step each.
    ///
    /// The inputs are drained once, and every step then captures the samples
    /// stamped before its own deadline.
    pub(super) fn run_replay(&mut self, due: ReplayDue) -> Progression {
        let ReplayDue { attachment, cycle, first_nanos, period_nanos, periods } = due;
        let cycle = match self.cycle(cycle) {
            Ok(cycle) => cycle,
            Err(status) => return self.fail(attachment, HyphaStatus::from(status)),
        };
        // The limit is read before the drain, so the drain takes every output it covers.
        let limit = match self.upstream_limit(attachment, cycle) {
            Ok(limit) => limit,
            Err(status) => return self.fail(attachment, HyphaStatus::from(status)),
        };
        if let Err(status) = crate::cycle::drain_inputs(cycle) {
            return self.fail(attachment, HyphaStatus::from(status));
        }
        let new = (periods > 0).then_some(Window { first_nanos, periods });
        let long_jump = periods > MAX_STEPS_PER_TURN;
        if long_jump {
            let last_nanos = nth_deadline(first_nanos, period_nanos, periods.saturating_sub(1));
            log::info!(target: "hypha", "hypha: cycle '{}' steps {periods} periods for a forward jump of /clock from {first_nanos} ns to {} ns", cycle.name(), last_nanos.unwrap_or(i64::MAX));
        }
        let admitted = upstream_of(self.registrations, attachment).and_then(|upstream| {
            let admit = upstream.hold.admit(new, period_nanos, &limit, Instant::now());
            Ok((admit.map_err(Error::from)?, upstream.hold.counted_last()))
        });
        let fresh = match admitted {
            Ok((Admit::Run(window), counted)) => {
                report_counted(cycle, counted, period_nanos);
                (window.periods > 0).then_some(window)
            }
            Ok((Admit::Hold, counted)) => {
                report_counted(cycle, counted, period_nanos);
                None
            }
            Err(status) => return self.fail(attachment, HyphaStatus::from(status)),
        };
        let deferred = match deferred_of(self.registrations, attachment) {
            Ok(deferred) => deferred.take(),
            Err(status) => return self.fail(attachment, HyphaStatus::from(status)),
        };
        let window = match join_windows(deferred, fresh, period_nanos) {
            Ok(Some(window)) => window,
            Ok(None) => return Progression::Continue,
            Err(status) => return self.fail(attachment, HyphaStatus::from(status)),
        };
        let started = Instant::now();
        let mut ran: u64 = 0;
        for step in window_steps(window, period_nanos) {
            let Ok(Step { deadline }) = step else {
                return self.fail(attachment, HyphaStatus::InvalidState);
            };
            // A step may detach its own cycle's registration: no further step then.
            if self.is_detached(attachment) {
                return Progression::Continue;
            }
            let slice_is_over = ran >= MAX_STEPS_PER_TURN || started.elapsed() >= STEP_SLICE_BUDGET;
            if ran > 0 && slice_is_over {
                return self.defer_rest(attachment, window, ran, period_nanos);
            }
            let decision = self.run_replay_step(attachment, cycle, deadline);
            if decision != Progression::Continue {
                return decision;
            }
            ran = ran.saturating_add(1);
        }
        Progression::Continue
    }

    /// Keep the deadlines of `window` after its first `ran` for the next turn.
    fn defer_rest(
        &mut self,
        attachment: AttachmentId,
        window: Window,
        ran: u64,
        period_nanos: i64,
    ) -> Progression {
        let Some(first_nanos) = nth_deadline(window.first_nanos, period_nanos, ran) else {
            return self.fail(attachment, HyphaStatus::InvalidState);
        };
        let rest = Window { first_nanos, periods: window.periods.saturating_sub(ran) };
        match deferred_of(self.registrations, attachment) {
            Ok(deferred) => {
                *deferred = Some(rest);
                Progression::Continue
            }
            Err(status) => self.fail(attachment, HyphaStatus::from(status)),
        }
    }

    fn fail(&mut self, attachment: AttachmentId, status: HyphaStatus) -> Progression {
        super::dispatch::fail_with(&mut self.failure, attachment, status)
    }

    /// Resolve the upstream names of `cycle`, and return the limit they give now.
    fn upstream_limit(&mut self, attachment: AttachmentId, cycle: &Cycle) -> Result<Limit, Error> {
        let node = self.node;
        let upstream = upstream_of(self.registrations, attachment)?;
        upstream.settle(&cycle.step_ref()?.inputs, node)
    }

    /// Run one replay step at `deadline`, with the inputs cut at that deadline.
    fn run_replay_step(
        &mut self,
        attachment: AttachmentId,
        cycle: &Cycle,
        deadline: i64,
    ) -> Progression {
        if let Err(status) = cycle.step_mut().map(|mut step| {
            step.scheduled_nanos = deadline;
            step.input_cut = Some(deadline);
        }) {
            return self.fail(attachment, HyphaStatus::from(status));
        }
        let result = invoke_cycle(self.health, cycle, self.set, 0);
        // Clearing it here keeps a step that never ran from leaking its cut.
        if let Ok(mut step) = cycle.step_mut() {
            step.input_cut = None;
        }
        self.act(attachment, result)
    }
}

/// Report the time base up to `counted`, the newest deadline the timer has counted, held
/// or run.
fn report_counted(cycle: &Cycle, counted: Option<i64>, period_nanos: i64) {
    let Some(last) = counted else {
        return;
    };
    let period = u64::try_from(period_nanos).unwrap_or(0);
    crate::cycle::report_time_base(cycle, last, TIME_BASE_BOUND_PERIODS.saturating_mul(period));
}

/// The upstream hold of the cycle timer `attachment`; its absence is an invalid state.
fn upstream_of(
    registrations: &mut [Registration],
    attachment: AttachmentId,
) -> Result<&mut UpstreamHold, Error> {
    registrations
        .iter_mut()
        .find(|registration| registration.id == attachment)
        .and_then(|registration| match registration.target {
            Target::Timer { cycle: Some(ref mut timer), .. } => Some(&mut timer.upstream),
            _ => None,
        })
        .ok_or(Error::InvalidState)
}

/// The deferred deadlines of the cycle timer `attachment`. See [`upstream_of`].
fn deferred_of(
    registrations: &mut [Registration],
    attachment: AttachmentId,
) -> Result<&mut Option<Window>, Error> {
    registrations
        .iter_mut()
        .find(|registration| registration.id == attachment)
        .and_then(|registration| match registration.target {
            Target::Timer { cycle: Some(ref mut timer), .. } => Some(&mut timer.deferred),
            _ => None,
        })
        .ok_or(Error::InvalidState)
}

/// The deferred deadlines followed by the fresh ones.
fn join_windows(
    deferred: Option<Window>,
    fresh: Option<Window>,
    period_nanos: i64,
) -> Result<Option<Window>, Error> {
    let (Some(deferred), Some(fresh)) = (deferred, fresh) else {
        return Ok(deferred.or(fresh));
    };
    let contiguous = nth_deadline(deferred.first_nanos, period_nanos, deferred.periods)
        == Some(fresh.first_nanos);
    if !contiguous {
        return Err(Error::InvalidState);
    }
    let periods = deferred.periods.checked_add(fresh.periods).ok_or(Error::InvalidState)?;
    Ok(Some(Window { first_nanos: deferred.first_nanos, periods }))
}
