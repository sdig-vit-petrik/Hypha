//! Run admission and budget selection.

use std::time::Duration;

use hypha_core::dispatch::DispatchLimits;
use hypha_core::port::dispatch::RunReport;

use hypha_runtime::{RunDriver, RunError, RunState};

#[derive(Default)]
struct Driver {
    budget: u32,
    limit: Option<Duration>,
}

impl RunDriver for Driver {
    fn dispatch(&mut self, budget: u32, limit: Option<Duration>) -> RunReport {
        self.budget = budget;
        self.limit = limit;
        RunReport::requested()
    }
}

#[test]
#[allow(clippy::expect_used)]
fn run_uses_core_budget_and_forwards_deadline() {
    let state = RunState::default();
    let mut driver = Driver::default();
    let limit = Duration::from_millis(20);
    let report = state
        .run(DispatchLimits::default(), Some(limit), &mut driver)
        .expect("run should be admitted");
    assert_eq!(driver.budget, DispatchLimits::default().dispatch_budget());
    assert_eq!(driver.limit, Some(limit));
    assert_eq!(report, RunReport::requested());
    assert!(!state.is_running());
}

struct Reentrant<'a> {
    state: &'a RunState,
    inner: Option<Result<(), RunError>>,
}

impl RunDriver for Reentrant<'_> {
    fn dispatch(&mut self, _budget: u32, _limit: Option<Duration>) -> RunReport {
        let mut nested = Driver::default();
        self.inner = Some(self.state.run(DispatchLimits::default(), None, &mut nested).map(|_| ()));
        RunReport::requested()
    }
}

#[test]
#[allow(clippy::expect_used)]
fn concurrent_run_is_refused_without_dispatch() {
    let state = RunState::default();
    let mut driver = Reentrant { state: &state, inner: None };
    state.run(DispatchLimits::default(), None, &mut driver).expect("outer run");
    assert_eq!(driver.inner, Some(Err(RunError::AlreadyRunning)));
    assert!(!state.is_running());
}

#[test]
fn panic_releases_run_ownership_for_the_next_call() {
    struct Panicking;
    impl RunDriver for Panicking {
        #[allow(clippy::panic)]
        fn dispatch(&mut self, _budget: u32, _limit: Option<Duration>) -> RunReport {
            panic!("adapter panic")
        }
    }
    let state = RunState::default();
    let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = state.run(DispatchLimits::default(), None, &mut Panicking);
    }));
    assert!(failed.is_err());
    assert!(!state.is_running());
}
