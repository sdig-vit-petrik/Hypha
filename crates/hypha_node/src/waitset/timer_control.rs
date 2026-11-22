//! Cancel, reset and read a plain timer, also from a callback of the running `WaitSet`.
//!
//! The dispatcher holds a timer's registration for the length of a run, so each plain
//! timer (`HYPHA_ATTACH_TIMER`) shares a [`TimerControl`] with the `WaitSet`'s registry
//! of live registrations, which no run borrows. A cancel or a reset changes it at the
//! call. The dispatcher applies a reset before its next native wait and before it next
//! observes the timer, and a call of the timer that the current turn found due but has
//! not made yet is held back once a callback cancels or resets the timer.
//!
//! A cycle's own timer has no control: a cycle runs at its rate.

use std::sync::{Arc, Mutex};

use hypha_core::timer::Timer;
use hypha_time::{Duration, Time};

use super::{Error, HyphaAttachmentId, HyphaWaitSet};

/// The state of a plain timer registration, as `hypha_waitset_timer_state` reports it.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HyphaTimerState {
    /// True after a cancel, until the next reset.
    pub canceled: bool,
    /// The attached period, in nanoseconds.
    pub period_nanos: i64,
    /// Nanoseconds until the next call, on the timer's own clock: simulated framework
    /// time under simulated time, else the host monotonic scheduling clock. Zero when
    /// the call is due now, `INT64_MAX` while canceled. Before the first run of a timer
    /// on host time it counts from the attach; the run starts the period anew.
    pub until_due_nanos: i64,
}

/// What a cancel, a reset and a state read share with the dispatcher for one plain timer.
pub(crate) struct TimerControl {
    period: Duration,
    /// Whether the timer runs on simulated time; see [`clock_now`].
    sim_time: bool,
    state: Mutex<ControlState>,
}

#[derive(Debug, Clone, Copy)]
struct ControlState {
    canceled: bool,
    /// The next call on the timer's clock, as of its last observation or reset.
    next_due: Time,
    /// A reset the dispatcher has not applied yet: the time its period restarts from.
    pending_reset: Option<Time>,
}

impl TimerControl {
    /// The control of `timer`, a plain timer on simulated time when `sim_time` is true.
    pub(crate) fn new(timer: &Timer, sim_time: bool) -> Arc<Self> {
        let state =
            ControlState { canceled: false, next_due: timer.next_due(), pending_reset: None };
        Arc::new(Self { period: timer.period(), sim_time, state: Mutex::new(state) })
    }

    /// Restart the period of `timer`, which the dispatcher holds, from the time of a
    /// reset not yet applied. The reset replaces the anchor of the first run.
    ///
    /// Returns false, and keeps the reset pending, for a reset earlier than the timer
    /// has seen: simulated time went back, a rewind the caller ends the run for. Nothing
    /// rewinds in place (FSR-37).
    pub(crate) fn apply_reset(&self, timer: &mut Timer, run_anchored: &mut bool) -> bool {
        let mut state = crate::locked(&self.state);
        let Some(at) = state.pending_reset else {
            return true;
        };
        if at < timer.latest_seen() {
            return false;
        }
        state.pending_reset = None;
        drop(state);
        timer.reset(at);
        *run_anchored = true;
        true
    }

    /// Follow the clock of a canceled `timer` to `now` without a call, so a rewind is
    /// still seen. Returns false when `now` is earlier than the timer has seen.
    pub(crate) fn follow_while_canceled(timer: &mut Timer, now: Time) -> bool {
        if now < timer.latest_seen() {
            return false;
        }
        timer.reset(now);
        true
    }

    /// True after a cancel, until the next reset.
    pub(crate) fn is_canceled(&self) -> bool {
        crate::locked(&self.state).canceled
    }

    /// True when a call that the current turn found due is held back: a callback
    /// canceled or reset the timer since.
    pub(crate) fn holds_back_due_call(&self) -> bool {
        let state = crate::locked(&self.state);
        state.canceled || state.pending_reset.is_some()
    }

    /// Record the next call after the dispatcher observed the timer.
    pub(crate) fn observed(&self, next_due: Time) {
        crate::locked(&self.state).next_due = next_due;
    }

    pub(super) fn cancel(&self) {
        crate::locked(&self.state).canceled = true;
    }

    /// Restart the period from now on the timer's clock and clear a cancel.
    fn reset(&self) {
        let now = clock_now(self.sim_time);
        let next_due = Timer::new(self.period, now).next_due();
        let mut state = crate::locked(&self.state);
        state.canceled = false;
        state.pending_reset = Some(now);
        state.next_due = next_due;
    }

