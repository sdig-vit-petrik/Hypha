//! The C ABI wrappers of `hypha_node::waitset::registrations`.

use super::access::waitset_here;
use crate::callbacks::CCallbacks;
use hypha_node::safe::{HyphaWaitSet, waitset_attach_foreign, waitset_detach};
use hypha_node::status::{Error, HyphaStatus};
use hypha_node::{HyphaAttachment, HyphaAttachmentId};
use std::sync::Arc;

/// Attach a registration of any kind (see `HyphaAttachment`) and write its id to `out`.
///
/// `InvalidArgument`: a `kind` other than `HYPHA_ATTACH_SUBSCRIPTION` or
/// `HYPHA_ATTACH_TIMER`, a missing callback, or an uncleared field the kind does not
/// read. Success transfers `context` and `release`, released once at the detach or the
/// node's destruction; a refused attach transfers nothing. `InvalidState`: during a run,
/// or a subscriber another registration dispatches.
///
/// During a run a callback, on the run's thread, may attach a `HYPHA_ATTACH_TIMER` when
/// the node runs on host time or on its own simulated clock (`use_sim_time`): it joins
/// the dispatch before the run's next wait, and its first call comes one period after
/// the attach. Its id lies above every id a registration attached between runs takes.
/// The attach is `InvalidState` from any other thread, for any other kind, and for a
/// timer on a simulated clock driven from outside the node, which needs a native poll.
/// # Safety
/// `waitset` and `attachment` are live; `out` is writable. The subscriber of
/// `attachment` is null or a live handle from `hypha_subscribe_typed`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_waitset_attach(
    waitset: *mut HyphaWaitSet,
    attachment: *const HyphaAttachment,
    out: *mut HyphaAttachmentId,
) -> HyphaStatus {
    let refused: bool = waitset.is_null() || attachment.is_null() || out.is_null();
    if refused {
        return HyphaStatus::InvalidArgument;
    }
    unsafe { out.write(0) };

    let supplied = unsafe { *attachment };
    // The record is checked before the `WaitSet` is reached. The subscriber handle is an
    // `Arc`; the registration takes one more owner, which a refused attach drops.
    // SAFETY: the `WaitSet` is live by the caller's contract.
    let reach = || unsafe { waitset_here(waitset) }.map_err(|_| Error::InvalidState);
    let attached = waitset_attach_foreign::<CCallbacks>(reach, &supplied, |subscriber| unsafe {
        Arc::increment_strong_count(subscriber);
        Arc::from_raw(subscriber.cast_const())
    });
    match attached {
        Ok(id) => {
            unsafe { out.write(id) };
            HyphaStatus::Ok
        }
        Err(error) => HyphaStatus::from(error),
    }
}

/// Remove a registration and release its context.
///
/// From a callback of this `WaitSet` the registration is never dispatched again, and its
/// context is released when the run returns. From another thread during a run:
/// `InvalidState`. An unknown registration: `InvalidArgument`.
///
/// # Safety
/// `waitset` must be a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_waitset_detach(
    waitset: *mut HyphaWaitSet,
    attachment: HyphaAttachmentId,
) -> HyphaStatus {
    // SAFETY: the `WaitSet` is null or live by the caller's contract.
    let set = match unsafe { waitset_here(waitset) } {
        Ok(set) => set,
        Err(status) => return status,
    };
    hypha_node::status::status_of(waitset_detach(set, attachment))
}
