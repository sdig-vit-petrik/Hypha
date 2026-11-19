//! The effective-rate report of `hypha play --clock`, kept free of the bus.
//!
//! Each chunk barrier charges the waiting it added to the participants, in the order they
//! answered: the first answer is charged from the send, each later one from the answer
//! before it. The participant with the largest charge set the rate. A participant that
//! holds for an upstream is charged only its own increment, not the upstream's wait.

use crate::replay_sync::Pace;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// The shortest wall time a rate divides by, so that a zero wall time reads as a rate.
const MIN_WALL: Duration = Duration::from_nanos(1);

/// The waiting that one participant added to the replay.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Charge {
    /// How many chunk barriers this participant answered last.
    pub last_in: u64,
    /// The waiting it added, summed over the barriers.
    pub waited: Duration,
}

/// The figures of one replay, from which its effective rate is read.
#[derive(Debug, Clone, Default)]
pub struct RateReport {
    data_nanos: u64,
    pace_wait: Duration,
    paused: Duration,
    barriers: u64,
    charged: BTreeMap<String, Charge>,
}

impl RateReport {
    /// Charges one chunk barrier. `sent` is when its `/clock` sample first went out, and
    /// `first_acks` holds each participant's first answer to it, in any order.
    pub fn record_barrier(&mut self, sent: Instant, first_acks: &[(String, Instant)]) {
        self.barriers = self.barriers.saturating_add(1);
        let mut order: Vec<&(String, Instant)> = first_acks.iter().collect();
        order.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
        let mut previous = sent;
        for (name, answered) in &order {
            let charge = self.charged.entry(name.clone()).or_default();
            charge.waited =
                charge.waited.saturating_add(answered.saturating_duration_since(previous));
            previous = *answered;
        }
        if let Some((name, _)) = order.last() {
            let charge = self.charged.entry(name.clone()).or_default();
            charge.last_in = charge.last_in.saturating_add(1);
        }
    }

    /// Adds data time that the replay published, in nanoseconds.
    pub fn add_data(&mut self, nanos: u64) {
        self.data_nanos = self.data_nanos.saturating_add(nanos);
    }

    /// Adds wall time that the schedule made the replay wait, for `--rate`.
    pub fn add_pace_wait(&mut self, waited: Duration) {
        self.pace_wait = self.pace_wait.saturating_add(waited);
    }

    /// Adds wall time that the replay spent paused.
    pub fn add_paused(&mut self, paused: Duration) {
        self.paused = self.paused.saturating_add(paused);
    }

    /// The one-line report: the effective rate, and what set it.
    #[must_use]
    pub fn summary(&self, wall: Duration, pace: Pace) -> String {
        let active = wall.saturating_sub(self.paused).max(MIN_WALL);
        let data = Duration::from_nanos(self.data_nanos).as_secs_f64();
        let effective = data / active.as_secs_f64();
        let limiter = self.limiter(wall, pace);
        format!(
            "effective rate {effective:.2}x: {data:.3} s of data in {:.3} s; {limiter}",
            wall.as_secs_f64()
        )
    }

    /// What set the rate: the participant with the largest charge (ties to the name that
    /// comes first), or the schedule when its own waiting was at least that large.
    fn limiter(&self, wall: Duration, pace: Pace) -> String {
        let top = self.charged.iter().filter(|(_, charge)| !charge.waited.is_zero()).fold(
            None,
            |best: Option<(&String, &Charge)>, (name, charge)| match best {
                Some((_, kept)) if kept.waited >= charge.waited => best,
                _ => Some((name, charge)),
            },
        );
        match top {
            Some((name, charge))
                if !(pace.factor().is_some() && self.pace_wait >= charge.waited) =>
            {
                let share = charge.waited.as_secs_f64() / wall.max(MIN_WALL).as_secs_f64() * 100.0;
                format!(
                    "set by '{name}' (last to answer {} of {} chunks, {:.3} s of waiting, {share:.0}% of the wall time)",
                    charge.last_in,
                    self.barriers,
                    charge.waited.as_secs_f64(),
                )
            }
            Some(_) => format!("set by --rate {pace}"),
            None if pace.factor().is_some() => "paced by --rate alone".to_string(),
            None => "not paced, and no participant held it".to_string(),
        }
    }
}

