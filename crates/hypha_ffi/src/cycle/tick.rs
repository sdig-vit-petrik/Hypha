//! One tick of a cycle by hand, for this crate's tests and its cycle benchmark.

use std::os::raw::c_void;

use super::{CycleTarget, HyphaCycle};
use hypha_node::HyphaStatus;
use hypha_node::safe::cycle_tick;
use hypha_node::waitset::HyphaCallbackResult;

/// Run one tick of the committed cycle behind `cycle`, as its timer does, without a
/// `WaitSet` run. A null handle, or a `WaitSet` running on another thread, fails with
/// `InvalidState`. Not part of the C ABI; for tests and `benches/cycle.rs`.
///
/// # Safety
/// `cycle` must be null or a live cycle handle.
#[doc(hidden)]
pub unsafe fn cycle_timer_tick(missed_periods: u64, cycle: *mut c_void) -> HyphaCallbackResult {
    // SAFETY: null or a live handle by the caller, whose `WaitSet` is then live too.
    let reached = unsafe { CycleTarget::of(cycle.cast_const().cast::<HyphaCycle>()) }
        .and_then(|target| unsafe { target.waitset() });
    match reached {
        Ok((set, id)) => cycle_tick(set, id, missed_periods),
        Err(_) => HyphaCallbackResult::fail(HyphaStatus::InvalidState as i32),
    }
}
