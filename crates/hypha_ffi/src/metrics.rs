//! The C ABI wrappers of `hypha_node::metrics`.

use crate::boundary::handle_ref;
use hypha_node::HyphaMetricCallback;
use hypha_node::safe::{HyphaNode, node_metrics};
use hypha_node::status::HyphaStatus;
use std::os::raw::{c_char, c_void};

/// Emit the metrics of `node`, once each, before the call returns. A null `node` emits
/// only the process-wide log counters. A null `callback` returns `InvalidArgument` and
/// calls nothing. `user_data` is passed to every callback unchanged and may be null;
/// the library keeps no copy of either.
///
/// # Safety
/// `node` must be null or come from `hypha_node_create`. `callback` must be
/// safe to call from this thread with `user_data`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_node_metrics(
    node: *const HyphaNode,
    callback: HyphaMetricCallback,
    user_data: *mut c_void,
) -> HyphaStatus {
    let Some(callback) = callback else {
        return HyphaStatus::InvalidArgument;
    };
    let mut emit = deliver(callback, user_data);
    node_metrics(unsafe { handle_ref(node) }.ok(), &mut emit);
    HyphaStatus::Ok
}

fn deliver(
    callback: extern "C" fn(*const c_char, usize, u64, *mut c_void),
    user_data: *mut c_void,
) -> impl FnMut(&str, u64) {
    move |name, value| callback(name.as_ptr().cast(), name.len(), value, user_data)
}