#[cfg(test)]
#[allow(clippy::arithmetic_side_effects, clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::{Charge, RateReport};
    use crate::replay_sync::Pace;
    use std::time::{Duration, Instant};

    fn ack(name: &str, at: Instant) -> (String, Instant) {
        (name.to_string(), at)
    }

    fn charge(report: &RateReport, name: &str) -> Charge {
        report.charged.get(name).copied().unwrap_or_default()
    }

    #[test]
    fn the_slowest_answer_sets_the_rate_by_its_increment() {
        let start = Instant::now();
        let mut report = RateReport::default();
        report.record_barrier(
            start,
            &[
                ack("x", start + Duration::from_millis(50)),
                ack("y", start + Duration::from_millis(5)),
            ],
        );
        // y is charged its 5 ms; x is charged the 45 ms that it added after y.
        assert_eq!(charge(&report, "y").waited, Duration::from_millis(5));
        assert_eq!(charge(&report, "x").waited, Duration::from_millis(45));
        assert_eq!(charge(&report, "y").last_in, 0);
        let line = report.summary(Duration::from_secs(1), Pace::Factor(1.0));
        assert!(line.contains("set by 'x'"), "{line}");
    }

    #[test]
    fn a_held_participant_is_charged_only_its_own_increment() {
        let start = Instant::now();
        let mut report = RateReport::default();
        // x answers at 50 ms, the chain's next node y at 52 ms: y answered last, but x set the rate.
        report.record_barrier(
            start,
            &[
                ack("x", start + Duration::from_millis(50)),
                ack("y", start + Duration::from_millis(52)),
            ],
        );
        assert_eq!(charge(&report, "y").last_in, 1);
        assert_eq!(charge(&report, "y").waited, Duration::from_millis(2));
        let line = report.summary(Duration::from_secs(1), Pace::Factor(1.0));
        assert!(line.contains("set by 'x'"), "{line}");
    }

    #[test]
    fn a_schedule_that_waited_longer_than_any_participant_sets_the_rate() {
        let start = Instant::now();
        let mut report = RateReport::default();
        report.record_barrier(start, &[ack("x", start + Duration::from_millis(50))]);
        report.add_pace_wait(Duration::from_millis(200));
        let line = report.summary(Duration::from_secs(1), Pace::Factor(1.0));
        assert!(line.contains("set by --rate 1.00x"), "{line}");
    }

    #[test]
    fn with_no_participant_the_schedule_alone_paces_the_replay() {
        let report = RateReport::default();
        let line = report.summary(Duration::from_secs(1), Pace::Factor(2.0));
        assert!(line.contains("paced by --rate alone"), "{line}");
    }

    #[test]
    fn a_zero_wall_time_does_not_divide_by_zero() {
        let mut report = RateReport::default();
        report.add_data(1_000_000_000);
        let line = report.summary(Duration::ZERO, Pace::Max);
        assert!(line.starts_with("effective rate "), "{line}");
    }

    #[test]
    fn paused_time_is_not_wall_time_for_the_rate() {
        let mut report = RateReport::default();
        report.add_data(2_000_000_000);
        report.add_paused(Duration::from_secs(1));
        // Two seconds of data in three seconds of wall time, one of them paused: 2 s of active
        // time, so 1x. Counting the pause would read 0.67x.
        let line = report.summary(Duration::from_secs(3), Pace::Max);
        assert!(line.starts_with("effective rate 1.00x"), "{line}");
    }

    #[test]
    fn the_summary_has_no_lossy_text() {
        let mut report = RateReport::default();
        report.add_data(500_000_000);
        let line = report.summary(Duration::from_secs(1), Pace::Max);
        assert_eq!(
            line,
            "effective rate 0.50x: 0.500 s of data in 1.000 s; not paced, and no participant held it"
        );
    }
}
