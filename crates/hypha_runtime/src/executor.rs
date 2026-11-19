//! Safe run admission, dispatch-budget selection, and exit cleanup.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use hypha_core::dispatch::DispatchLimits;
use hypha_core::port::dispatch::RunReport;

/// The C ABI's execution handle state, independent of the native wait set.
#[derive(Debug, Default)]
pub struct RunState(AtomicBool);

impl RunState {
    /// Whether a run currently owns this execution handle.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// A boundary adapter that runs the native dispatcher.
pub trait RunDriver {
    /// Run the native wait set with the validated budget and optional wall-duration bound.
    fn dispatch(&mut self, budget: u32, limit: Option<Duration>) -> RunReport;
}

/// Why runtime could not begin a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunError {
    /// Another thread or a recursive invocation already owns the run.
    AlreadyRunning,
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyRunning => {
                f.write_str("another thread or a recursive invocation already owns the run")
            }
        }
    }
}

impl std::error::Error for RunError {}

struct RunGuard<'a>(&'a RunState);

impl Drop for RunGuard<'_> {
    fn drop(&mut self) {
        self.0.0.store(false, Ordering::SeqCst);
    }
}

impl RunState {
    /// Claim this execution handle and dispatch once.
    ///
    /// The guard releases run ownership even if a driver panics.
    ///
    /// # Errors
    /// Returns [`RunError::AlreadyRunning`] when another run owns this handle.
    pub fn run<D: RunDriver>(
        &self,
        limits: DispatchLimits,
        limit: Option<Duration>,
        driver: &mut D,
    ) -> Result<RunReport, RunError> {
        self.0
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| RunError::AlreadyRunning)?;
        let _guard = RunGuard(self);
        Ok(driver.dispatch(limits.dispatch_budget(), limit))
    }
}
