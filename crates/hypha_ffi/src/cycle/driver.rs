//! The C ABI wrappers of `hypha_node::cycle::driver`.

use super::{CycleTarget, HyphaCycle};
use crate::boundary::{borrow_str, read_in};
use crate::status::try_guard;
use hypha_node::HyphaClockDriverOptions;
use hypha_node::safe::{cycle_expect_participant, cycle_set_clock_driver};
use hypha_node::status::HyphaStatus;
use std::os::raw::c_char;

/// Make an uncommitted rate cycle drive `/clock`; its period is the clock step. Commit
/// enables process-wide simulated time at `start_nanos`. The driver publishes
/// `/clock = start_nanos` with no step; its first step runs at `start_nanos` plus one
/// period. `options` is read for the call only. `InvalidArgument`: null pointer, a
/// start below zero or of 2^32 seconds or more (`/clock` carries seconds in 32 bits; the
/// reason is then `hypha_last_error`), a zero timeout, a factor that is negative or not finite, a cycle
/// without a rate, or a triggered cycle.
/// `InvalidState`: committed cycle, or its `WaitSet` runs.
///
/// # Safety
/// `cycle` is null or a live handle; `options` is null or readable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_cycle_set_clock_driver(
    cycle: *mut HyphaCycle,
    options: *const HyphaClockDriverOptions,
) -> HyphaStatus {
    try_guard(|| {
        let options = unsafe { read_in(options) }?;
        let (set, cycle) = unsafe { CycleTarget::of(cycle)?.waitset() }?;
        Ok(cycle_set_clock_driver(&set, cycle, options)?)
    })
}

/// Add the node `name` to the participants that must acknowledge a driver cycle's start
/// value before its first step. `name` points to `name_len` UTF-8 bytes, read for the call only.
/// `InvalidArgument`: null cycle, empty or non-UTF-8 name, a cycle without
/// `hypha_cycle_set_clock_driver`. `InvalidState`: committed cycle, or its `WaitSet` runs.
///
/// # Safety
/// `cycle` is null or a live handle; `name` is null or points to `name_len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_cycle_expect_participant(
    cycle: *mut HyphaCycle,
    name: *const c_char,
    name_len: usize,
) -> HyphaStatus {
    try_guard(|| {
        let cycle = unsafe { CycleTarget::of(cycle) }?;
        let name = unsafe { borrow_str(name, name_len) }?;
        let (set, cycle) = unsafe { cycle.waitset() }?;
        Ok(cycle_expect_participant(&set, cycle, name)?)
    })
}
