//! The framework clock.

use crate::sdk::ffi;
use crate::sdk::status::numbered_error;

/// The current time in nanoseconds since the UNIX epoch.
#[must_use]
pub fn now_nanos() -> i64 {
    ffi::clock_now_nanos()
}

/// The current steady time in nanoseconds.
#[must_use]
pub fn steady_now_nanos() -> i64 {
    ffi::clock_steady_now_nanos()
}

/// Enables simulation time at `initial_nanos`, which stands until a `/clock`
/// sample or [`disable_sim_time`] moves it.
///
/// # Errors
/// Returns a message when the library refuses to enable sim time.
pub fn enable_sim_time(initial_nanos: i64) -> Result<(), String> {
    ffi::clock_enable_sim_time(initial_nanos)
        .map_err(|error| numbered_error("failed to enable sim time", error))
}

/// Disables simulation time.
///
/// # Errors
/// Never fails: the runtime always restores the wall clock.
pub fn disable_sim_time() -> Result<(), String> {
    ffi::clock_disable_sim_time();
    Ok(())
}
