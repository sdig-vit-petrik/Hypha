//! The C ABI wrappers of `hypha_node::cycle::source`.

use crate::boundary::{handle_ref, require_non_null, take_box, write_out};
use crate::status::try_guard;
use crate::waitset::access::waitset_here;
use hypha_node::safe::{
    HyphaInputSource, HyphaSubscriber, HyphaWaitSet, input_source_create, input_source_destroy,
};
use hypha_node::status::HyphaStatus;
use std::sync::Arc;

/// Open a managed receiver that stores accepted message bytes in bounded shared
/// history. Success transfers `subscriber`; failure leaves it to the caller. A null
/// `subscriber` makes an unwired input, which captures as `NeverArrived`.
///
/// `InvalidArgument`: null `waitset` or `out`, or a subscriber of another node.
/// `InvalidOperation`: the subscriber is attached to a `WaitSet`, was read by an owning
/// receive, or belongs to another input source. `InvalidState`: the `WaitSet` runs.
/// `ResourceLimit`: no room for another source. The caller destroys the handle with
/// `hypha_input_source_destroy`.
///
/// # Safety
/// `waitset` must be live, `subscriber` null or live, and `out` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_input_source_create(
    waitset: *mut HyphaWaitSet,
    subscriber: *mut HyphaSubscriber,
    out: *mut *mut HyphaInputSource,
) -> HyphaStatus {
    try_guard(|| {
        require_non_null(waitset)?;
        unsafe { write_out(out, std::ptr::null_mut()) }?;
        let set = unsafe { waitset_here(waitset) }?;
        // SAFETY: a non-null handle is one `Arc` owner of the subscriber; a refusal hands
        // it back below.
        let offered =
            (!subscriber.is_null()).then(|| unsafe { Arc::from_raw(subscriber.cast_const()) });
        match input_source_create(set, offered) {
            Ok(source) => unsafe { write_out(out, Box::into_raw(source)) },
            Err((error, refused)) => {
                let _caller_keeps = refused.map(Arc::into_raw);
                Err(HyphaStatus::from(error))
            }
        }
    })
}

/// Release the source handle. A cycle that binds the source keeps its subscriber and
/// history alive until that cycle is destroyed, so the handle of a bound source is
/// released at any time. For a source no cycle binds, a running `WaitSet` refuses with
/// `InvalidState` and keeps the handle valid.
///
/// # Safety
/// `source` must be a live source handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_input_source_destroy(source: *mut HyphaInputSource) -> HyphaStatus {
    try_guard(|| {
        unsafe { handle_ref(source) }?;
        let owned = unsafe { take_box(source) }.ok_or(HyphaStatus::InvalidArgument)?;
        match input_source_destroy(owned) {
            Ok(()) => Ok(()),
            Err((error, refused)) => {
                let _caller_keeps = Box::into_raw(refused);
                Err(HyphaStatus::from(error))
            }
        }
    })
}
