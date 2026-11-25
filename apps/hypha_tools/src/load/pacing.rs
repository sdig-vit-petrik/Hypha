//! The schedule of one publisher: its periodic deadlines, its bursts, and the deadlines
//! of the measuring window it missed.
//!
//! A periodic publisher keeps absolute deadlines `start + k * period`. When it gets to
//! send, every deadline that has passed is due; it sends one message for the latest of
//! them and counts the earlier ones as missed, the way a timer does: it does not catch
//! up. A deadline counts against the window when it lies in `[window_start,
//! window_end)`, whenever the publisher noticed it. A burst adds messages back to back
//! at its own interval and has no deadlines to miss.

use crate::load::protocol::Times;

/// What is due at one moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Due {
    /// Messages to send now.
    pub sends: u64,
    /// Deadlines of the window that passed unserved.
    pub missed: u64,
    /// When something is next due.
    pub wake: i64,
}

/// The schedule of one publisher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pacer {
    /// The period in nanoseconds, at least 1; `None` sends back to back.
    period: Option<i64>,
    /// Messages per burst and nanoseconds between bursts, at least 1.
    burst: Option<(u32, i64)>,
    next_period: i64,
    next_burst: Option<i64>,
    window_start: i64,
    window_end: i64,
}

/// `numerator / divisor` rounded up, for a positive `divisor`.
fn ceil_div(numerator: i64, divisor: i64) -> i64 {
    let quotient = numerator.checked_div_euclid(divisor).unwrap_or(0);
    let exact = numerator.checked_rem_euclid(divisor) == Some(0);
    if exact { quotient } else { quotient.saturating_add(1) }
}

impl Pacer {
    /// The schedule from `times.start`. A period or burst interval below 1 ns is 1 ns.
    #[must_use]
    pub fn new(period: Option<i64>, burst: Option<(u32, i64)>, times: Times) -> Self {
        let burst = burst.map(|(count, every)| (count, every.max(1)));
        Self {
            period: period.map(|period| period.max(1)),
            burst,
            next_period: times.start,
            next_burst: burst.map(|(_, every)| times.start.saturating_add(every)),
            window_start: times.window_start,
            window_end: times.window_end,
        }
    }

    /// The deadlines `next_period + k * period` for `k` in `[0, count)` that lie in the
    /// window.
    fn in_window(&self, period: i64, count: i64) -> u64 {
        let first = ceil_div(self.window_start.saturating_sub(self.next_period), period).max(0);
        let end =
            ceil_div(self.window_end.saturating_sub(self.next_period), period).clamp(0, count);
        u64::try_from(end.saturating_sub(first)).unwrap_or(0)
    }

    /// What is due at `now`, which moves the schedule on.
    pub fn due(&mut self, now: i64) -> Due {
        let mut due = Due { sends: 0, missed: 0, wake: i64::MAX };
        match self.period {
            Some(period) if now >= self.next_period => {
                // Deadlines `next_period + k * period` for k in [0, passed) have passed;
                // the last is served and the others are missed.
                let passed = now.saturating_sub(self.next_period).checked_div(period).unwrap_or(0);
                let passed = passed.saturating_add(1);
                due.missed = self.in_window(period, passed.saturating_sub(1));
                self.next_period = self.next_period.saturating_add(passed.saturating_mul(period));
                due.sends = 1;
            }
            Some(_) => {}
            None => due.sends = 1,
        }
        if let (Some((count, every)), Some(at)) = (self.burst, self.next_burst)
            && now >= at
        {
            due.sends = due.sends.saturating_add(u64::from(count));
            let passed = now.saturating_sub(at).checked_div(every).unwrap_or(0).saturating_add(1);
            self.next_burst = Some(at.saturating_add(passed.saturating_mul(every)));
        }
        if self.period.is_some() {
            due.wake = self.next_period;
        }
        if let Some(at) = self.next_burst {
            due.wake = due.wake.min(at);
        }
        due
    }

