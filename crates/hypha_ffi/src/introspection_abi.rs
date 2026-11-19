//! The C ABI wrappers of `hypha_node::introspection_abi`.

use crate::boundary::{borrow_bytes_mut, borrow_cstr, handle_ref, write_out};
use crate::status::try_guard;
use hypha_core::message::EncodeInto as _;
use hypha_node::safe::{
    HyphaConfig, HyphaNode, config_record_config, node_introspect, node_record_config,
};
use hypha_node::status::HyphaStatus;
use std::os::raw::c_char;

/// Record one configuration row of `node`; a repeated `key` replaces its row. A null
/// `default_text` means no default. `source` is a `HYPHA_CONFIG_SOURCE_*` value.
///
/// # Safety
/// `node` must be a live node. `key` and `value_text` must be NUL-terminated
/// strings. `default_text` must be null or a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_node_record_config(
    node: *mut HyphaNode,
    key: *const c_char,
    value_text: *const c_char,
    default_text: *const c_char,
    source: u32,
) -> HyphaStatus {
    try_guard(|| {
        let session = unsafe { handle_ref(node) }?;
        let key = unsafe { borrow_cstr(key) }?;
        let value = unsafe { borrow_cstr(value_text) }?;
        let default =
            if default_text.is_null() { None } else { Some(unsafe { borrow_cstr(default_text) }?) };
        node_record_config(session, key, value, default, source)?;
        Ok(())
    })
}

/// Record a configuration row before node creation, copied into each node created from
/// `config`. As `hypha_node_record_config` otherwise.
///
/// # Safety
/// `config` must be live. `key`, `value_text` and a non-null `default_text` must
/// be NUL-terminated UTF-8 strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_config_record_config(
    config: *const HyphaConfig,
    key: *const c_char,
    value_text: *const c_char,
    default_text: *const c_char,
    source: u32,
) -> HyphaStatus {
    try_guard(|| {
        let config = unsafe { handle_ref(config) }?;
        let key = unsafe { borrow_cstr(key) }?;
        let value = unsafe { borrow_cstr(value_text) }?;
        let default =
            if default_text.is_null() { None } else { Some(unsafe { borrow_cstr(default_text) }?) };
        config_record_config(config, key, value, default, source)?;
        Ok(())
    })
}

/// Copy a snapshot of `node`, taken now, into `out` as a ROS 1 serialized
/// `hypha_msgs/NodeIntrospection`.
///
/// `out_len` receives the snapshot length. When `capacity` is smaller, nothing is copied
/// and `TooLarge` is returned; a retry takes a new snapshot, which may have grown. `out`
/// may be null only when `capacity` is 0. The caller owns `out`.
/// # Safety
/// `node` must be a live node. `out` must be null or point to `capacity`
/// writable bytes. `out_len` must point to a writable `size_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_node_introspect(
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
        let message = node_introspect(session);
        let length = message.encoded_len();
        unsafe { write_out(out_len, length) }?;
        if length > capacity {
            return Err(HyphaStatus::TooLarge);
        }
        let buffer = unsafe { borrow_bytes_mut(out, length) }?;
        message.encode_into(buffer).map(drop).map_err(|_| HyphaStatus::Backend)
    })
}
