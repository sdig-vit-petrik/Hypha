//! The schedule of a data-triggered cycle, as pure rules.
//!
//! A triggered cycle runs one step per trigger sample, oldest first, and one fallback
//! step for each silence limit without a trigger. The node passes the clocks and the
//! pending trigger stamps here as plain values.

use crate::upstream_hold::{HoldError, Limit};

/// One step of a triggered cycle, at a framework time in nanoseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerEvent {
    /// A step for the trigger sample sent at this time.
    Trigger(i64),
    /// A step without a trigger sample, after a silence limit without one.
    Fallback(i64),
    /// Host time: the oldest pending trigger, sent at this time, runs no step: it was
    /// stamped before a step that already ran, or is older than the silence limit.
    Late(i64),
}

/// The clock a triggered cycle runs on.
#[derive(Debug, Clone, Copy)]
pub enum TriggerClock<'a> {
    /// Host time: triggers run at once; a fallback at `F` needs `steady >= F`.
    ///
    /// The schedule runs on `steady`, so a system clock step never moves a fallback; a
    /// trigger stamp's age is counted on `wall`.
    Host {
        /// The wall clock now, in nanoseconds since the Unix epoch. Ages stamps.
        wall: i64,
        /// The monotonic reading now, in nanoseconds. Times the fallbacks.
        steady: i64,
    },
    /// Simulated time: an event at `t` needs `t <= now` and `t <= limit.at`.
    Sim {
        /// The node's simulated clock, in nanoseconds.
        now: i64,
        /// What the upstream nodes allow.
        limit: &'a Limit,
    },
}

/// What a turn of a triggered cycle does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TriggerAdmit {
    /// Run the events written to `out`, possibly none.
    Run,
    /// Under simulated time, a pending trigger is stamped before the event that runs
    /// before it. Nothing runs.
    OutOfOrder {
        /// The trigger stamp, in nanoseconds.
        stamp: i64,
        /// The time of the event before it, in nanoseconds.
        before: i64,
    },
}

/// The fallback anchor and stall state of one triggered cycle.
#[derive(Debug)]
pub struct TriggerSchedule {
    silence: i64,
    /// What the next fallback counts from: the time of the last event run.
    last: Option<i64>,
    /// The time of the last event run, which no later event precedes.
    ran: Option<i64>,
    /// Whether the last admission stopped at [`MAX_EVENTS_PER_ADMIT`] with events left.
    capped: bool,
}

/// The most events one simulated-time admission writes; a long clock jump runs over
/// several calls.
pub const MAX_EVENTS_PER_ADMIT: usize = 1024;

impl TriggerSchedule {
    /// A schedule with a silence limit of `silence_nanos`, or `None` unless it is positive.
    #[must_use]
    pub fn new(silence_nanos: i64) -> Option<Self> {
        (silence_nanos > 0).then_some(Self {
            silence: silence_nanos,
            last: None,
            ran: None,
            capped: false,
        })
    }

    /// Write to `out` the events that run now, in order: a prefix of `pending` with the
    /// fallbacks due before each, which a trigger at the fallback time preempts.
    ///
    /// Host time coalesces the fallbacks due before the next trigger into one; a trigger
    /// stamped before the last event, or older than the limit, is [`TriggerEvent::Late`].
    /// Simulated time runs each fallback once no upstream can still send a trigger at its
    /// time; a trigger stamped before the last event is `OutOfOrder`.
    ///
    /// # Errors
    /// [`HoldError::Overflow`] for an unrepresentable time, `NoBlocker` for a bare timeout.
    pub fn admit(
        &mut self,
        pending: &[i64],
        clock: TriggerClock<'_>,
        out: &mut Vec<TriggerEvent>,
    ) -> Result<TriggerAdmit, HoldError> {
        out.clear();
        self.capped = false;
        let (now, limit) = match clock {
            TriggerClock::Host { steady, .. } => (steady, None),
            TriggerClock::Sim { now, limit } => (now, limit.at),
        };
        let host = matches!(clock, TriggerClock::Host { .. });
        if !host {
            let mut before = self.ran;
            for &stamp in pending {
                if let Some(before) = before.filter(|before| stamp < *before) {
                    return Ok(TriggerAdmit::OutOfOrder { stamp, before });
                }
                before = Some(stamp);
            }
        }
        let mut last = *self.last.get_or_insert(now);
        let mut next: PendingStamps<'_> = pending.iter().copied().peekable();
        loop {
            let full = !host && out.len() >= MAX_EVENTS_PER_ADMIT;
            if full {
                self.capped = true;
                break;
            }
            if host {
                while let Some(&stamp) = next.peek() {
                    let time = trigger_time(stamp, clock)?;
                    let before_ran = self.ran.is_some_and(|ran| time < ran);
                    let too_old = self.too_old(stamp, clock);
                    let is_late = before_ran || too_old;
                    if !is_late {
                        break;
                    }
                    next.next();
                    out.push(TriggerEvent::Late(stamp));
                }
            }
            let fallback = last.checked_add(self.silence).ok_or(HoldError::Overflow)?;
            let head = next_trigger_time(&mut next, clock)?;
            let trigger = head.filter(|trigger| *trigger <= fallback);
            if let Some(time) = trigger {
                let clock_allows = host || time <= now;
                if !clock_allows {
                    break;
                }
                if limit.is_some_and(|at| time > at) {
                    break;
                }
                next.next();
                out.push(TriggerEvent::Trigger(time));
                self.ran = Some(time);
                last = time;
                continue;
            }
            if fallback > now {
                break;
            }
            // An upstream at `at - 1` may still send a trigger stamped `at - 1`.
            let Some(latest) = fallback_bound(now, head, limit) else {
                return Err(HoldError::Overflow);
            };
            if fallback > latest {
                break;
            }
            let time = if host { latest_multiple(last, self.silence, latest)? } else { fallback };
            out.push(TriggerEvent::Fallback(time));
            self.ran = Some(time);
            last = time;
        }
        self.last = Some(last);
        Ok(TriggerAdmit::Run)
    }

