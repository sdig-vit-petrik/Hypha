//! The C ABI wrappers of `hypha_node::clock`.

use crate::status::status_of;
use hypha_node::safe::{
    clock_advance_sim_time, clock_disable_sim_time, clock_enable_sim_time, clock_is_sim_time,
    clock_is_valid, clock_now_nanos, clock_set_sim_time, clock_steady_now_nanos,
};
use hypha_node::status::HyphaStatus;

#[unsafe(no_mangle)]
/// Return framework time in nanoseconds since the Unix epoch (UTC), simulated while
/// simulated time is enabled. Inside a cycle step it is the time of the step.
pub extern "C" fn hypha_clock_now_nanos() -> i64 {
    clock_now_nanos()
}

#[unsafe(no_mangle)]
/// Return the host monotonic clock time in nanoseconds. It never follows simulated time.
pub extern "C" fn hypha_clock_steady_now_nanos() -> i64 {
    clock_steady_now_nanos()
}

#[unsafe(no_mangle)]
/// Return 1 while the active clock is simulated, else 0.
pub extern "C" fn hypha_clock_is_sim_time() -> i32 {
    i32::from(clock_is_sim_time())
}

#[unsafe(no_mangle)]
/// Return 1 when framework time holds a time, else 0. Host time always does; simulated
/// time does once it has a value (a `/clock` sample, a set, an advance, or an enable at a
/// value). The zero that a simulated-time node starts with is a placeholder: until the
/// first value `hypha_clock_now_nanos` returns it, and this returns 0.
pub extern "C" fn hypha_clock_is_valid() -> i32 {
    i32::from(clock_is_valid())
}

#[unsafe(no_mangle)]
/// Enable simulated time at `initial_nanos`, for a test or a tool that owns the
/// simulation; not beside a sim-time node or a clock-driver cycle.
///
/// Returns `HyphaStatus::InvalidArgument` when `initial_nanos` is negative.
pub extern "C" fn hypha_clock_enable_sim_time(initial_nanos: i64) -> i32 {
    status_of(clock_enable_sim_time(initial_nanos)) as i32
}

#[unsafe(no_mangle)]
/// Disable simulated time and restore the UTC wall clock. Returns `HyphaStatus::Ok`.
pub extern "C" fn hypha_clock_disable_sim_time() -> i32 {
    clock_disable_sim_time();
    HyphaStatus::Ok as i32
}

#[unsafe(no_mangle)]
/// Set the simulated clock value.
///
/// Returns `HyphaStatus::InvalidArgument` when simulated time is disabled or
/// `nanos` is negative.
pub extern "C" fn hypha_clock_set_sim_time(nanos: i64) -> i32 {
    status_of(clock_set_sim_time(nanos)) as i32
}

#[unsafe(no_mangle)]
/// Advance the simulated clock by `delta_nanos`.
///
/// Returns `HyphaStatus::InvalidArgument` when simulated time is disabled or
/// `delta_nanos` is negative.
pub extern "C" fn hypha_clock_advance_sim_time(delta_nanos: i64) -> i32 {
    status_of(clock_advance_sim_time(delta_nanos)) as i32
}
