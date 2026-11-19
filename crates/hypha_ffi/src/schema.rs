//! The C ABI wrappers of `hypha_node::schema`.

use crate::boundary::{borrow_str, handle_ref, write_bytes, write_out};
use crate::status::try_guard;
use hypha_node::safe::{HyphaNode, schema_lookup};
use hypha_node::status::HyphaStatus;
use std::os::raw::c_char;

/// Resolves the message definition of `(topic, data_type, md5)` into `definition`, with
/// no trailing NUL. A null `definition` with zero capacity writes the size to
/// `definition_len` and copies nothing; a short buffer returns `TooLarge`.
/// `NotAvailable`: a missing schema. `InvalidSchema`: a mismatched or invalid one.
/// `InvalidState`: called from a dispatcher callback, unless the node already holds the
/// definition, since it waits for the publisher's answer.
/// # Safety
/// All pointers must be valid for their specified lengths.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_schema_lookup(
    node: *const HyphaNode,
    topic: *const c_char,
    topic_len: usize,
    data_type: *const c_char,
    data_type_len: usize,
    md5: *const c_char,
    md5_len: usize,
    definition: *mut c_char,
    definition_capacity: usize,
    definition_len: *mut usize,
) -> HyphaStatus {
    try_guard(|| {
        let hypha_node = unsafe { handle_ref(node) }?;
        if definition_len.is_null() || (definition.is_null() && definition_capacity != 0) {
            return Err(HyphaStatus::InvalidArgument);
        }
        let topic_str = unsafe { borrow_str(topic, topic_len) }?;
        let data_type_str = unsafe { borrow_str(data_type, data_type_len) }?;
        let md5_str = unsafe { borrow_str(md5, md5_len) }?;
        let def_text = schema_lookup(hypha_node, topic_str, data_type_str, md5_str)?;
        unsafe { write_out(definition_len, def_text.len()) }?;
        if definition.is_null() {
            return Ok(());
        }
        unsafe { write_bytes(def_text.as_bytes(), definition, definition_capacity) }
    })
}
