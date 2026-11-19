//! The C ABI wrappers of `hypha_node::node_discovery`.

use crate::boundary::{borrow_str, handle_ref};
use crate::status::{status_of, try_guard};
use hypha_node::safe::{HyphaNode, ListedNode, ListedServicePort, list_nodes, list_service_ports};
use hypha_node::status::HyphaStatus;
use hypha_node::{
    HyphaNodeCallback, HyphaNodeInfo, HyphaServicePortCallback, HyphaServicePortInfo,
};

/// List every node registered on the bus: `callback` runs once for each, before the
/// call returns. `InvalidArgument`: a null `callback`. `Backend`: the node registry
/// cannot be inspected.
///
/// # Safety
/// `user_data` is passed through to `callback` unexamined.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_list_nodes(
    callback: HyphaNodeCallback,
    user_data: *mut std::os::raw::c_void,
) -> hypha_node::status::HyphaStatus {
    let Some(callback) = callback else {
        return HyphaStatus::InvalidArgument;
    };
    let mut report = |node: &ListedNode<'_>| {
        let info = HyphaNodeInfo {
            name: node.name.as_ptr(),
            executable: node.executable.as_ptr(),
            pid: node.pid,
            alive: i32::from(node.alive),
        };
        callback(&raw const info, user_data);
    };
    status_of(list_nodes(&mut report))
}

/// Checks that `topic` is a canonical public topic name. `InvalidTopic`: a name the
/// transport would refuse. `InvalidArgument`: a null or non-UTF-8 input.
///
/// # Safety
/// `topic` must point to `topic_len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_topic_validate_public_name(
    topic: *const std::os::raw::c_char,
    topic_len: usize,
) -> HyphaStatus {
    try_guard(|| {
        let topic_str = unsafe { borrow_str(topic, topic_len) }?;
        if !hypha_core::port::is_valid_topic_name(topic_str) {
            return Err(HyphaStatus::InvalidTopic);
        }
        Ok(())
    })
}

/// List every data service and its active endpoint counts. A topic is reported without
/// its leading `/`. `InvalidFrame`: a service name could not be reported, so the listing
/// is incomplete.
///
/// # Safety
/// `node` must remain valid for the call. `user_data` is passed through to `callback`
/// unexamined and may be null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_list_service_ports(
    node: *const HyphaNode,
    callback: HyphaServicePortCallback,
    user_data: *mut std::os::raw::c_void,
) -> HyphaStatus {
    try_guard(|| {
        let session = unsafe { handle_ref(node) }?;
        let callback = callback.ok_or(HyphaStatus::InvalidArgument)?;
        let mut report = |service: &ListedServicePort<'_>| {
            let info = HyphaServicePortInfo {
                name: service.name.as_ptr(),
                publishers: service.publishers,
                subscribers: service.subscribers,
            };
            callback(&raw const info, user_data);
        };
        list_service_ports(session, &mut report)?;
        Ok(())
    })
}
