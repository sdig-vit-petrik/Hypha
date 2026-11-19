//! The due arithmetic of a clock-driven timer, as pure rules.

use crate::upstream_hold::nth_deadline;

/// The periods a replay step runs: zero when no deadline expired, else the periods
/// missed in this poll plus the deadline that fired.
#[must_use]
pub const fn replay_periods(fired: u32, missed_delta: u64) -> u64 {
    if fired == 0 { 0 } else { missed_delta.saturating_add(1) }
}

/// The deadline of a cycle after `missed_delta` more periods, from `scheduled_nanos`.
/// `None` when the deadline leaves the `i64` range.
#[must_use]
pub fn callback_deadline(
    scheduled_nanos: i64,
    missed_delta: u64,
    period_nanos: i64,
) -> Option<i64> {
    nth_deadline(scheduled_nanos, period_nanos, missed_delta)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn replay_runs_no_period_without_a_fired_deadline() {
        assert_eq!(replay_periods(0, 7), 0);
        assert_eq!(replay_periods(1, 0), 1);
        assert_eq!(replay_periods(2, 4), 5);
        assert_eq!(replay_periods(1, u64::MAX), u64::MAX);
    }

    #[test]
    fn a_callback_deadline_advances_by_its_missed_periods_and_refuses_overflow() {
        assert_eq!(callback_deadline(100, 3, 10), Some(130));
        assert_eq!(callback_deadline(-50, 0, 10), Some(-50));
        assert_eq!(callback_deadline(i64::MAX, 1, 1), None);
    }
}
