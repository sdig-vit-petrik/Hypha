//! `WaitSet` dispatch ownership and C ABI implementation.

use hypha_runtime::timer_schedule::{callback_deadline, replay_periods};

use super::replay::{CycleTimer, ReplayDue};
use super::{
    AttachmentId, BatchStep, BatchTurn, CallbackWarnings, Dispatcher, Duration, EventOutcome,
    ExecutionHealth, HYPHA_ACTION_CONTINUE, HYPHA_ACTION_FAIL, HYPHA_ACTION_STOP,
    HyphaCallbackResult, HyphaNode, HyphaStatus, HyphaWaitSet, Instant, Progression,
    ReceiveFailure, Registration, Target, invoke_cycle, invoke_data, invoke_interval,
};
use crate::cycle::{Cycle, CycleId};
use crate::status::Error;

/// One timer firing of the current turn, in registration order.
#[derive(Clone, Copy)]
pub(super) enum Due {
    /// A timer in host time, with its count.
    Timer { attachment: AttachmentId, cycle: Option<CycleId>, missed: u64 },
    /// A replay cycle with expired deadlines.
    Replay(ReplayDue),
    /// A triggered cycle, which decides its own steps.
    Triggered(AttachmentId),
    /// A cycle that drives `/clock`.
    Driver(AttachmentId),
}

/// Connects native readiness to the C callbacks.
pub(super) struct CallbackDispatcher<'a> {
    /// A `Vec`, so the timers a callback attached during the run join before the next wait.
    pub(super) registrations: &'a mut Vec<Registration>,
    /// The cycles of the `WaitSet`, shared for the run.
    pub(super) cycles: &'a [Cycle],
    /// The address of the `WaitSet` this run serves. It keys the `WaitSet`'s offer among
    /// the node's progress reporters.
    pub(super) waitset: usize,
    /// The `WaitSet` this run serves, which a Rust subscription callback receives.
    pub(super) set: &'a HyphaWaitSet,
    /// The node this run serves. Its transport owns the latched topics.
    pub(super) node: &'a HyphaNode,
    pub(super) batch_size: u32,
    pub(super) warn_nanos: u64,
    pub(super) warn_interval_nanos: u64,
    pub(super) slow_callbacks: u64,
    pub(super) failure: Option<(AttachmentId, HyphaStatus)>,
    /// Counts every callback answer, over every run of every `WaitSet` on the node.
    pub(super) health: &'a ExecutionHealth,
    pub(super) due: Vec<Due>,
    /// Registrations a callback detached during this run: never dispatched again.
    pub(super) detached: &'a std::cell::RefCell<Vec<AttachmentId>>,
}

impl Due {
    fn attachment(&self) -> AttachmentId {
        match *self {
            Self::Timer { attachment, .. }
            | Self::Triggered(attachment)
            | Self::Driver(attachment) => attachment,
            Self::Replay(ref replay) => replay.attachment,
        }
    }
}

impl<'a> CallbackDispatcher<'a> {
    /// Whether a callback of this run detached `attachment`.
    pub(super) fn is_detached(&self, attachment: AttachmentId) -> bool {
        is_detached_in(self.detached, attachment)
    }

    /// The cycle a registration named at its attach; its absence is an invalid state.
    pub(super) fn cycle(&self, id: CycleId) -> Result<&'a Cycle, Error> {
        find_cycle(self.cycles, id)
    }
}

/// See [`CallbackDispatcher::cycle`]; for a turn that holds the registrations.
fn find_cycle(cycles: &[Cycle], id: CycleId) -> Result<&Cycle, Error> {
    cycles.iter().find(|cycle| cycle.id == id).ok_or(Error::InvalidState)
}