    /// Whether the last admission stopped at [`MAX_EVENTS_PER_ADMIT`] with events due.
    #[must_use]
    pub fn capped(&self) -> bool {
        self.capped
    }

    /// The next fallback deadline on the schedule's clock, `None` before the first evaluation.
    #[must_use]
    pub fn next_fallback(&self) -> Option<i64> {
        self.last.and_then(|last| last.checked_add(self.silence))
    }

    /// Whether a host trigger stamped at `stamp` is older than the silence limit.
    fn too_old(&self, stamp: i64, clock: TriggerClock<'_>) -> bool {
        match clock {
            TriggerClock::Host { wall, .. } => {
                wall.checked_sub(stamp).is_some_and(|age| age > self.silence)
            }
            TriggerClock::Sim { .. } => false,
        }
    }

    /// The clock up to which this cycle may report, exclusive: the lowest of its first
    /// pending trigger, its next fallback and the upstream limit.
    #[must_use]
    pub fn frontier(&self, pending: &[i64], limit: &Limit) -> Option<i64> {
        let fallback = self.next_fallback();
        [pending.first().copied(), fallback, limit.at].into_iter().flatten().min()
    }
}

type PendingStamps<'a> = std::iter::Peekable<std::iter::Copied<std::slice::Iter<'a, i64>>>;

/// The schedule time of a pending trigger stamp on `clock`.
///
/// Under host time the stamp's wall age is taken back from the monotonic reading; a
/// stamp from the future counts as sent now.
fn trigger_time(stamp: i64, clock: TriggerClock<'_>) -> Result<i64, HoldError> {
    let TriggerClock::Host { wall, steady } = clock else {
        return Ok(stamp);
    };
    let age = wall.checked_sub(stamp).ok_or(HoldError::Overflow)?.max(0);
    steady.checked_sub(age).ok_or(HoldError::Overflow)
}

fn next_trigger_time(
    next: &mut PendingStamps<'_>,
    clock: TriggerClock<'_>,
) -> Result<Option<i64>, HoldError> {
    let Some(&stamp) = next.peek() else {
        return Ok(None);
    };
    trigger_time(stamp, clock).map(Some)
}

/// The latest time a fallback may run at: `now`, before the next trigger, and below
/// the upstream limit.
fn fallback_bound(now: i64, trigger: Option<i64>, limit: Option<i64>) -> Option<i64> {
    let before_trigger = trigger.map_or(Some(now), |trigger| trigger.checked_sub(1))?;
    let below_limit = limit.map_or(Some(now), |at| at.checked_sub(1))?;
    Some(now.min(before_trigger).min(below_limit))
}

/// The latest `last + k * silence`, `k >= 1`, at or before `bound`.
fn latest_multiple(last: i64, silence: i64, bound: i64) -> Result<i64, HoldError> {
    let span = i128::from(bound).checked_sub(i128::from(last)).ok_or(HoldError::Overflow)?;
    let periods = span.checked_div(i128::from(silence)).ok_or(HoldError::Overflow)?;
    let advance = periods.checked_mul(i128::from(silence)).ok_or(HoldError::Overflow)?;
    let time = i128::from(last).checked_add(advance).ok_or(HoldError::Overflow)?;
    i64::try_from(time).map_err(|_| HoldError::Overflow)
}

