//! The time of the cycle step that runs on this thread.
//!
//! While a step runs, framework time on its thread is the step's time, not the clock:
//! a replay runs several steps at their own deadlines while the clock stays put.

use std::cell::Cell;

use crate::{Time, now};

thread_local! {
    /// The time of the step that runs on this thread, if one does.
    static STEP_TIME: Cell<Option<i64>> = const { Cell::new(None) };
}

/// Holds framework time at a step's time on this thread until it is dropped.
#[must_use = "the step time holds only while the guard lives"]
pub struct StepTimeGuard(Option<i64>);

impl StepTimeGuard {
    /// Framework time on this thread is `nanos` until the guard is dropped.
    pub fn hold(nanos: i64) -> Self {
        Self(STEP_TIME.replace(Some(nanos)))
    }
}

impl Drop for StepTimeGuard {
    fn drop(&mut self) {
        STEP_TIME.set(self.0);
    }
}

/// Framework time on this thread: the time of the step that runs on it, else [`now`].
#[must_use]
pub fn framework_now() -> Time {
    STEP_TIME.get().map_or_else(now, Time::from_nanos)
}

#[cfg(test)]
mod tests {
    use super::{StepTimeGuard, framework_now};
    use crate::now;

    #[test]
    fn a_step_holds_framework_time_and_restores_it() {
        let outer = StepTimeGuard::hold(500);
        assert_eq!(framework_now().nanos(), 500);
        {
            let _inner = StepTimeGuard::hold(700);
            assert_eq!(framework_now().nanos(), 700);
        }
        assert_eq!(framework_now().nanos(), 500);
        drop(outer);
        let after = framework_now().nanos();
        assert!(after >= now().nanos().saturating_sub(1_000_000_000), "the clock again");
    }
}
