//! The C ABI wrappers of `hypha_node::waitset::configuration`.

use hypha_node::safe::{HyphaNode, HyphaWaitSet};
use hypha_node::status::HyphaStatus;
use hypha_node::waitset::configuration::with_default_waitset;
use std::sync::Arc;

/// Return this node's `WaitSet`, creating it on first use.
///
/// The node owns it and it stays valid as long as the node; `hypha_node_destroy`
/// releases it with its cycles and every registration context.
///
/// # Safety
/// `node` must be a live node handle. `out` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_node_default_waitset(
    node: *mut HyphaNode,
    out: *mut *mut HyphaWaitSet,
) -> HyphaStatus {
    let invalid_arguments = node.is_null() || out.is_null();
    if invalid_arguments {
        return HyphaStatus::InvalidArgument;
    }
    unsafe { out.write(std::ptr::null_mut()) };
    let shared = unsafe { share_node(node) };
    match with_default_waitset(&shared, |set| std::ptr::from_ref(set).cast_mut()) {
        Ok(waitset) => {
            unsafe { out.write(waitset) };
            HyphaStatus::Ok
        }
        Err(error) => HyphaStatus::from(error),
    }
}

/// Another owner of the node behind `handle`, for a `WaitSet` on it.
///
/// # Safety
/// `handle` must be a live node handle from `hypha_node_create`.
pub(crate) unsafe fn share_node(handle: *const HyphaNode) -> Arc<HyphaNode> {
    // SAFETY: from `Arc::into_raw` in `hypha_node_create` and live by the caller.
    unsafe {
        Arc::increment_strong_count(handle);
        Arc::from_raw(handle)
    }
}