#[cfg(test)]
#[allow(clippy::arithmetic_side_effects, clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    const S: i64 = 100;

    /// A host clock whose wall and monotonic readings are both `now`.
    fn host(now: i64) -> TriggerClock<'static> {
        TriggerClock::Host { wall: now, steady: now }
    }

    /// A host clock whose wall and monotonic readings differ, as after a wall-clock step.
    fn host_at(wall: i64, steady: i64) -> TriggerClock<'static> {
        TriggerClock::Host { wall, steady }
    }

    fn run(s: &mut TriggerSchedule, pending: &[i64], clock: TriggerClock<'_>) -> Vec<TriggerEvent> {
        let mut out = Vec::new();
        assert_eq!(s.admit(pending, clock, &mut out), Ok(TriggerAdmit::Run));
        out
    }

    #[test]
    fn new_refuses_a_silence_that_is_not_positive() {
        assert!(TriggerSchedule::new(0).is_none());
        assert!(TriggerSchedule::new(-1).is_none());
    }

    #[test]
    fn the_first_call_anchors_the_fallback() {
        let mut s = TriggerSchedule::new(S).unwrap();
        assert_eq!(run(&mut s, &[], host(1_000)), vec![]);
        assert_eq!(run(&mut s, &[], host(1_099)), vec![]);
        assert_eq!(run(&mut s, &[], host(1_100)), vec![TriggerEvent::Fallback(1_100)]);
    }

    #[test]
    fn a_trigger_runs_and_moves_the_fallback() {
        let mut s = TriggerSchedule::new(S).unwrap();
        let _ = run(&mut s, &[], host(0));
        assert_eq!(run(&mut s, &[30], host(30)), vec![TriggerEvent::Trigger(30)]);
        assert_eq!(run(&mut s, &[], host(129)), vec![]);
        assert_eq!(run(&mut s, &[], host(130)), vec![TriggerEvent::Fallback(130)]);
    }

    #[test]
    fn missed_host_fallbacks_run_as_one_at_the_latest_due_time() {
        let mut s = TriggerSchedule::new(S).unwrap();
        let _ = run(&mut s, &[], host(0));
        assert_eq!(run(&mut s, &[], host(350)), vec![TriggerEvent::Fallback(300)]);
        assert_eq!(run(&mut s, &[], host(399)), vec![]);
        assert_eq!(run(&mut s, &[], host(400)), vec![TriggerEvent::Fallback(400)]);
    }

    #[test]
    fn a_host_clock_jump_runs_one_fallback() {
        let mut s = TriggerSchedule::new(S).unwrap();
        let _ = run(&mut s, &[], host(0));
        let year = 365 * 24 * 3_600 * 1_000_000_000_i64;
        assert_eq!(run(&mut s, &[], host(year)), vec![TriggerEvent::Fallback(year)]);
    }

    #[test]
    fn a_host_trigger_older_than_the_limit_is_late() {
        let mut s = TriggerSchedule::new(S).unwrap();
        assert_eq!(run(&mut s, &[10], host(1_000_000)), vec![TriggerEvent::Late(10)]);
        assert_eq!(run(&mut s, &[], host(1_000_099)), vec![], "no burst of fallbacks");
    }

    #[test]
    fn host_silence_counts_from_the_send_stamp_of_a_trigger() {
        let mut s = TriggerSchedule::new(S).unwrap();
        let _ = run(&mut s, &[], host(0));
        // Sent at 50, drained at 90: the next fallback is due at 150, not at 190.
        assert_eq!(run(&mut s, &[50], host(90)), vec![TriggerEvent::Trigger(50)]);
        assert_eq!(run(&mut s, &[], host(149)), vec![]);
        assert_eq!(run(&mut s, &[], host(150)), vec![TriggerEvent::Fallback(150)]);
    }

    #[test]
    fn a_host_trigger_stamped_before_a_step_that_ran_is_late() {
        let mut s = TriggerSchedule::new(S).unwrap();
        let _ = run(&mut s, &[], host(0));
        assert_eq!(run(&mut s, &[], host(150)), vec![TriggerEvent::Fallback(100)]);
        // Sent at 90, younger than the limit, but drained after the fallback at 100.
        assert_eq!(run(&mut s, &[90], host(160)), vec![TriggerEvent::Late(90)]);
        // A trigger at the last step's time still runs: the step time does not go back.
        assert_eq!(run(&mut s, &[100], host(170)), vec![TriggerEvent::Trigger(100)]);
    }

    #[test]
    fn a_sim_trigger_stamped_before_the_last_event_is_refused() {
        let free = Limit { at: None, blocking: None };
        let sim = |now| TriggerClock::Sim { now, limit: &free };
        let mut s = TriggerSchedule::new(1_000).unwrap();
        let mut out = Vec::new();
        assert_eq!(
            s.admit(&[200, 150], sim(300), &mut out),
            Ok(TriggerAdmit::OutOfOrder { stamp: 150, before: 200 })
        );
        assert!(out.is_empty(), "nothing runs");
        assert_eq!(run(&mut s, &[200], sim(300)), vec![TriggerEvent::Trigger(200)]);
        assert_eq!(
            s.admit(&[199], sim(300), &mut out),
            Ok(TriggerAdmit::OutOfOrder { stamp: 199, before: 200 })
        );
        assert_eq!(run(&mut s, &[200], sim(300)), vec![TriggerEvent::Trigger(200)], "a tie runs");
    }

    #[test]
    fn a_sim_trigger_before_the_first_evaluation_still_runs() {
        let free = Limit { at: None, blocking: None };
        let mut s = TriggerSchedule::new(S).unwrap();
        let clock = TriggerClock::Sim { now: 1_000, limit: &free };
        assert_eq!(run(&mut s, &[990], clock), vec![TriggerEvent::Trigger(990)]);
    }

    #[test]
    fn host_fallbacks_before_a_late_trigger_run_as_one() {
        let mut s = TriggerSchedule::new(S).unwrap();
        let _ = run(&mut s, &[], host(0));
        assert_eq!(
            run(&mut s, &[350], host(400)),
            vec![TriggerEvent::Fallback(300), TriggerEvent::Trigger(350)]
        );
    }

    #[test]
    fn a_long_sim_jump_is_admitted_over_capped_calls_in_order() {
        let free = Limit { at: None, blocking: None };
        let mut s = TriggerSchedule::new(S).unwrap();
        let _ = run(&mut s, &[], TriggerClock::Sim { now: 0, limit: &free });
        let now = S * 3000;
        let mut all = Vec::new();
        let mut calls = 0;
        loop {
            let events = run(&mut s, &[], TriggerClock::Sim { now, limit: &free });
            assert!(events.len() <= MAX_EVENTS_PER_ADMIT);
            calls += 1;
            all.extend(events);
            if !s.capped() {
                break;
            }
        }
        assert_eq!(calls, 3, "1024 + 1024 + 952 events");
        let expected: Vec<TriggerEvent> =
            (1..=3000).map(|n| TriggerEvent::Fallback(S * n)).collect();
        assert_eq!(all, expected);
    }

    #[test]
    fn a_sim_jump_runs_each_fallback_it_skips() {
        let free = Limit { at: None, blocking: None };
        let mut s = TriggerSchedule::new(S).unwrap();
        let _ = run(&mut s, &[], TriggerClock::Sim { now: 0, limit: &free });
        assert_eq!(
            run(&mut s, &[], TriggerClock::Sim { now: 350, limit: &free }),
            vec![
                TriggerEvent::Fallback(100),
                TriggerEvent::Fallback(200),
                TriggerEvent::Fallback(300),
            ]
        );
    }

    #[test]
    fn a_sim_fallback_waits_until_no_upstream_can_send_a_trigger_at_its_time() {
        let mut s = TriggerSchedule::new(S).unwrap();
        let free = Limit { at: None, blocking: None };
        let _ = run(&mut s, &[], TriggerClock::Sim { now: 100, limit: &free });
        // The upstream reported 199: it may still send a trigger stamped 200.
        let at_fallback = Limit { at: Some(200), blocking: Some(("up".into(), Some(199))) };
        let held = TriggerClock::Sim { now: 300, limit: &at_fallback };
        assert_eq!(run(&mut s, &[], held), vec![]);
        assert_eq!(s.frontier(&[], &at_fallback), Some(200));
        // It reported 200 and sent the trigger at 200, which replaces the fallback.
        let past = Limit { at: Some(201), blocking: Some(("up".into(), Some(200))) };
        let clock = TriggerClock::Sim { now: 300, limit: &past };
        assert_eq!(run(&mut s, &[200], clock), vec![TriggerEvent::Trigger(200)]);
    }

    #[test]
    fn a_trigger_at_the_fallback_time_preempts_it() {
        let mut s = TriggerSchedule::new(S).unwrap();
        let _ = run(&mut s, &[], host(0));
        assert_eq!(run(&mut s, &[100], host(100)), vec![TriggerEvent::Trigger(100)]);
    }

    #[test]
    fn a_late_trigger_runs_after_the_fallbacks_before_it() {
        let free = Limit { at: None, blocking: None };
        let mut s = TriggerSchedule::new(S).unwrap();
        let _ = run(&mut s, &[], TriggerClock::Sim { now: 0, limit: &free });
        assert_eq!(
            run(&mut s, &[250], TriggerClock::Sim { now: 250, limit: &free }),
            vec![
                TriggerEvent::Fallback(100),
                TriggerEvent::Fallback(200),
                TriggerEvent::Trigger(250)
            ]
        );
    }

    #[test]
    fn sim_triggers_wait_for_the_clock() {
        let free = Limit { at: None, blocking: None };
        let mut s = TriggerSchedule::new(1_000).unwrap();
        let _ = run(&mut s, &[], TriggerClock::Sim { now: 0, limit: &free });
        assert_eq!(run(&mut s, &[100], TriggerClock::Sim { now: 50, limit: &free }), vec![]);
        assert_eq!(s.frontier(&[100], &free), Some(100));
    }

    #[test]
    fn sim_triggers_wait_for_the_upstream_limit() {
        let limit = Limit { at: Some(150), blocking: Some(("up".into(), Some(149))) };
        let mut s = TriggerSchedule::new(1_000).unwrap();
        let _ = run(&mut s, &[], TriggerClock::Sim { now: 0, limit: &limit });
        assert_eq!(
            run(&mut s, &[100, 200], TriggerClock::Sim { now: 300, limit: &limit }),
            vec![TriggerEvent::Trigger(100)]
        );
        // 200 is still pending; the limit caps the report below 150.
        assert_eq!(s.frontier(&[200], &limit), Some(150));
    }

    #[test]
    fn a_held_trigger_waits_with_no_time_limit() {
        let limit = Limit { at: Some(50), blocking: Some(("upstream_a".into(), Some(49))) };
        let mut s = TriggerSchedule::new(1_000).unwrap();
        let mut out = Vec::new();
        let _ = s.admit(&[], TriggerClock::Sim { now: 0, limit: &limit }, &mut out);
        for _ in 0..3 {
            assert_eq!(
                s.admit(&[100], TriggerClock::Sim { now: 200, limit: &limit }, &mut out),
                Ok(TriggerAdmit::Run)
            );
            assert!(out.is_empty(), "the trigger stays held");
        }
    }

    #[test]
    fn a_wall_clock_step_back_does_not_move_a_host_fallback() {
        let mut s = TriggerSchedule::new(S).unwrap();
        assert_eq!(run(&mut s, &[], host_at(1_000, 0)), vec![]);
        // The wall clock steps back by 960 ns; the fallback stays at its steady time.
        assert_eq!(run(&mut s, &[], host_at(40, 99)), vec![]);
        assert_eq!(run(&mut s, &[], host_at(40, 100)), vec![TriggerEvent::Fallback(100)]);
    }

    #[test]
    fn a_trigger_received_across_a_wall_step_keeps_its_steady_silence() {
        let mut s = TriggerSchedule::new(S).unwrap();
        let _ = run(&mut s, &[], host(0));
        // Stamped at wall 1_000 after the wall clock stepped back to 40: sent at once.
        assert_eq!(run(&mut s, &[1_000], host_at(40, 100)), vec![TriggerEvent::Trigger(100)]);
        // Its silence counts from its steady time, whatever the wall clock does next.
        assert_eq!(run(&mut s, &[], host_at(-5_000, 199)), vec![]);
        assert_eq!(run(&mut s, &[], host_at(-5_000, 200)), vec![TriggerEvent::Fallback(200)]);
    }

    #[test]
    fn a_trigger_with_an_old_wall_stamp_is_late_after_a_steady_fallback() {
        let mut s = TriggerSchedule::new(S).unwrap();
        let _ = run(&mut s, &[], host_at(1_000, 0));
        assert_eq!(run(&mut s, &[], host_at(1_000, 100)), vec![TriggerEvent::Fallback(100)]);
        // Sent at steady 50, before the fallback at 100: late.
        assert_eq!(run(&mut s, &[950], host_at(1_010, 110)), vec![TriggerEvent::Late(950)]);
    }
}
