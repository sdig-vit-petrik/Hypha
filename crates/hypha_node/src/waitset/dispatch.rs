//! `WaitSet` dispatch ownership and C ABI implementation.

use super::replay::ReplayDue;
use super::{
    AttachmentId, BatchStep, BatchTurn, CallbackWarnings, Dispatcher, Duration, EventOutcome,
    ExecutionHealth, HYPHA_ACTION_CONTINUE, HYPHA_ACTION_FAIL, HYPHA_ACTION_STOP,
    HyphaCallbackResult, HyphaNode, HyphaStatus, HyphaWaitSet, Instant, Progression,
    ReceiveFailure, Registration, Target, invoke_cycle, invoke_data, invoke_interval,
};
use crate::cycle::{Cycle, CycleId};
use crate::status::Error;

mod timer_due;
use timer_due::{host_deadline, timer_due};

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
    /// The index in `due` of the item that runs now.
    pub(super) due_index: usize,
    /// The replay and triggered steps this run ran; a repeat of the timers that adds none
    /// ends the repeats (`service_edges`).
    pub(super) steps_run: u64,
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
        self.fire_due(now, false)
    }

    /// Fire what is due, as [`Self::fire_due_timers`]; with `services_only`, only the replay
    /// and triggered cycles that have a server or a client, for a repeat of the turn.
    pub(super) fn fire_due(&mut self, now: hypha_time::Time, services_only: bool) -> Progression {
        self.due.clear();
        let (cycles, node) = (self.cycles, self.node);
        // A registration a callback detached is not observed: it neither fires nor fails.
        let live = self.registrations.iter_mut().filter(|r| !is_detached_in(self.detached, r.id));
        for registration in live {
            if services_only && !super::service_edges::has_services(cycles, registration) {
                continue;
            }
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
            self.due_index = index;
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
        if let Some(outcome) = self.on_service_event(attachment) {
            return outcome;
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
                | Target::TriggerWake
                | Target::Service(_) => return EventOutcome::Drained,
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
        if self.node.on_sim_time() {
            let decision = self.repeat_for_local_edges(now);
            if decision != Progression::Continue {
                return decision;
            }
        }
        let decision = self.drive_services();
        if decision != Progression::Continue {
            return decision;
        }
        // `sim` says the node reads `/clock`; a replay cycle also runs on simulated time
        // set through the C ABI, which `on_sim_time` adds. Only then can one be attached.
        if self.node.on_sim_time() {
            let decision = self.drain_replay_inputs();
            if decision != Progression::Continue {
                return decision;
            }
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
        let replay = self.node.on_sim_time();
        for cycle in self.cycles {
            crate::cycle::check_input_publishers(cycle, now_nanos, replay);
        }
        // The node's housekeeping, after the turn and outside every step.
        self.node.topic_streams.housekeep(&self.node.transport, now_nanos);
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
    fn next_deadline(&mut self) -> Option<hypha_time::Time> {
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
            return Some(hypha_time::steady_now());
        }
        let host_timers = !self.node.on_sim_time();
        let live = self.registrations.iter().filter(|r| !is_detached_in(self.detached, r.id));
        let timers = live.filter_map(|r| host_deadline(&r.target, host_timers)).min();
        // The housekeeping's next round, or the rest of one a pass left unfinished: a
        // deadline like a timer's, so the timer that ends the wait ends it for this too.
        let housekeeping =
            self.node.topic_streams.next_deadline(crate::subscriber::host_now_nanos());
        let calls = self.service_deadline();
        timers.into_iter().chain(housekeeping).chain(calls).min().map(hypha_time::Time::from_nanos)
    }
}

pub(super) fn is_detached_in(
    detached: &std::cell::RefCell<Vec<AttachmentId>>,
    attachment: AttachmentId,
) -> bool {
    detached.try_borrow().is_ok_and(|detached| detached.contains(&attachment))
}

/// Map a callback's status code back to a status, defaulting to a backend failure.
pub(crate) fn status_from_code(code: i32) -> HyphaStatus {
    // A callback that reports a failure without naming one still failed.
    if code == 0 {
        return HyphaStatus::Backend;
    }
    HyphaStatus::from_i32(code)
}