impl CallbackDispatcher<'_> {
    /// Add slow callbacks to this run's report and to the node's health.
    pub(super) fn count_slow(&mut self, slow: u64) {
        self.slow_callbacks = self.slow_callbacks.wrapping_add(slow);
        self.health.add_slow_callbacks(slow);
    }

    fn find(&mut self, attachment: AttachmentId) -> Option<&mut Registration> {
        self.registrations.iter_mut().find(|registration| registration.id == attachment)
    }

    /// Take the timers a callback attached since the last wait into the dispatch.
    pub(super) fn admit_attached(&mut self) {
        let Ok(mut attaches) = self.set.run_attaches.try_borrow_mut() else {
            return;
        };
        if attaches.pending.is_empty() {
            return;
        }
        self.registrations.append(&mut attaches.pending);
        attaches.held = self.registrations.len();
    }

    /// Apply the resets a call made to plain timers since the dispatcher last looked.
    fn apply_timer_resets(&mut self) {
        for registration in self.registrations.iter_mut() {
            if let Target::Timer {
                ref mut timer,
                ref mut run_anchored,
                control: Some(ref control),
                ..
            } = registration.target
            {
                // A reset that finds simulated time gone back stays pending, and the
                // next due check ends the run for it (`fire_due_timers`).
                let _applied = control.apply_reset(timer, run_anchored);
            }
        }
    }

    /// Whether a callback canceled or reset the plain timer `attachment` since this turn
    /// found its call due.
    fn holds_back(&self, attachment: AttachmentId) -> bool {
        self.registrations.iter().any(|registration| {
            registration.id == attachment
                && matches!(
                    registration.target,
                    Target::Timer { control: Some(ref control), .. } if control.holds_back_due_call()
                )
        })
    }

    /// Fire every clock-driven timer that is due.
    ///
    /// The due time is measured against the node's clock, so a timer follows
    /// simulation pause and jump. The native interval only supplies wake-ups.
    fn fire_due_timers(&mut self, now: hypha_time::Time) -> Progression {
        self.due.clear();
        let (cycles, node) = (self.cycles, self.node);
        // A registration a callback detached is not observed: it neither fires nor fails.
        let live = self.registrations.iter_mut().filter(|r| !is_detached_in(self.detached, r.id));
        for registration in live {
            let due = match self_scheduled(registration) {
                Some(item) => Ok(Some(item)),
                None => timer_due(cycles, node, registration.id, &mut registration.target, now),
            };
            match due {
                Ok(Some(item)) => self.due.push(item),
                Ok(None) => {}
                Err(status) => return fail_with(&mut self.failure, registration.id, status),
            }
        }
        self.run_due(now)
    }

    /// Run the due items of this turn in order, until one does not continue.
    fn run_due(&mut self, now: hypha_time::Time) -> Progression {
        for index in 0..self.due.len() {
            let Some(&item) = self.due.get(index) else {
                break;
            };
            // A callback earlier in this turn may have detached the registration.
            if self.is_detached(item.attachment()) {
                continue;
            }
            let decision = match item {
                // A callback earlier in this turn may have canceled or reset the timer.
                Due::Timer { attachment, .. } if self.holds_back(attachment) => continue,
                Due::Timer { attachment, cycle, missed } => {
                    let result = self.invoke_periodic(attachment, cycle, missed);
                    self.act(attachment, result)
                }
                Due::Replay(replay) => self.run_replay(replay),
                Due::Triggered(attachment) => self.run_triggered(attachment, now.nanos()),
                Due::Driver(attachment) => self.run_driver(attachment),
            };
            if decision != Progression::Continue {
                return decision;
            }
        }
        Progression::Continue
    }

    /// Run one periodic callback, or one step of `cycle` in host time, timed as a
    /// subscription callback is.
    fn invoke_periodic(
        &mut self,
        attachment: AttachmentId,
        cycle: Option<CycleId>,
        missed: u64,
    ) -> HyphaCallbackResult {
        let warn_nanos = self.warn_nanos;
        let warn_interval = self.warn_interval_nanos;
        let health = self.health;
        let set = self.set;
        let owner = match cycle.map(|id| find_cycle(self.cycles, id)) {
            None => None,
            Some(Ok(cycle)) => Some(cycle),
            Some(Err(status)) => return invalid(status),
        };
        let Some(registration) = self.find(attachment) else {
            return invalid(Error::InvalidState);
        };
        let Registration { id, ref mut target, ref mut warnings, .. } = *registration;
        let (result, was_slow) = match (owner, target) {
            (Some(cycle), _) => timed(warnings, id, warn_nanos, warn_interval, || {
                invoke_cycle(health, cycle, set, missed)
            }),
            (None, Target::Timer { callback, .. }) => {
                timed(warnings, id, warn_nanos, warn_interval, || {
                    invoke_interval(health, callback, missed)
                })
            }
            (None, _) => return invalid(Error::InvalidState),
        };
        self.count_slow(u64::from(was_slow));
        result
    }

    pub(super) fn act(
        &mut self,
        attachment: AttachmentId,
        result: HyphaCallbackResult,
    ) -> Progression {
        record_action(self.node, &mut self.failure, attachment, result)
    }
}

