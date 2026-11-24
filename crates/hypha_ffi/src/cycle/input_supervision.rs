//! The C ABI wrappers of `hypha_node::cycle::input_supervision`.

use super::{CycleTarget, HyphaCycle};
use crate::boundary::{borrow_str, handle_ref, read_in, write_out, write_text};
use crate::status::try_guard;
use hypha_node::safe::cycle_bind_input_supervision;
use hypha_node::status::HyphaStatus;
use hypha_node::{HyphaInputSupervisionRequest, HyphaInputSupervisionResult};
use std::os::raw::c_char;

/// Check, read and attach the supervision of input `binding_index` in one call.
///
/// A disabled input reads its keys and attaches nothing. `out` receives what attached,
/// and a refusal writes its cause to `detail` (`detail_capacity` bytes; `detail_len`
/// receives the length; a short buffer returns `TooLarge`). `InvalidState`: the cycle is
/// committed or its `WaitSet` runs.
/// # Safety
/// `cycle` is live; `request`, `out` and `detail_len` are non-null; the request's `node`
/// is live and `name` holds `name_len` bytes. `detail` is null or `detail_capacity`
/// writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_cycle_bind_input_supervision(
    cycle: *mut HyphaCycle,
    binding_index: u32,
    request: *const HyphaInputSupervisionRequest,
    out: *mut HyphaInputSupervisionResult,
    detail: *mut c_char,
    detail_capacity: usize,
    detail_len: *mut usize,
) -> HyphaStatus {
    try_guard(|| {
        let cycle = unsafe { CycleTarget::of(cycle) }?;
        let request = unsafe { read_in(request) }?;
        let session = unsafe { handle_ref(request.node) }?;
        unsafe {
            write_out(out, HyphaInputSupervisionResult::default())?;
            write_out(detail_len, 0)?;
        }
        let port_name = unsafe { borrow_str(request.name, request.name_len) }?;
        let (set, cycle) = unsafe { cycle.waitset() }?;
        let bound =
            cycle_bind_input_supervision(&set, cycle, session, &request, port_name, binding_index);
        let (status, detail_text, result) = match bound {
            Ok(Some(result)) => (Ok(()), String::new(), Some(result)),
            Ok(None) => (Ok(()), String::new(), None),
            Err((error, text)) => (Err(HyphaStatus::from(error)), text, None),
        };
        unsafe { write_text(&detail_text, detail, detail_capacity, detail_len) }?;
        if let Some(result) = result {
            unsafe { write_out(out, result) }?;
        }
        status
    })
}
