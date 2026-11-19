//! The C ABI wrappers of `hypha_node::node`.

use crate::boundary::{borrow_str, handle_ref, write_out, write_text};
use crate::status::try_guard;
use hypha_node::safe::{HyphaNode, node_create, node_destroy, node_name, node_ok, node_shutdown};
use hypha_node::status::HyphaStatus;
use std::os::raw::c_char;
use std::sync::Arc;

/// Write the name of `node` (`node.name` when the parameter file sets it) to `out`.
/// `TooLarge` when `capacity` is too small; `out_len` then holds the length, and no
/// byte is written.
///
/// # Safety
/// `node` is live. `out` is null or `capacity` writable bytes. `out_len` points to a
/// writable `size_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_node_name(
    node: *const HyphaNode,
    out: *mut c_char,
    capacity: usize,
    out_len: *mut usize,
) -> HyphaStatus {
    try_guard(|| {
        let session = unsafe { handle_ref(node) }?;
        unsafe { write_text(node_name(session), out, capacity, out_len) }
    })
}

/// Create a node called `name`: `[a-z][a-z0-9_]`, at most 64 bytes, that no live node of
/// another process in the domain holds (`InvalidState`), or a CLI tool name `_hyphacli_`
/// plus a node name (64 bytes in all), which any number of processes may hold. A
/// non-null `config` (still owned by the caller) supplies the parameters; a nonzero
/// `parameter_file_len` must then name its file (`InvalidArgument` otherwise). A null
/// `config` loads `parameter_file`, or none when the length is 0. `Backend`: the dead
/// run of its own name cannot be reclaimed.
/// # Safety
/// `name` points to `name_len` bytes, `parameter_file` to `parameter_file_len` bytes when
/// nonzero; `config` is null or live; `out` points to one writable pointer. The caller
/// releases the node with `hypha_node_destroy`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_node_create(
    name: *const c_char,
    name_len: usize,
    parameter_file: *const c_char,
    parameter_file_len: usize,
    config: *const hypha_node::config::HyphaConfig,
    out: *mut *mut HyphaNode,
) -> HyphaStatus {
    // Installed before the arguments are read, so a refusal of one reaches the log.
    hypha_log::install();
    try_guard(|| {
        unsafe { write_out(out, std::ptr::null_mut()) }?;
        let code_name = unsafe { borrow_str(name, name_len) }.map_err(refuse_text)?;
        let parameter_file = if parameter_file_len == 0 {
            None
        } else {
            Some(unsafe { borrow_str(parameter_file, parameter_file_len) }.map_err(refuse_text)?)
        };
        let prepared = if config.is_null() { None } else { Some(unsafe { handle_ref(config) }?) };
        let node = node_create(code_name, parameter_file, prepared)?;
        unsafe { write_out(out, Arc::into_raw(node).cast_mut()) }
    })
}

/// Release a node. Passing null is allowed and does nothing.
///
/// # Safety
/// `node` must come from `hypha_node_create` and must not be used afterwards.
/// Every endpoint made from it must be released first.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_node_destroy(node: *mut HyphaNode) {
    if node.is_null() {
        return;
    }
    // SAFETY: the handle is the owner `hypha_node_create` handed out, used no more.
    let owned = unsafe { Arc::from_raw(node.cast_const()) };
    node_destroy(&owned);
    drop(owned);
}

/// Non-zero while the node has not been asked to stop, by `hypha_node_shutdown` or by
/// SIGINT or SIGTERM, which stop every node of the process for good.
///
/// # Safety
/// `node` must come from `hypha_node_create`, or be null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_node_ok(node: *const HyphaNode) -> i32 {
    unsafe { handle_ref(node) }.map_or(0, |session| i32::from(node_ok(session)))
}

/// Ask the node to stop: `hypha_node_ok` then returns 0 for it. Null does nothing.
///
/// # Safety
/// `node` must come from `hypha_node_create`, or be null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_node_shutdown(node: *mut HyphaNode) {
    if let Ok(session) = unsafe { handle_ref(node) } {
        node_shutdown(session);
    }
}

fn refuse_text(status: HyphaStatus) -> HyphaStatus {
    ::log::error!(target: "hypha", "hypha: node creation refused: the node name or parameter file is not valid text");
    status
}