/// Turn a callback answer into a run-loop decision, recording any failure in `failure`.
///
/// Takes the dispatcher's fields apart so the borrow of the registration never
/// overlaps a borrow of the whole dispatcher.
pub(super) fn record_action(
    node: &HyphaNode,
    failure: &mut Option<(AttachmentId, HyphaStatus)>,
    attachment: AttachmentId,
    result: HyphaCallbackResult,
) -> Progression {
    match result.action {
        HYPHA_ACTION_STOP => Progression::Stop,
        // A failure the callback did not qualify is still a failure.
        HYPHA_ACTION_FAIL => fail_with(failure, attachment, status_from_code(result.status)),
        // An unknown action is not a licence to guess. A continue with a nonzero status
        // is a failure the run survives.
        _ if !node.is_running() => Progression::Stop,
        _ => Progression::Continue,
    }
}

pub(super) fn fail_with(
    failure: &mut Option<(AttachmentId, HyphaStatus)>,
    attachment: AttachmentId,
    status: impl Into<HyphaStatus>,
) -> Progression {
    *failure = Some((attachment, status.into()));
    Progression::Fail(hypha_core::port::PortError::Backend(35))
}

/// The due item of a cycle that decides its own steps: a triggered or a driver cycle.
fn self_scheduled(registration: &Registration) -> Option<Due> {
    match registration.target {
        Target::Triggered(_) => Some(Due::Triggered(registration.id)),
        Target::Driver(_) => Some(Due::Driver(registration.id)),
        _ => None,
    }
}

/// The answer that fails the run with `status`: a due item whose registration or cycle is gone.
fn invalid(status: Error) -> HyphaCallbackResult {
    HyphaCallbackResult::fail(HyphaStatus::from(status) as i32)
}

/// Time one callback of registration `id` and count it in `warnings` when slow.
pub(super) fn timed<F: FnOnce() -> HyphaCallbackResult>(
    warnings: &mut CallbackWarnings,
    id: AttachmentId,
    warn_nanos: u64,
    warn_interval_nanos: u64,
    body: F,
) -> (HyphaCallbackResult, bool) {
    if warn_nanos == 0 {
        return (body(), false);
    }
    let started = Instant::now();
    let result = body();
    let elapsed = started.elapsed();
    if !hypha_core::dispatch::is_slow_callback(elapsed, warn_nanos) {
        return (result, false);
    }
    warnings.slow_callbacks = warnings.slow_callbacks.wrapping_add(1);

    let interval = Duration::from_nanos(warn_interval_nanos);
    let is_due = hypha_core::dispatch::is_warning_due(
        warnings.last_warning.map(|last| last.elapsed()),
        interval,
    );
    if is_due {
        log::warn!(target: "hypha", "hypha: attachment {} callback took {} ms, threshold {} ms, {} warnings suppressed",
                id,
                elapsed.as_millis(),
                Duration::from_nanos(warn_nanos).as_millis(),
                warnings.suppressed_warnings);
        warnings.last_warning = Some(Instant::now());
        warnings.suppressed_warnings = 0;
    } else {
        warnings.suppressed_warnings = warnings.suppressed_warnings.wrapping_add(1);
    }
    (result, true)
}

