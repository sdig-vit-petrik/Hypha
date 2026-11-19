//! Replay under simulated time: the time-base bound and the fidelity report.
//!
//!
//! A step that cannot have its inputs (a transport overflow, or an eviction a cursor
//! had not passed) is not faithful: it is logged and fails the run.

use std::time::{Duration, Instant};

use crate::CaptureMode;

/// How far ahead of its cut, in periods, a sample may be stamped before a replay
/// step reports it as a time-base mismatch.
pub const TIME_BASE_BOUND_PERIODS: u64 = 250;

/// The shortest interval between two fidelity warnings for one topic.
pub(crate) const WARNING_INTERVAL: Duration = Duration::from_secs(1);

/// Whether a binding of `mode` counts the losses it sees: a queued binding always, a
/// latest binding only in replay.
#[must_use]
pub const fn counts_losses(mode: CaptureMode, is_replay: bool) -> bool {
    is_replay || matches!(mode, CaptureMode::Queued)
}

/// Whether a replay step lost its due value to a too-short history: a sample stamped
/// before `cut_nanos` was evicted and every retained one is stamped at or after it.
#[must_use]
pub fn is_short_history(
    newest_evicted: Option<u64>,
    framework_sent_nanos: impl IntoIterator<Item = u64>,
    cut_nanos: u64,
) -> bool {
    let evicted_a_due_sample = newest_evicted.is_some_and(|evicted| evicted < cut_nanos);
    if !evicted_a_due_sample {
        return false;
    }
    let mut retained = false;
    for sent in framework_sent_nanos {
        if sent < cut_nanos {
            return false;
        }
        retained = true;
    }
    retained
}

/// Lets a warning through at most once per interval.
#[derive(Debug, Default)]
struct WarningGate {
    last: Option<Instant>,
}

impl WarningGate {
    /// True when no warning went out within `interval` before `now`; starts a new interval.
    fn due(&mut self, now: Instant, interval: Duration) -> bool {
        let due = self.last.is_none_or(|last| now.saturating_duration_since(last) >= interval);
        if due {
            self.last = Some(now);
        }
        due
    }
}

/// The fidelity state of one managed input: its unfaithful losses and their warnings.
#[derive(Debug, Default)]
pub struct ReplayFidelity {
    /// Transport sequence gaps already reported.
    reported_missing: u64,
    /// Losses since the last warning.
    pending: u64,
    /// Losses since the source was created.
    total: u64,
    /// The warning gate of the loss warning.
    loss: WarningGate,
    /// The warning gate of the time-base check.
    time_base: WarningGate,
    /// The warning gate of the short-history check.
    short_history: WarningGate,
}

impl ReplayFidelity {
    /// Record the losses seen by one replay step, and return the warning due, if any.
    ///
    /// `missing_total` is the source's cumulative transport sequence gaps. Losses within
    /// one second of a warning are summed into the next. Returns this step's losses and
    /// the warning text when one is due.
    pub fn record(
        &mut self,
        topic: &str,
        lost: u64,
        missing_total: u64,
        now: Instant,
    ) -> (u64, Option<String>) {
        let missing = missing_total.saturating_sub(self.reported_missing);
        self.reported_missing = missing_total;
        let new_losses = lost.saturating_add(missing);
        self.pending = self.pending.saturating_add(new_losses);
        self.total = self.total.saturating_add(new_losses);
        if self.pending == 0 || !self.loss.due(now, WARNING_INTERVAL) {
            return (new_losses, None);
        }
        let count = std::mem::take(&mut self.pending);
        let text = format!(
            "replay not faithful: {count} samples lost on {topic} ({} in total)",
            self.total
        );
        (new_losses, Some(text))
    }

    /// The warning for samples a replay step cannot admit because they are stamped
    /// more than `bound_nanos` past its cut, or `None`.
    pub fn record_time_base(
        &mut self,
        topic: &str,
        ahead: usize,
        bound_nanos: u64,
        now: Instant,
    ) -> Option<String> {
        if ahead == 0 || !self.time_base.due(now, WARNING_INTERVAL) {
            return None;
        }
        Some(format!(
            "replay time base mismatch: {ahead} samples on {topic} are stamped more than {} ms \
             after the simulated time of the step, so no step can admit them. A publisher on \
             wall time cannot feed a simulated-time cycle; check that every node uses the same \
             time base",
            bound_nanos / 1_000_000
        ))
    }

