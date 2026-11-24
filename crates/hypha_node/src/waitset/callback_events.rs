//! Plain callbacks in the replay lockstep (#72).
//!
//! A `WaitSet` of a node that follows `/clock` runs its plain subscriptions and plain
//! timers as events: each sample at its framework send time, each timer period at its
//! deadline, in time order, under the step-time guard, so `now()` and every stamp the
//! callback sends read the event's time. An event waits for the upstream nodes as a
//! triggered step does, and the `WaitSet` reports the events it ran as its progress.
//! The rules are in `docs/execution-model.md` section 11.2.

use std::time::Instant;

use hypha_runtime::upstream_hold::Limit;

use super::dispatch::{CallbackDispatcher, fail_with, observe_with_delta, timed};
use super::replay::{MAX_STEPS_PER_TURN, STEP_SLICE_BUDGET};
use super::upstream::UpstreamHold;
use super::{AttachmentId, Progression, Registration, Target, invoke_data, invoke_interval};
use crate::status::Error;

/// A received sample that waits for its turn: the one sample a subscription borrows.
pub(crate) struct Head {
    sample: hypha_iceoryx2::Iox2Sample,
    received_nanos: u64,
    stamp: i64,
}

/// The replay state of the plain callbacks of one `WaitSet`, kept across its runs.
pub(crate) struct CallbackReplay {
    /// True while this turn's plain callbacks take part; set at the start of each turn.
    pub(super) participating: bool,
    upstream: UpstreamHold,
    limit: Limit,
    /// The time of the event that ran last, to refuse one that comes out of order.
    last_event: Option<i64>,
    /// The clock of the last turn, to see a rewind.
    last_now: Option<i64>,
    /// The earliest event time left after the last turn, for the progress report.
    pub(super) frontier: Option<i64>,
    /// True when the last turn stopped at its event budget with due events left.
    pub(super) capped: bool,
    /// The publisher nodes of the plain subscriptions at the last look, sorted.
    publishers_seen: Vec<u128>,
    /// The publisher nodes already named as taking no part in the replay.
    warned: Vec<u128>,
}

impl Default for CallbackReplay {
    fn default() -> Self {
        Self {
            participating: false,
            upstream: UpstreamHold::default(),
            limit: Limit { at: None, blocking: None },
            last_event: None,
            last_now: None,
            frontier: None,
            capped: false,
            publishers_seen: Vec::new(),
            warned: Vec::new(),
        }
    }
}

/// One event of a turn. Events run in the order of this tuple: time, then a timer before
/// a sample (a timer at `t` sees the inputs before `t`, as a cycle step does), then
/// registration order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Event {
    time: i64,
    kind: Kind,
    index: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Timer,
    Sample,
}

/// A plain subscription or a plain timer: a registration whose callbacks are events.
pub(super) const fn is_plain(target: &Target) -> bool {
    matches!(target, Target::Subscription { .. } | Target::Timer { cycle: None, .. })
}