impl Dispatcher for CallbackDispatcher<'_> {
    fn on_event(&mut self, attachment: AttachmentId) -> EventOutcome {
        let batch_size = self.batch_size;
        let warn_nanos = self.warn_nanos;
        let warn_interval = self.warn_interval_nanos;
        let health = self.health;
        let set = self.set;

        if self.is_detached(attachment) {
            return EventOutcome::Drained;
        }
        let detached = self.detached;
        let mut outcome = EventOutcome::Drained;
        let mut answer: Option<HyphaCallbackResult> = None;
        let mut slow = 0_u64;

        {
            let Some(registration) = self.find(attachment) else {
                return EventOutcome::Drained;
            };
            let Registration { id, ref mut target, ref mut warnings, ref mut transport_failing } =
                *registration;
            let (subscriber, callback) = match *target {
                Target::Subscription { ref subscriber, ref mut callback, .. } => {
                    (subscriber, callback)
                }
                Target::LatchReplay => {
                    // An endpoint came or went: replay latched topics to a new subscriber,
                    // and refresh the streams of the watched topics.
                    self.node.transport.replay_latched();
                    self.node.topic_streams.graph_changed();
                    return EventOutcome::Drained;
                }
                // These are time-driven. A trigger wake only wakes the turn.
                Target::SimClock
                | Target::Timer { .. }
                | Target::Triggered(_)
                | Target::Driver(_)
                | Target::TriggerWake => return EventOutcome::Drained,
            };

            let mut turn = BatchTurn::new(batch_size);
            loop {
                // The one validated receive: a refused frame is counted inside it and never
                // reaches the callback.
                let mut received_nanos = 0;
                let received = subscriber
                    .receive_validated_with(&mut received_nanos, |_| ())
                    .map(|received| received.map(|(sample, ())| sample));
                let failed = matches!(received, Err(ReceiveFailure::Transport(_)));
                if let Err(ReceiveFailure::Transport(status)) = received
                    && hypha_core::dispatch::transport_failure_starts(*transport_failing, failed)
                {
                    log::error!(target: "hypha", "hypha: attachment {id} lost its subscriber transport: status {status:?}; \
                                 its samples are not delivered until the transport recovers");
                }
                *transport_failing = failed;
                let (step, stop_early) = match received {
                    Ok(Some(sample)) => {
                        let (result, was_slow) =
                            timed(warnings, id, warn_nanos, warn_interval, || {
                                invoke_data(health, callback, &sample, received_nanos, set)
                            });
                        if was_slow {
                            slow = slow.saturating_add(1);
                        }
                        let stop_early = result.action != HYPHA_ACTION_CONTINUE;
                        if stop_early {
                            answer = Some(result);
                        }
                        (BatchStep::Accepted, stop_early)
                    }
                    // A refused frame was taken from the queue, so valid traffic behind it
                    // still reaches its callback.
                    Err(ReceiveFailure::Refused(_)) => (BatchStep::Refused, false),
                    // A transport failure took no sample, so repeating it makes no progress.
                    Ok(None) | Err(ReceiveFailure::Transport(_)) => (BatchStep::Exhausted, false),
                };
                let should_continue = turn.record(step);
                // The callback may have detached its own registration: no further sample.
                let detached_now = is_detached_in(detached, attachment);
                if stop_early || !should_continue || detached_now {
                    break;
                }
            }
            if turn.is_full() {
                outcome = EventOutcome::MoreWork;
            }
        }

        self.count_slow(slow);
        let Some(result) = answer else {
            return outcome;
        };
        match self.act(attachment, result) {
            Progression::Continue => outcome,
            Progression::Stop => EventOutcome::Stop,
            Progression::Fail(cause) => EventOutcome::Fail(cause),
        }
    }

    fn on_timers(&mut self) -> Progression {
        if !self.node.is_running() {
            return Progression::Stop;
        }
        // Only a node on simulated time has the `/clock` inputs.
        let sim = self.node.uses_sim_time;
        if sim && let Err(decision) = self.drain_sim_clock() {
            return decision;
        }
        // Snapshot the newest read before the clock is read: the deadlines up to the
        // snapshot are then among those that this turn's timers fire.
        let snapshot = if sim { self.newest_read() } else { None };
        let now = hypha_time::scheduling_now();
        let decision = self.fire_due_timers(now);
        if decision != Progression::Continue {
            return decision;
        }
        self.publish_progress(snapshot, now);
        Progression::Continue
    }

    fn on_tick(&mut self) -> Progression {
        let now_nanos = crate::subscriber::host_now_nanos();
        for registration in &*self.registrations {
            if is_detached_in(self.detached, registration.id) {
                continue;
            }
            if let Target::Subscription { ref subscriber, .. } = registration.target {
                subscriber.check_no_publisher_warning(now_nanos);
            }
        }
        // The node's housekeeping, after the turn and outside every step.
        self.node.topic_streams.housekeep(&self.node.transport, std::time::Instant::now());
        // Every dispatch sweep ends here, so an unbounded run is observed too.
        self.health.record_sweep_end();
        Progression::Continue
    }

    /// A periodic wake of the native set. A polled subscription reads its queue here; a
    /// timer's wake only ends the wait.
    fn on_interval(&mut self, attachment: AttachmentId, _missed: u64) -> Progression {
        let is_polled_subscription = self
            .find(attachment)
            .is_some_and(|registration| matches!(registration.target, Target::Subscription { .. }));
        if is_polled_subscription {
            return match self.on_event(attachment) {
                EventOutcome::Drained | EventOutcome::MoreWork => Progression::Continue,
                EventOutcome::Stop => Progression::Stop,
                EventOutcome::Fail(cause) => Progression::Fail(cause),
            };
        }
        Progression::Continue
    }

    /// The wait may last until the earliest host deadline. Under simulated time the
    /// timers hold no host deadline, and a canceled timer holds none.
    fn next_timed_wait(&mut self) -> Option<std::time::Duration> {
        // What a callback attached or reset takes effect before this wait.
        self.admit_attached();
        self.apply_timer_resets();
        // A replay cycle with deadlines its last slice did not reach goes on at once.
        let sliced = self.registrations.iter().any(|registration| match registration.target {
            Target::Timer { cycle: Some(ref timer), .. } => timer.deferred.is_some(),
            Target::Triggered(ref state) => state.schedule.capped(),
            _ => false,
        });
        if sliced {
            return Some(std::time::Duration::ZERO);
        }
        // The housekeeping's next round, or the rest of one a pass left unfinished.
        let housekeeping = self.node.topic_streams.next_wake(std::time::Instant::now());
        let host_timers = !self.node.on_sim_time();
        let live = self.registrations.iter().filter(|r| !is_detached_in(self.detached, r.id));
        let Some(deadline) = live.filter_map(|r| host_deadline(&r.target, host_timers)).min()
        else {
            return housekeeping;
        };
        let remaining = deadline.saturating_sub(hypha_time::steady_now().nanos());
        let timed = std::time::Duration::from_nanos(u64::try_from(remaining).unwrap_or(0));
        Some(housekeeping.map_or(timed, |soon| soon.min(timed)))
    }
}