    /// The warning for samples a replay step cannot admit because all retained
    /// samples are stamped at or after the deadline, or `None`.
    pub fn record_short_history(
        &mut self,
        topic: &str,
        retained: usize,
        deadline: u64,
        now: Instant,
    ) -> Option<String> {
        if retained == 0 || !self.short_history.due(now, WARNING_INTERVAL) {
            return None;
        }
        Some(format!(
            "all {retained} retained samples on {topic} are stamped at or after deadline {deadline}: \
             the input history is shorter than one replay chunk"
        ))
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_loss_is_reported_at_once_and_a_repeat_waits_for_the_interval() {
        let start = Instant::now();
        let mut fidelity = ReplayFidelity::default();
        let (_, first) = fidelity.record("/imu", 37, 0, start);
        assert_eq!(
            first.expect("the first loss is written"),
            "replay not faithful: 37 samples lost on /imu (37 in total)"
        );
        assert!(fidelity.record("/imu", 3, 0, start).1.is_none());
        let later = start + WARNING_INTERVAL;
        let (_, second) = fidelity.record("/imu", 0, 0, later);
        assert_eq!(
            second.expect("the summed losses follow"),
            "replay not faithful: 3 samples lost on /imu (40 in total)"
        );
    }

    #[test]
    fn a_rate_limited_loss_still_counts_as_this_steps_loss() {
        let start = Instant::now();
        let mut fidelity = ReplayFidelity::default();
        assert_eq!(fidelity.record("/imu", 1, 0, start).0, 1);
        // The warning waits for the interval, but the step lost two samples all the same.
        let (new_losses, warning) = fidelity.record("/imu", 2, 0, start);
        assert_eq!(new_losses, 2);
        assert!(warning.is_none());
    }

    #[test]
    fn a_transport_gap_is_counted_once() {
        let start = Instant::now();
        let mut fidelity = ReplayFidelity::default();
        assert!(fidelity.record("/imu", 0, 5, start).1.is_some());
        assert!(fidelity.record("/imu", 0, 5, start + WARNING_INTERVAL).1.is_none());
        let (new_losses, warning) = fidelity.record("/imu", 0, 7, start + WARNING_INTERVAL);
        assert_eq!(new_losses, 2);
        assert!(warning.is_some());
    }

    #[test]
    fn no_loss_writes_nothing() {
        let mut fidelity = ReplayFidelity::default();
        assert_eq!(fidelity.record("/imu", 0, 0, Instant::now()), (0, None));
    }

    #[test]
    fn no_short_history_before_any_eviction() {
        // The first step of an input whose samples all come after the deadline.
        assert!(!is_short_history(None, [60, 70], 50));
    }

    #[test]
    fn short_history_when_the_value_before_the_cut_was_evicted() {
        assert!(is_short_history(Some(40), [60, 50], 50));
        assert!(!is_short_history(Some(40), [], 50));
        assert!(!is_short_history(Some(40), [60, 45], 50));
    }

    #[test]
    fn no_short_history_when_only_samples_after_the_cut_were_evicted() {
        assert!(!is_short_history(Some(55), [60, 70], 50));
        assert!(!is_short_history(Some(50), [60, 70], 50));
    }

    #[test]
    fn a_latest_binding_counts_no_losses_in_host_time_and_counts_them_in_replay() {
        assert!(counts_losses(CaptureMode::Queued, false));
        assert!(!counts_losses(CaptureMode::Latest, false));
        assert!(counts_losses(CaptureMode::Queued, true));
        assert!(counts_losses(CaptureMode::Latest, true));
    }

    #[test]
    fn short_history_is_warned_once_per_interval() {
        let start = Instant::now();
        let mut fidelity = ReplayFidelity::default();
        assert!(fidelity.record_short_history("/kinematics", 0, 100_000_000, start).is_none());
        let first = fidelity
            .record_short_history("/kinematics", 1, 100_000_000, start)
            .expect("the first short history warning is written");
        assert_eq!(
            first,
            "all 1 retained samples on /kinematics are stamped at or after deadline 100000000: \
             the input history is shorter than one replay chunk"
        );
        assert!(fidelity.record_short_history("/kinematics", 1, 120_000_000, start).is_none());
        assert!(
            fidelity.record_short_history("/kinematics", 1, 120_000_000, later(start)).is_some()
        );
    }

    #[test]
    fn samples_far_ahead_of_the_cut_are_warned_once_per_interval() {
        let start = Instant::now();
        let mut fidelity = ReplayFidelity::default();
        assert!(fidelity.record_time_base("/clock_in", 0, 5_000_000_000, start).is_none());
        let first = fidelity
            .record_time_base("/speed", 50, 5_000_000_000, start)
            .expect("the first mismatch is written");
        assert!(first.starts_with("replay time base mismatch: 50 samples on /speed"));
        assert!(first.contains("5000 ms"));
        assert!(fidelity.record_time_base("/speed", 50, 5_000_000_000, start).is_none());
        assert!(fidelity.record_time_base("/speed", 50, 5_000_000_000, later(start)).is_some());
    }

    #[test]
    fn the_warning_gate_lets_one_through_per_interval() {
        let start = Instant::now();
        let mut gate = WarningGate::default();
        assert!(gate.due(start, WARNING_INTERVAL));
        assert!(!gate.due(start, WARNING_INTERVAL));
        assert!(gate.due(later(start), WARNING_INTERVAL));
    }

    fn later(start: Instant) -> Instant {
        start.checked_add(WARNING_INTERVAL).expect("a later instant")
    }
}
