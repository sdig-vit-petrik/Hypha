//! How a C call reaches a `WaitSet` from its raw handle.
//!
//! A `WaitSet` is not `Sync`: a call claims it through its `WaitSetShared` before it
//! forms a reference, so while another thread is inside a call or a run on it, the call
//! is refused there with `InvalidState` (#83).

use hypha_node::safe::{HyphaWaitSet, WaitSetClaim, WaitSetShared};
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

/// A `WaitSet` the calling thread holds for one call: its claim drops with it.
pub(crate) struct ClaimedWaitSet<'a> {
    set: &'a HyphaWaitSet,
    _claim: WaitSetClaim<'a>,
}

impl std::ops::Deref for ClaimedWaitSet<'_> {
    type Target = HyphaWaitSet;

    fn deref(&self) -> &HyphaWaitSet {
        self.set
    }
}

/// The `WaitSet` behind `waitset`, claimed by the calling thread until the result drops.
/// `InvalidArgument` for null, `InvalidState` while another thread is inside a call or a
/// run on it.
///
/// # Safety
/// `waitset` must be null or a live `WaitSet` handle, live for `'a`.
pub(crate) unsafe fn waitset_here<'a>(
    waitset: *const HyphaWaitSet,
) -> Result<ClaimedWaitSet<'a>, HyphaStatus> {
    // SAFETY: the caller's contract is this function's.
    let shared = unsafe { shared_of(waitset) }.ok_or(HyphaStatus::InvalidArgument)?;
    let claim = shared.claim().map_err(HyphaStatus::from)?;
    // SAFETY: non-null and live by the caller; the claim keeps every other thread off it,
    // which reaches only `shared` meanwhile, until this result drops.
    let set = unsafe { &*waitset };
    Ok(ClaimedWaitSet { set, _claim: claim })
}
