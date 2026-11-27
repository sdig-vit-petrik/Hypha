//! The C ABI wrappers of `hypha_node::last_error`.

use crate::boundary::write_text;
use crate::status::try_guard;
use hypha_node::status::HyphaStatus;
use std::os::raw::c_char;

/// Copy the reason of the last refusal on this thread to `out`, UTF-8 without a
/// terminator; empty when none was recorded since the last `hypha_cycle_create` or
/// `hypha_cycle_commit` on this thread. `hypha_cycle_set_clock_driver` records the reason
/// of a start value it refuses. `*out_len` receives the length; `TooLarge`, with
/// `out` untouched, when `capacity` is too small. `InvalidArgument` for a null `out_len`.
///
/// # Safety
/// `out` is null or `capacity` writable bytes. `out_len` points to a writable `size_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_last_error(
    out: *mut c_char,
    capacity: usize,
    out_len: *mut usize,
) -> HyphaStatus {
    let text = hypha_node::last_error::last_error();
    try_guard(|| unsafe { write_text(&text, out, capacity, out_len) })
}