/// Start a host timer's periods at the first run turn, not at the attach, and return
/// the deadline it is due at now.
fn anchor_at_first_run(
    timer: &mut hypha_core::timer::Timer,
    run_anchored: &mut bool,
    now: hypha_time::Time,
) -> i64 {
    if !*run_anchored {
        timer.reset(now);
        *run_anchored = true;
    }
    timer.next_due().nanos()
}

/// Observe `timer` at `now`, with the periods this observation skipped: the `missed` of
/// the callback, not the running total.
pub(super) fn observe_with_delta(
    timer: &mut hypha_core::timer::Timer,
    now: hypha_time::Time,
) -> (hypha_core::timer::Observation, u64) {
    let previous_missed = timer.missed_count();
    let observation = timer.observe(now);
    (observation, timer.missed_count().wrapping_sub(previous_missed))
}

/// What the timer of registration `id` is due for at `now`: the replay call of a cycle's
/// grid timer, or one call of a timer on the node's clock. `None` when it has nothing to
/// run, or when `target` is no timer.
///
/// # Errors
/// The status that ends the run: a rewind of simulated time (FSR-37), an unknown cycle,
/// or a deadline outside the range of the clock.
fn timer_due(
    cycles: &[Cycle],
    node: &HyphaNode,
    id: AttachmentId,
    target: &mut Target,
    now: hypha_time::Time,
) -> Result<Option<Due>, HyphaStatus> {
    let Target::Timer { ref mut timer, ref mut cycle, ref mut run_anchored, ref control, .. } =
        *target
    else {
        return Ok(None);
    };
    if timer.is_grid()
        && let Some(owned) = cycle.as_mut()
    {
        return grid_due(cycles, id, timer, owned, now);
    }
    // On simulated time a plain timer starts on the first clock value, not on the
    // placeholder zero a node starts with.
    if !*run_anchored && node.on_sim_time() && !hypha_time::sim_time_has_value() {
        return Ok(None);
    }
    if let Some(control) = control.as_deref() {
        // A reset or a canceled timer that finds time gone back is a rewind, as for any
        // timer: the run ends and the process restarts (FSR-37).
        let followed = control.apply_reset(timer, run_anchored)
            && (!control.is_canceled() || super::TimerControl::follow_while_canceled(timer, now));
        if !followed {
            return Err(refuse_timer_rewind(None, id, timer, now));
        }
        // A canceled timer calls nothing and counts no missed period.
        if control.is_canceled() {
            return Ok(None);
        }
    }
    let scheduled = anchor_at_first_run(timer, run_anchored, now);
    let (hypha_core::timer::Observation { fired, rewound }, missed_delta) =
        observe_with_delta(timer, now);
    if let Some(control) = control.as_deref() {
        control.observed(timer.next_due());
    }
    let owner = cycle.as_ref().map(|owned| find_cycle(cycles, owned.cycle)).transpose()?;
    if rewound {
        return Err(refuse_timer_rewind(owner, id, timer, now));
    }
    if fired == 0 {
        return Ok(None);
    }
    // A cycle's step learns the deadline it runs for.
    if let Some(cycle) = owner {
        let deadline = callback_deadline(scheduled, missed_delta, timer.period().nanos())
            .ok_or(HyphaStatus::InvalidState)?;
        cycle.step_mut()?.scheduled_nanos = deadline;
    }
    Ok(Some(Due::Timer {
        attachment: id,
        cycle: owner.map(|cycle| cycle.id),
        missed: missed_delta,
    }))
}

