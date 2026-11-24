//! The C ABI wrappers of `hypha_node::config`.

use crate::boundary::{borrow_str, handle_ref, require_non_null, take_box, write_out, write_text};
use crate::status::try_guard;
use hypha_node::safe::{
    HyphaConfig, HyphaNode, config_create, config_ignore, config_resolve_key, config_unread_keys,
    node_config,
};
use hypha_node::status::HyphaStatus;
use std::os::raw::c_char;
use std::ptr;

/// Load a parameter file without joining the bus; a zero `parameter_file_len` creates
/// an empty configuration. On success the caller owns `*out` and releases it with
/// `hypha_config_destroy`.
///
/// # Safety
/// `parameter_file` holds `parameter_file_len` bytes when that is nonzero. `out` points
/// to one writable pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_config_create(
    parameter_file: *const c_char,
    parameter_file_len: usize,
    out: *mut *mut HyphaConfig,
) -> HyphaStatus {
    try_guard(|| {
        unsafe { write_out(out, ptr::null_mut()) }?;
        let parameter_file = if parameter_file_len == 0 {
            None
        } else {
            Some(unsafe { borrow_str(parameter_file, parameter_file_len) }?)
        };
        let config = Box::new(config_create(parameter_file)?);
        unsafe { write_out(out, Box::into_raw(config)) }
    })
}

/// Release a configuration handle. Passing null is allowed and does nothing.
///
/// # Safety
/// `config` must come from `hypha_config_create` and must not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_config_destroy(config: *mut HyphaConfig) {
    drop(unsafe { take_box(config) });
}

/// Borrow the configuration of `node`. The pointer lives as long as `node` and
/// must not be passed to `hypha_config_destroy`. Returns null when `node` is null.
///
/// # Safety
/// `node` must come from `hypha_node_create`, or be null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_node_config(node: *const HyphaNode) -> *const HyphaConfig {
    unsafe { handle_ref(node) }
        .map_or(ptr::null(), |session| std::ptr::from_ref(node_config(session)))
}

/// Write the unread keys of `config` under `prefix` (all for a zero `prefix_len`),
/// joined by `'\n'`, to `out`; `node.name` and the keys `hypha_config_ignore` or the
/// file's `unused` list ignore are excluded. `TooLarge` when `out_capacity`
/// is too small; `out_len` then holds the byte length.
///
/// # Safety
/// `config` is live. `prefix` holds `prefix_len` bytes when that is nonzero. `out` is
/// null or `out_capacity` writable bytes. `out_len` points to a writable `usize`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_config_unread_keys(
    config: *const HyphaConfig,
    prefix: *const c_char,
    prefix_len: usize,
    out: *mut c_char,
    out_capacity: usize,
    out_len: *mut usize,
) -> HyphaStatus {
    try_guard(|| {
        let handle = unsafe { handle_ref(config) }?;
        let prefix_str =
            if prefix_len == 0 { "" } else { unsafe { borrow_str(prefix, prefix_len) }? };
        let joined = config_unread_keys(handle, prefix_str);
        unsafe { write_text(&joined, out, out_capacity, out_len) }
    })
}

/// Mark `key`, a key, a table or a list entry (`sensors[0].rate`), as set on purpose and
/// not read, for `reason`: it and every key under it leave `hypha_config_unread_keys`, and
/// it becomes a node manifest row with source `IntrospectedConfig::SOURCE_IGNORED` whose
/// value is `reason`, logged once at info. A key `config` does not set is accepted; a
/// second call for the same key keeps the first reason and returns `Ok`. Both texts are
/// UTF-8, read for the call only and copied; nothing is returned to free.
///
/// `InvalidArgument`: a null `config`, a zero `key_len` or `reason_len`, a null text with
/// a nonzero length, or text that is not UTF-8.
///
/// # Safety
/// `config` is live or null. `key` holds `key_len` bytes and `reason` holds `reason_len`
/// bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_config_ignore(
    config: *const HyphaConfig,
    key: *const c_char,
    key_len: usize,
    reason: *const c_char,
    reason_len: usize,
) -> HyphaStatus {
    try_guard(|| {
        // SAFETY: the caller passes a live configuration or null, which is refused.
        let handle = unsafe { handle_ref(config) }?;
        // SAFETY: the caller passes `key_len` readable bytes at `key`.
        let key = unsafe { borrow_str(key, key_len) }?;
        // SAFETY: the caller passes `reason_len` readable bytes at `reason`.
        let reason = unsafe { borrow_str(reason, reason_len) }?;
        config_ignore(handle, key, reason)?;
        Ok(())
    })
}

/// Resolve the file name that the text parameter `key` holds against the directory of
/// the file that wrote `key`, and write it to `out`. An absolute name, or any name when
/// `config` was loaded from no file, stays as it is. Marks `key` as read.
///
/// `InvalidArgument`: a null `config` or `out_len`, or `key` names no text value or an
/// empty one. `TooLarge`: `out_capacity` is too small; `out_len` then holds the byte
/// length. `out` may be null only with `out_capacity` 0.
///
/// # Safety
/// `config` is live. `key` holds `key_len` bytes. `out` is null or `out_capacity`
/// writable bytes. `out_len` points to a writable `usize`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_config_resolve_key(
    config: *const HyphaConfig,
    key: *const c_char,
    key_len: usize,
    out: *mut c_char,
    out_capacity: usize,
    out_len: *mut usize,
) -> HyphaStatus {
    try_guard(|| {
        let handle = unsafe { handle_ref(config) }?;
        let key = unsafe { borrow_str(key, key_len) }?;
        require_non_null(out_len)?;
        let text = config_resolve_key(handle, key)?;
        unsafe { write_text(&text, out, out_capacity, out_len) }
    })
}
