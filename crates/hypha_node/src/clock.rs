//! Framework clock across the C boundary. The clock state lives in `hypha_time`.

use hypha_time::{Duration, Time};
use hypha_time::{
    advance_sim_time, disable_sim_time, enable_sim_time, is_sim_time, set_sim_time, steady_now,
};

use crate::status::Error;

/// The MD5 of `rosgraph_msgs/Clock`, the identity of `/clock` frames.
pub(crate) const CLOCK_MD5: &str = "a9c97c1d230cfc112e270351a944ee47";

/// The active framework time in nanoseconds; inside a cycle step, the time of the step.
#[must_use]
pub fn clock_now_nanos() -> i64 {
    hypha_time::framework_now().nanos()
}

/// The monotonic clock time in nanoseconds.
#[must_use]
pub fn clock_steady_now_nanos() -> i64 {
    steady_now().nanos()
}

/// True when the active clock is simulated.
#[must_use]
pub fn clock_is_sim_time() -> bool {
    is_sim_time()
}

/// Enable simulated time at `initial_nanos`. `InvalidArgument` when it is negative.
///
/// # Errors
/// [`Error::InvalidArgument`] when the clock refuses the value.
pub fn clock_enable_sim_time(initial_nanos: i64) -> Result<(), Error> {
    applied(enable_sim_time(Time::from_nanos(initial_nanos)))
}

/// Disable simulated time and restore the UTC wall clock.
pub fn clock_disable_sim_time() {
    disable_sim_time();
}

/// Set the simulated clock value.
///
/// # Errors
/// [`Error::InvalidArgument`] when simulated time is disabled or `nanos` is negative.
pub fn clock_set_sim_time(nanos: i64) -> Result<(), Error> {
    applied(set_sim_time(Time::from_nanos(nanos)))
}

/// Advance the simulated clock by `delta_nanos`.
///
/// # Errors
/// [`Error::InvalidArgument`] when simulated time is disabled or `delta_nanos` is negative.
pub fn clock_advance_sim_time(delta_nanos: i64) -> Result<(), Error> {
    applied(advance_sim_time(Duration::from_nanos(delta_nanos)))
}

/// Map a clock change that the registry refused to an error.
fn applied(applied: bool) -> Result<(), Error> {
    if applied {
        return Ok(());
    }
    Err(Error::InvalidArgument)
}
