//! How a C call reaches a `WaitSet` from its raw handle.
//!
//! A `WaitSet` is not `Sync`: while it runs, another thread reaches only its
//! `WaitSetShared` and is refused there with `InvalidState`.

use hypha_node::safe::{HyphaWaitSet, WaitSetShared};
use hypha_node::status::HyphaStatus;

/// The cross-thread state of the `WaitSet` behind `waitset`, reached without a reference
/// to the `WaitSet` itself. `None` for null.
///
/// # Safety
/// `waitset` must be null or a live `WaitSet` handle, live for `'a`.
pub(crate) unsafe fn shared_of<'a>(waitset: *const HyphaWaitSet) -> Option<&'a WaitSetShared> {
    if waitset.is_null() {
        return None;
    }
    // SAFETY: non-null by the check and live by the caller; a place projection borrows
    // only this `Sync` field, written only when the `WaitSet` is built and dropped.
    let shared: &std::sync::Arc<WaitSetShared> = unsafe { &(*waitset).shared };
    Some(shared)
}

/// The `WaitSet` behind `waitset`. `InvalidArgument` for null, `InvalidState` while it
/// runs on another thread.
///
/// # Safety
/// `waitset` must be null or a live `WaitSet` handle, live for `'a`, that no other
/// thread uses meanwhile but through its `WaitSetShared`.
pub(crate) unsafe fn waitset_here<'a>(
    waitset: *const HyphaWaitSet,
) -> Result<&'a HyphaWaitSet, HyphaStatus> {
    // SAFETY: the caller's contract is this function's.
    let shared = unsafe { shared_of(waitset) }.ok_or(HyphaStatus::InvalidArgument)?;
    if shared.runs_on_another_thread() {
        return Err(HyphaStatus::InvalidState);
    }
    // SAFETY: non-null and live by the caller; no run holds it on another thread, and
    // the caller keeps every other thread off it but for `shared`.
    Ok(unsafe { &*waitset })
}