    /// The deadlines of the window that were never served: those from the next one on,
    /// for a publisher that stopped. 0 for a publisher without a period.
    #[must_use]
    pub fn unserved(&self) -> u64 {
        self.period.map_or(0, |period| self.in_window(period, i64::MAX))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A period of 10, the window [100, 200): its deadlines are 100, 110, ..., 190.
    fn pacer() -> Pacer {
        let times = Times { start: 0, window_start: 100, window_end: 200, publish_end: 250 };
        Pacer::new(Some(10), None, times)
    }

    /// Serves every deadline from the schedule's next one up to `until` on time.
    fn on_time(pacer: &mut Pacer, until: i64) {
        while pacer.next_period < until {
            let due = pacer.due(pacer.next_period);
            assert_eq!((due.sends, due.missed), (1, 0));
        }
    }

    #[test]
    fn a_publisher_on_time_misses_nothing_and_wakes_at_its_next_deadline() {
        let mut pacer = pacer();
        let due = pacer.due(0);
        assert_eq!(due, Due { sends: 1, missed: 0, wake: 10 });
        assert_eq!(pacer.due(5), Due { sends: 0, missed: 0, wake: 10 });
        on_time(&mut pacer, 250);
        assert_eq!(pacer.unserved(), 0);
    }

    #[test]
    fn a_stall_before_the_window_counts_nothing() {
        let mut pacer = pacer();
        pacer.due(0);
        // Deadlines 10 to 90 pass; 90 is served, 10 to 80 lie before the window.
        assert_eq!(pacer.due(95), Due { sends: 1, missed: 0, wake: 100 });
    }

    #[test]
    fn a_stall_across_the_window_start_counts_the_window_deadlines_it_missed() {
        let mut pacer = pacer();
        pacer.due(0);
        // Deadlines 10 to 120 pass; 120 is served, 100 and 110 are the window's missed.
        assert_eq!(pacer.due(125).missed, 2);
        assert_eq!(pacer.next_period, 130);
    }

    #[test]
    fn a_stall_inside_the_window_counts_every_deadline_it_skipped() {
        let mut pacer = pacer();
        on_time(&mut pacer, 130);
        // 130 to 170 pass at 175: 170 is served, 130 to 160 are missed.
        assert_eq!(pacer.due(175).missed, 4);
    }

    #[test]
    fn a_stall_across_the_window_end_counts_only_the_window_deadlines() {
        let mut pacer = pacer();
        on_time(&mut pacer, 180);
        // 180 to 220 pass at 225: 220 is served; 180 and 190 lie in the window.
        assert_eq!(pacer.due(225).missed, 2);
    }

    #[test]
    fn a_stall_after_the_window_counts_nothing() {
        let mut pacer = pacer();
        on_time(&mut pacer, 200);
        assert_eq!(pacer.due(245).missed, 0);
    }

    #[test]
    fn a_publisher_that_stops_inside_the_window_leaves_its_unserved_deadlines() {
        let mut pacer = pacer();
        on_time(&mut pacer, 150);
        // It stopped at 150 (a stall to the end of publishing): 150 to 190 never sent.
        assert_eq!(pacer.unserved(), 5);
    }

    #[test]
    fn a_burst_adds_messages_and_misses_no_period() {
        let times = Times { start: 0, window_start: 0, window_end: 100, publish_end: 100 };
        let mut pacer = Pacer::new(Some(10), Some((3, 25)), times);
        assert_eq!(pacer.due(0), Due { sends: 1, missed: 0, wake: 10 });
        assert_eq!(pacer.due(10).sends, 1);
        assert_eq!(pacer.due(20), Due { sends: 1, missed: 0, wake: 25 });
        assert_eq!(pacer.due(25), Due { sends: 3, missed: 0, wake: 30 });
        assert_eq!(pacer.due(30), Due { sends: 1, missed: 0, wake: 40 });
    }

    #[test]
    fn a_publisher_without_a_period_sends_at_once_and_has_no_deadlines() {
        let times = Times { start: 0, window_start: 0, window_end: 100, publish_end: 100 };
        let mut pacer = Pacer::new(None, None, times);
        assert_eq!(pacer.due(7), Due { sends: 1, missed: 0, wake: i64::MAX });
        assert_eq!(pacer.unserved(), 0);
    }
}