    fn snapshot(&self) -> HyphaTimerState {
        let state = *crate::locked(&self.state);
        let until_due_nanos = if state.canceled {
            i64::MAX
        } else {
            let now = clock_now(self.sim_time);
            state.next_due.nanos().saturating_sub(now.nanos()).max(0)
        };
        HyphaTimerState {
            canceled: state.canceled,
            period_nanos: self.period.nanos(),
            until_due_nanos,
        }
    }
}

/// The clock of a plain timer: framework time for a timer on simulated time, else the
/// host's monotonic scheduling clock.
pub(crate) fn clock_now(sim_time: bool) -> Time {
    if sim_time { hypha_time::now() } else { hypha_time::scheduling_now() }
}

/// Cancel the plain timer `timer` of `set`: it calls nothing until a reset, counts no
/// missed period and costs no wake-up. A cancel from a callback also holds back a call
/// of the timer still due later in the same turn. Idempotent.
///
/// # Errors
/// `InvalidArgument` for an id that names no plain timer of `set` (a cycle's timer, a
/// subscription or an unknown id); `InvalidState` from another thread during a run.
pub fn waitset_timer_cancel(set: &HyphaWaitSet, timer: HyphaAttachmentId) -> Result<(), Error> {
    control_of(set, timer).map(|control| control.cancel())
}

/// Reset the plain timer `timer` of `set`: its next call comes one period after now on
/// its clock, and a cancel ends. The periods before the reset count as missed in no
/// call. From a callback it takes effect before the next wait and holds back a call of
/// the timer still due later in the same turn.
///
/// # Errors
/// As [`waitset_timer_cancel`].
pub fn waitset_timer_reset(set: &HyphaWaitSet, timer: HyphaAttachmentId) -> Result<(), Error> {
    control_of(set, timer).map(|control| control.reset())
}

/// The state of the plain timer `timer` of `set`, read at the call.
///
/// # Errors
/// As [`waitset_timer_cancel`].
pub fn waitset_timer_state(
    set: &HyphaWaitSet,
    timer: HyphaAttachmentId,
) -> Result<HyphaTimerState, Error> {
    control_of(set, timer).map(|control| control.snapshot())
}

/// The control of the plain timer `timer`, from the registry a run does not borrow.
fn control_of(set: &HyphaWaitSet, timer: HyphaAttachmentId) -> Result<Arc<TimerControl>, Error> {
    if set.shared.runs_on_another_thread() {
        return Err(Error::InvalidState);
    }
    let live = set.live.try_borrow().map_err(|_| Error::InvalidState)?;
    live.iter()
        .find(|registration| registration.id == timer)
        .and_then(|registration| registration.timer.clone())
        .ok_or(Error::InvalidArgument)
}

#[cfg(test)]
mod tests {
    use super::{Duration, Time, Timer, TimerControl};

    const PERIOD: i64 = 100;

    fn control_at(nanos: i64) -> (Timer, std::sync::Arc<TimerControl>) {
        let timer = Timer::new(Duration::from_nanos(PERIOD), Time::from_nanos(nanos));
        let control = TimerControl::new(&timer, true);
        (timer, control)
    }

    #[test]
    fn a_pending_reset_restarts_the_held_timer_from_its_time_and_anchors_it() {
        let (mut timer, control) = control_at(0);
        crate::locked(&control.state).pending_reset = Some(Time::from_nanos(250));
        let mut run_anchored = false;
        control.apply_reset(&mut timer, &mut run_anchored);
        assert_eq!(timer.next_due(), Time::from_nanos(350));
        assert!(run_anchored, "a reset replaces the anchor of the first run");
        assert!(!control.holds_back_due_call(), "an applied reset holds back nothing");
        // Applied once: the timer goes on from its own cadence.
        let observation = timer.observe(Time::from_nanos(360));
        assert_eq!(observation.fired, 1);
        control.apply_reset(&mut timer, &mut run_anchored);
        assert_eq!(timer.next_due(), Time::from_nanos(450));
    }

    #[test]
    fn a_cancel_holds_back_a_due_call_and_a_timer_with_no_reset_is_unchanged() {
        let (mut timer, control) = control_at(0);
        control.cancel();
        assert!(control.is_canceled());
        assert!(control.holds_back_due_call());
        let mut run_anchored = false;
        control.apply_reset(&mut timer, &mut run_anchored);
        assert_eq!(timer.next_due(), Time::from_nanos(PERIOD));
        assert!(!run_anchored);
        assert_eq!(control.snapshot().until_due_nanos, i64::MAX);
    }
}