/// The replay call of the grid timer of a cycle at `now`, `None` when it has nothing to
/// run; the status that ends the run for an unknown cycle or a rewind.
fn grid_due(
    cycles: &[Cycle],
    attachment: AttachmentId,
    timer: &mut hypha_core::timer::Timer,
    owned: &mut CycleTimer,
    now: hypha_time::Time,
) -> Result<Option<Due>, HyphaStatus> {
    let CycleTimer { cycle: id, ref mut upstream, ref deferred } = *owned;
    let cycle = find_cycle(cycles, id)?;
    if timer.awaiting_anchor() {
        // Nothing fires before the first clock value sets the phase.
        if !hypha_time::sim_time_has_value() {
            return Ok(None);
        }
        timer.anchor_grid(now);
    }
    let first_nanos = timer.next_due().nanos();
    let (hypha_core::timer::Observation { fired, rewound }, missed_delta) =
        observe_with_delta(timer, now);
    if rewound {
        return Err(refuse_timer_rewind(Some(cycle), attachment, timer, now));
    }
    // A held cycle runs its backlog, and a sliced one the deadlines its last turn did not
    // reach, on a turn with no new deadline.
    let held = upstream.hold.backlog_first().is_some();
    let sliced = deferred.is_some();
    if fired == 0 && !held && !sliced {
        return Ok(None);
    }
    let periods = replay_periods(fired, missed_delta);
    Ok(Some(Due::Replay(ReplayDue {
        attachment,
        cycle: id,
        first_nanos,
        period_nanos: timer.period().nanos(),
        periods,
    })))
}