impl CallbackDispatcher<'_> {
    /// Decide, for this turn, whether the plain callbacks take part in the replay: the
    /// node follows `/clock` in the lockstep (not `HYPHA_REPLAY_LOCKSTEP=0`), is no tool
    /// and drives no clock, and the `WaitSet` has one. An open-loop node's callbacks run as
    /// their samples arrive, as on host time; its timers still follow `/clock`.
    pub(super) fn decide_participation(&mut self) -> bool {
        let node = self.node;
        let follows_clock = node.uses_sim_time
            && !node.open_loop
            && !node.drives_clock.load(std::sync::atomic::Ordering::Acquire)
            && !hypha_core::port::is_cli_node_name(&node.name);
        let detached = self.detached;
        let participating = follows_clock
            && self.registrations.iter().any(|registration| {
                is_plain(&registration.target)
                    && !super::dispatch::is_detached_in(detached, registration.id)
            });
        self.callbacks.participating = participating;
        participating
    }

    /// Run the plain callbacks' events that are due at `now`, in time order.
    pub(super) fn run_callback_events(&mut self, now: i64) -> Progression {
        match self.callback_turn(now) {
            Ok(decision) => decision,
            Err((attachment, status)) => fail_with(&mut self.failure, attachment, status),
        }
    }

    fn callback_turn(&mut self, now: i64) -> Result<Progression, (AttachmentId, Error)> {
        self.callbacks.capped = false;
        if !hypha_time::sim_time_has_value() {
            // Nothing runs before the first clock value, as for a cycle.
            self.callbacks.frontier = None;
            return Ok(Progression::Continue);
        }
        if self.callbacks.last_now.is_some_and(|last| now < last) {
            let status = crate::replay_restart::refuse_rewind("a plain callback", now);
            return Err((0, Error::from_status(status)));
        }
        self.callbacks.last_now = Some(now);
        self.name_unpaced_publishers();
        self.settle_upstream().map_err(|status| (0, status))?;
        let started = Instant::now();
        let mut ran = 0_u64;
        let mut decision = Progression::Continue;
        loop {
            // A timer a callback attached, reset or canceled joins the order at once.
            self.admit_attached();
            self.anchor_and_reset(now)?;
            self.fill_heads()?;
            let Some(event) = self.earliest_event() else {
                break;
            };
            if !self.admitted(event, now) {
                break;
            }
            if ran >= MAX_STEPS_PER_TURN || started.elapsed() >= STEP_SLICE_BUDGET {
                self.callbacks.capped = true;
                break;
            }
            ran = ran.saturating_add(1);
            decision = self.run_event(event)?;
            if decision != Progression::Continue {
                break;
            }
        }
        self.callbacks.frontier = self.frontier();
        Ok(decision)
    }

    /// Warn once for each publisher of a plain subscription that takes no part in the
    /// replay: it neither reports progress nor publishes `/clock`. Nothing waits for it,
    /// and its samples run by their stamps alone. Looked at when the set of publishers
    /// changes, after the first clock value, so the player's `/clock` publisher exists.
    fn name_unpaced_publishers(&mut self) {
        let mut topics: Vec<(u128, &str)> = Vec::new();
        let mut ids = Vec::new();
        for registration in self.registrations.iter() {
            if let Target::Subscription { ref subscriber, .. } = registration.target {
                ids.clear();
                subscriber.port.publisher_node_ids(&mut ids);
                topics.extend(ids.iter().map(|&id| (id, subscriber.topic.as_str())));
            }
        }
        let mut seen: Vec<u128> = topics.iter().map(|&(id, _)| id).collect();
        seen.sort_unstable();
        seen.dedup();
        if seen == self.callbacks.publishers_seen {
            return;
        }
        self.callbacks.publishers_seen.clone_from(&seen);
        let mut paced = Vec::new();
        for input in [&self.node.progress_subscriber, &self.node.sim_clock_subscriber] {
            if let Ok(guard) = input.try_lock()
                && let Some(subscriber) = guard.as_deref()
            {
                subscriber.port.publisher_node_ids(&mut paced);
            }
        }
        let unpaced: Vec<u128> = seen
            .into_iter()
            .filter(|id| !paced.contains(id) && !self.callbacks.warned.contains(id))
            .collect();
        let Ok(names) = hypha_iceoryx2::node_names(&unpaced) else {
            return;
        };
        for (id, name) in names {
            if name == self.node.name {
                continue;
            }
            self.callbacks.warned.push(id);
            let on: Vec<&str> =
                topics.iter().filter(|&&(of, _)| of == id).map(|&(_, topic)| topic).collect();
            log::warn!(target: "hypha", "hypha: '{name}' publishes on {on:?} but takes no part in the replay: '{}' does not wait for it and runs its samples by their stamps alone", self.node.name);
        }
    }

    /// Read the upstream limit of every plain subscription of this `WaitSet`.
    fn settle_upstream(&mut self) -> Result<(), Error> {
        let detached = self.detached;
        let subscribers =
            self.registrations.iter().filter_map(|registration| match registration.target {
                Target::Subscription { ref subscriber, .. }
                    if !super::dispatch::is_detached_in(detached, registration.id) =>
                {
                    Some(&**subscriber)
                }
                _ => None,
            });
        self.callbacks.limit =
            self.callbacks.upstream.settle_subscribers(subscribers, self.node)?;
        Ok(())
    }

    /// Anchor a plain timer that has no start yet on the clock, and apply the resets
    /// that callbacks made. A reset earlier than the timer has seen is a rewind.
    fn anchor_and_reset(&mut self, now: i64) -> Result<(), (AttachmentId, Error)> {
        for registration in self.registrations.iter_mut() {
            let Target::Timer {
                ref mut timer, cycle: None, ref mut run_anchored, ref control, ..
            } = registration.target
            else {
                continue;
            };
            if let Some(control) = control.as_deref()
                && !control.apply_reset(timer, run_anchored)
            {
                let subject =
                    super::dispatch::timer_subject(registration.id, timer.period().nanos());
                let status = crate::replay_restart::refuse_rewind(&subject, now);
                return Err((registration.id, Error::from_status(status)));
            }
            if !*run_anchored {
                timer.reset(hypha_time::Time::from_nanos(now));
                *run_anchored = true;
            }
            if let Some(control) = control.as_deref() {
                control.observed(timer.next_due());
            }
        }
        Ok(())
    }

    /// Receive the next sample of every plain subscription that holds none. A sample
    /// stamped before the event that ran last fails the run: under simulated time the
    /// stamps a `WaitSet` reads must ascend.
    fn fill_heads(&mut self) -> Result<(), (AttachmentId, Error)> {
        let detached = self.detached;
        let last_event = self.callbacks.last_event;
        for registration in self.registrations.iter_mut() {
            if super::dispatch::is_detached_in(detached, registration.id) {
                continue;
            }
            let Target::Subscription { ref subscriber, ref mut head, .. } = registration.target
            else {
                continue;
            };
            while head.is_none() {
                // The validation of a receive counts a gap the queue left before the sample.
                let lost_before = subscriber.missing_sequences();
                let mut received_nanos = 0;
                let received = subscriber
                    .receive_validated_with(&mut received_nanos, |_| ())
                    .map(|received| received.map(|(sample, ())| sample));
                let sample = match received {
                    Ok(Some(sample)) => sample,
                    // A refused frame is counted and taken; the next one may be valid.
                    Err(super::ReceiveFailure::Refused(_)) => continue,
                    Ok(None) | Err(super::ReceiveFailure::Transport(_)) => break,
                };
                let lost = subscriber.missing_sequences().saturating_sub(lost_before);
                if lost > 0 {
                    // A replay is faithful only when no sample it should read was lost.
                    log::error!(target: "hypha", "hypha: replay not faithful: {lost} samples lost on {}", subscriber.topic);
                    return Err((registration.id, Error::SequenceGap));
                }
                let sent = sample.header().metadata.framework_sent_nanos;
                let stamp =
                    i64::try_from(sent).map_err(|_| (registration.id, Error::CorruptPayload))?;
                if let Some(before) = last_event.filter(|&before| stamp < before) {
                    log::error!(target: "hypha", "hypha: '{}' delivered a sample stamped {stamp} ns after an event at {before} ns: under simulated time the samples a node reads must ascend; a publisher that takes no part in the replay, or a thread of a node that publishes outside its callbacks, can send it late", subscriber.topic);
                    return Err((registration.id, Error::SequenceGap));
                }
                *head = Some(Head { sample, received_nanos, stamp });
            }
        }
        Ok(())
    }

    /// The earliest event of this turn: a held sample, or the next call of a plain
    /// timer that is neither canceled nor waiting for its start.
    fn earliest_event(&self) -> Option<Event> {
        self.registrations
            .iter()
            .enumerate()
            .filter(|(_, registration)| !self.is_detached(registration.id))
            .filter_map(|(index, registration)| event_of(index, registration))
            .min()
    }

    /// Whether `event` may run now. Both kinds need the clock to have reached them. A
    /// timer at `t` needs every upstream at `t - 1` or later, as a cycle step does; a
    /// sample at `t` needs every upstream at `t`, so every sample stamped `t` is present
    /// and samples that share a stamp run in the same order on every replay.
    fn admitted(&self, event: Event, now: i64) -> bool {
        if event.time > now {
            return false;
        }
        match (event.kind, self.callbacks.limit.at) {
            (_, None) => true,
            (Kind::Timer, Some(at)) => event.time <= at,
            (Kind::Sample, Some(at)) => event.time < at,
        }
    }

    /// Run one event under the step-time guard: framework time on this thread is the
    /// event's time while its callback runs.
    fn run_event(&mut self, event: Event) -> Result<Progression, (AttachmentId, Error)> {
        let warn_nanos = self.warn_nanos;
        let warn_interval = self.warn_interval_nanos;
        let health = self.health;
        let set = self.set;
        let Some(registration) = self.registrations.get_mut(event.index) else {
            return Err((0, Error::InvalidState));
        };
        let Registration { id, ref mut target, ref mut warnings, .. } = *registration;
        let _event_time = hypha_time::StepTimeGuard::hold(event.time);
        let (result, slow) = match *target {
            Target::Subscription { ref mut callback, ref mut head, .. } => {
                let Some(held) = head.take() else {
                    return Err((id, Error::InvalidState));
                };
                timed(warnings, id, warn_nanos, warn_interval, || {
                    invoke_data(health, callback, &held.sample, held.received_nanos, set)
                })
            }
            Target::Timer { ref mut timer, ref mut callback, cycle: None, ref control, .. } => {
                let due = hypha_time::Time::from_nanos(event.time);
                let (observation, _missed) = observe_with_delta(timer, due);
                if observation.fired == 0 {
                    return Err((id, Error::InvalidState));
                }
                if let Some(control) = control.as_deref() {
                    control.observed(timer.next_due());
                }
                // One call per period: a replayed timer misses none.
                timed(warnings, id, warn_nanos, warn_interval, || {
                    invoke_interval(health, callback, 0)
                })
            }
            _ => return Err((id, Error::InvalidState)),
        };
        self.callbacks.last_event = Some(event.time);
        self.count_slow(u64::from(slow));
        Ok(self.act(id, result))
    }

    /// The earliest time an event of this `WaitSet` can still run at: a held sample, a
    /// pending timer call, or the upstream limit, past which samples may still come.
    fn frontier(&self) -> Option<i64> {
        let pending = self.earliest_event().map(|event| event.time);
        [pending, self.callbacks.limit.at].into_iter().flatten().min()
    }
}

/// The event of one registration this turn, if it has one.
fn event_of(index: usize, registration: &Registration) -> Option<Event> {
    match registration.target {
        Target::Subscription { head: Some(ref held), .. } => {
            Some(Event { time: held.stamp, kind: Kind::Sample, index })
        }
        Target::Timer { ref timer, cycle: None, run_anchored: true, ref control, .. } => {
            let canceled = control.as_deref().is_some_and(super::TimerControl::is_canceled);
            (!canceled).then(|| Event { time: timer.next_due().nanos(), kind: Kind::Timer, index })
        }
        _ => None,
    }
}
