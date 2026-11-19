//! The C ABI wrappers of `hypha_node::param_topic`.

use crate::boundary::{borrow_str, handle_ref, require_non_null, write_out, write_text};
use crate::status::try_guard;
use hypha_node::safe::{HyphaNode, param_topic};
use hypha_node::status::HyphaStatus;
use std::os::raw::c_char;

/// Read the topic of port `name`, as written, from the parameter file of `node` into
/// `out`, and record it as a configuration row. `direction` is a `HYPHA_PORT_*` value.
/// `rule` `HYPHA_PORT_TOPIC_REQUIRED` refuses an unset topic with `NotAvailable` and
/// `topic = ""` with `InvalidArgument`; `HYPHA_PORT_TOPIC_OPTIONAL` gives length 0 for
/// both. `InvalidArgument`: another direction or rule, an empty name or an entry that is
/// not a table with a text `topic`. `InvalidTopic`: an invalid topic. `TooLarge`: a
/// short buffer.
///
/// # Safety
/// `node` is live. `name` holds `name_len` bytes. `out` is null or `out_capacity`
/// writable bytes. `out_len` points to a writable `size_t`.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn hypha_param_topic(
    node: *const HyphaNode,
    direction: u32,
    name: *const c_char,
    name_len: usize,
    rule: u32,
    out: *mut c_char,
    out_capacity: usize,
    out_len: *mut usize,
) -> HyphaStatus {
    try_guard(|| {
        let session = unsafe { handle_ref(node) }?;
        require_non_null(out_len)?;
        let name_str = unsafe { borrow_str(name, name_len) }?;
        let topic = match param_topic(session, direction, name_str, rule) {
            Ok(topic) => topic,
            Err(error) => {
                unsafe { write_out(out_len, 0) }?;
                return Err(HyphaStatus::from(error));
            }
        };
        unsafe { write_text(&topic, out, out_capacity, out_len) }
    })
}