pub(super) fn is_detached_in(
    detached: &std::cell::RefCell<Vec<AttachmentId>>,
    attachment: AttachmentId,
) -> bool {
    detached.try_borrow().is_ok_and(|detached| detached.contains(&attachment))
}

/// The monotonic deadline of a registration that runs on the host clock, or `None`.
fn host_deadline(target: &Target, host_timers: bool) -> Option<i64> {
    match *target {
        Target::Timer { ref timer, ref control, .. } if host_timers => {
            let canceled = control.as_deref().is_some_and(super::TimerControl::is_canceled);
            (!canceled).then(|| timer.next_due().nanos())
        }
        Target::Triggered(ref state) if host_timers => state.schedule.next_fallback(),
        _ => None,
    }
}

/// Map a callback's status code back to a status, defaulting to a backend failure.
pub(crate) fn status_from_code(code: i32) -> HyphaStatus {
    // A callback that reports a failure without naming one still failed.
    if code == 0 {
        return HyphaStatus::Backend;
    }
    HyphaStatus::from_i32(code)
}

/// The status that ends the run when the timer of registration `id` saw simulated time
/// go back to `now`. Nothing rewinds in place: the run ends and the process restarts
/// (FSR-37). The line names the cycle `owner` that the timer steps, or the plain timer.
fn refuse_timer_rewind(
    owner: Option<&Cycle>,
    id: AttachmentId,
    timer: &hypha_core::timer::Timer,
    now: hypha_time::Time,
) -> HyphaStatus {
    let subject = owner.map_or_else(
        || timer_subject(id, timer.period().nanos()),
        |cycle| format!("cycle '{}'", cycle.name()),
    );
    crate::replay_restart::refuse_rewind(&subject, now.nanos())
}

/// How a rewind line names a plain timer: by its attachment id and period.
pub(super) fn timer_subject(id: AttachmentId, period_nanos: i64) -> String {
    let period_millis = period_nanos / 1_000_000;
    format!("timer {id} (period {period_millis} ms)")
}

#[cfg(test)]
mod timer_subject_tests {
    use super::timer_subject;

    #[test]
    fn a_rewound_plain_timer_is_named_by_its_attachment_and_period() {
        assert_eq!(timer_subject(7, 1_000_000_000), "timer 7 (period 1000 ms)");
        assert_eq!(timer_subject(2, 2_500_000), "timer 2 (period 2 ms)");
    }
}

#[cfg(test)]
mod host_deadline_tests {
    use std::sync::Arc;

    use hypha_core::timer::Timer;
    use hypha_time::{Duration, Time};

    use super::{Target, host_deadline};
    use crate::waitset::TimerControl;

    #[test]
    fn a_canceled_host_timer_holds_no_deadline_and_costs_no_wake_up() {
        let timer = Timer::new(Duration::from_nanos(100), Time::from_nanos(1_000));
        let control = TimerControl::new(&timer, false);
        let shared = Some(Arc::clone(&control));
        let target = Target::Timer {
            timer,
            callback: None,
            cycle: None,
            run_anchored: true,
            control: shared,
        };
        assert_eq!(host_deadline(&target, true), Some(1_100));
        control.cancel();
        assert_eq!(host_deadline(&target, true), None);
    }
}
