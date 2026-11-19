//! The steps of a replay window: the deadlines one admitted window runs.

use crate::upstream_hold::{HoldError, Window, nth_deadline};

/// One step of an admitted window: the deadline it runs at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    /// The deadline of the step, in nanoseconds.
    pub deadline: i64,
}

/// The steps of an admitted window, in order. See [`window_steps`].
#[derive(Debug, Clone)]
pub struct Steps {
    first_nanos: i64,
    period_nanos: i64,
    /// The index of the next deadline to step.
    next: u64,
    /// One past the index of the last deadline to step.
    end: u64,
}

/// The steps that an admitted window runs: one step at each of its deadlines. Each item
/// fails when its deadline is not representable.
#[must_use]
pub fn window_steps(window: Window, period_nanos: i64) -> Steps {
    Steps { first_nanos: window.first_nanos, period_nanos, next: 0, end: window.periods }
}

impl Iterator for Steps {
    type Item = Result<Step, HoldError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.next >= self.end {
            return None;
        }
        let index = self.next;
        self.next = self.next.saturating_add(1);
        let deadline = nth_deadline(self.first_nanos, self.period_nanos, index);
        Some(deadline.map(|deadline| Step { deadline }).ok_or(HoldError::Overflow))
    }
}

#[cfg(test)]
#[allow(clippy::arithmetic_side_effects, clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    const PERIOD: i64 = 10;

    fn window(first_nanos: i64, periods: u64) -> Window {
        Window { first_nanos, periods }
    }

    #[test]
    fn a_window_steps_at_each_of_its_deadlines() {
        let steps: Vec<Step> =
            window_steps(window(100, 3), PERIOD).map(|step| step.expect("representable")).collect();
        assert_eq!(steps, [Step { deadline: 100 }, Step { deadline: 110 }, Step { deadline: 120 }]);
        assert_eq!(window_steps(window(100, 0), PERIOD).count(), 0);
    }

    #[test]
    fn a_jump_of_many_periods_steps_at_each_of_the_periods_it_covers() {
        let steps: Vec<Step> =
            window_steps(window(100, 4), PERIOD).map(|step| step.expect("representable")).collect();
        assert_eq!(
            steps,
            [
                Step { deadline: 100 },
                Step { deadline: 110 },
                Step { deadline: 120 },
                Step { deadline: 130 },
            ]
        );
    }

    #[test]
    fn a_step_past_the_representable_clock_fails_the_window() {
        let mut steps = window_steps(window(i64::MAX - 5, 2), PERIOD);
        assert_eq!(steps.next(), Some(Ok(Step { deadline: i64::MAX - 5 })));
        assert_eq!(steps.next(), Some(Err(HoldError::Overflow)));
        assert_eq!(steps.next(), None);
    }
}
