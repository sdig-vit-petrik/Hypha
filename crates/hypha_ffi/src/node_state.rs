//! The C ABI wrappers of `hypha_node::node_state`.

use crate::boundary::{borrow_bytes_mut, handle_ref, write_out};
use crate::status::try_guard;
use hypha_core::message::EncodeInto as _;
use hypha_node::safe::{HyphaNode, node_state};
use hypha_node::status::HyphaStatus;

/// Judge the required inputs of `node` now and copy its state into `out` as a ROS 1
/// serialized `hypha_msgs/NodeState`. It logs and changes nothing.
///
/// `out_len` receives the length. When `capacity` is smaller, nothing is copied and
/// `TooLarge` is returned. `out` may be null only when `capacity` is 0. The caller owns
/// `out`.
///
/// # Safety
/// `node` is live. `out` is null or `capacity` writable bytes. `out_len` points to a
/// writable `size_t`. Call it on the thread that runs the node, or while none does.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_node_state(
    node: *const HyphaNode,
    out: *mut u8,
    capacity: usize,
    out_len: *mut usize,
) -> HyphaStatus {
    try_guard(|| {
        let session = unsafe { handle_ref(node) }?;
        if out_len.is_null() || (out.is_null() && capacity > 0) {
            return Err(HyphaStatus::InvalidArgument);
        }
        let state = node_state(session);
        let length = state.encoded_len();
        unsafe { write_out(out_len, length) }?;
        if length > capacity {
            return Err(HyphaStatus::TooLarge);
        }
        let buffer = unsafe { borrow_bytes_mut(out, length) }?;
        state.encode_into(buffer).map(drop).map_err(|_| HyphaStatus::Backend)
    })
}
