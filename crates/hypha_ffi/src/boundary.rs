//! The raw-pointer reads and writes of the C ABI, each with its contract stated once.
//!
//! No entry point dereferences a pointer itself. Each function here refuses a null
//! pointer with `InvalidArgument`; liveness, size and initialisation are the caller's.

use std::os::raw::c_char;

use hypha_node::status::HyphaStatus;

/// `InvalidArgument` for a null `pointer`, for an argument checked before it is used.
#[inline]
pub(crate) fn require_non_null<T>(pointer: *const T) -> Result<(), HyphaStatus> {
    if pointer.is_null() { Err(HyphaStatus::InvalidArgument) } else { Ok(()) }
}

/// The value behind a handle. `InvalidArgument` for null.
///
/// # Safety
/// `handle` must be null or point to a live `T` that nothing writes for `'a`.
pub(crate) unsafe fn handle_ref<'a, T>(handle: *const T) -> Result<&'a T, HyphaStatus> {
    // SAFETY: non-null by the check in `as_ref`; live and unwritten for `'a` by the caller.
    unsafe { handle.as_ref() }.ok_or(HyphaStatus::InvalidArgument)
}

/// The value behind a handle, for writing. `InvalidArgument` for null.
///
/// # Safety
/// `handle` must be null or point to a live `T` that nothing else touches for `'a`.
pub(crate) unsafe fn handle_mut<'a, T>(handle: *mut T) -> Result<&'a mut T, HyphaStatus> {
    // SAFETY: non-null by the check in `as_mut`; live and exclusive for `'a` by the caller.
    unsafe { handle.as_mut() }.ok_or(HyphaStatus::InvalidArgument)
}

/// A `#[repr(C)]` argument the caller passes by pointer, copied. `InvalidArgument` for null.
///
/// # Safety
/// `value` must be null or point to a readable, initialised `T`.
pub(crate) unsafe fn read_in<T: Copy>(value: *const T) -> Result<T, HyphaStatus> {
    // SAFETY: non-null by the check in `as_ref`; readable and initialised by the caller.
    unsafe { value.as_ref() }.copied().ok_or(HyphaStatus::InvalidArgument)
}

/// Write `value` to the caller's `out`. `InvalidArgument` for null, and nothing is written.
///
/// # Safety
/// `out` must be null or point to writable storage for one `T`.
pub(crate) unsafe fn write_out<T>(out: *mut T, value: T) -> Result<(), HyphaStatus> {
    if out.is_null() {
        return Err(HyphaStatus::InvalidArgument);
    }
    // SAFETY: non-null by the check; writable storage for one `T` by the caller.
    unsafe { out.write(value) };
    Ok(())
}

/// Take back a handle that `Box::into_raw` made. `None` for null.
///
/// # Safety
/// `handle` must be null or come from `Box::into_raw`, and must not be used afterwards.
pub(crate) unsafe fn take_box<T>(handle: *mut T) -> Option<Box<T>> {
    if handle.is_null() {
        return None;
    }
    // SAFETY: non-null by the check; from `Box::into_raw` and used no more by the caller.
    Some(unsafe { Box::from_raw(handle) })
}

/// `len` bytes of text. `InvalidArgument` for null or for bytes that are not UTF-8.
///
/// # Safety
/// `text` must be null, or point to at least `len` readable bytes.
pub(crate) unsafe fn borrow_str<'a>(
    text: *const c_char,
    len: usize,
) -> Result<&'a str, HyphaStatus> {
    // SAFETY: `borrow_bytes` carries the same contract.
    let bytes = unsafe { borrow_bytes(text.cast::<u8>(), len) }?;
    std::str::from_utf8(bytes).map_err(|_| HyphaStatus::InvalidArgument)
}

/// `len` bytes. `InvalidArgument` for null, or a length no slice can have.
///
/// # Safety
/// `bytes` must be null, or point to at least `len` readable bytes.
pub(crate) unsafe fn borrow_bytes<'a>(
    bytes: *const u8,
    len: usize,
) -> Result<&'a [u8], HyphaStatus> {
    if bytes.is_null() || len > isize::MAX as usize {
        return Err(HyphaStatus::InvalidArgument);
    }
    // SAFETY: non-null and at most `isize::MAX` by the check; readable by the caller.
    Ok(unsafe { std::slice::from_raw_parts(bytes, len) })
}

/// `len` writable bytes. `InvalidArgument` for null, or a length no slice can have.
///
/// # Safety
/// `bytes` must be null, or point to `len` writable bytes nothing else touches for `'a`.
pub(crate) unsafe fn borrow_bytes_mut<'a>(
    bytes: *mut u8,
    len: usize,
) -> Result<&'a mut [u8], HyphaStatus> {
    if bytes.is_null() || len > isize::MAX as usize {
        return Err(HyphaStatus::InvalidArgument);
    }
    // SAFETY: non-null and at most `isize::MAX` by the check; exclusive by the caller.
    Ok(unsafe { std::slice::from_raw_parts_mut(bytes, len) })
}

/// A NUL-terminated string. `InvalidArgument` for null or for bytes that are not UTF-8.
///
/// # Safety
/// `text` must be null, or point to a NUL-terminated string that stays valid for `'a`.
pub(crate) unsafe fn borrow_cstr<'a>(text: *const c_char) -> Result<&'a str, HyphaStatus> {
    if text.is_null() {
        return Err(HyphaStatus::InvalidArgument);
    }
    // SAFETY: non-null by the check; NUL-terminated and valid for `'a` by the caller.
    unsafe { std::ffi::CStr::from_ptr(text) }.to_str().map_err(|_| HyphaStatus::InvalidArgument)
}

/// Copy `bytes` into the caller's buffer of `capacity` bytes. `TooLarge` when they do
/// not fit, with nothing written; nothing is written for no bytes either. A null `out`
/// with room is `InvalidArgument`.
///
/// # Safety
/// `out` must be null, or point to `capacity` writable bytes.
pub(crate) unsafe fn write_bytes(
    bytes: &[u8],
    out: *mut c_char,
    capacity: usize,
) -> Result<(), HyphaStatus> {
    if capacity < bytes.len() {
        return Err(HyphaStatus::TooLarge);
    }
    if bytes.is_empty() {
        return Ok(());
    }
    if out.is_null() {
        return Err(HyphaStatus::InvalidArgument);
    }
    // SAFETY: non-null, `capacity >= bytes.len()` by the checks; writable by the caller.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), out.cast::<u8>(), bytes.len()) };
    Ok(())
}

/// Copy `text` to `out` and its length to `out_len`. The length is written first, so a
/// caller that gets `TooLarge` can retry with enough room. A null `out` is `TooLarge`.
///
/// # Safety
/// `out_len` is null or a writable `usize`; `out` is null or `capacity` writable bytes.
pub(crate) unsafe fn write_text(
    text: &str,
    out: *mut c_char,
    capacity: usize,
    out_len: *mut usize,
) -> Result<(), HyphaStatus> {
    // SAFETY: both carry the contract of this function.
    unsafe {
        write_out(out_len, text.len())?;
        if out.is_null() && !text.is_empty() {
            return Err(HyphaStatus::TooLarge);
        }
        write_bytes(text.as_bytes(), out, capacity)
    }
}

/// The policy name of `qos`: empty when `qos.name_len` is zero, without a read.
///
/// # Safety
/// `qos.name` must point to `qos.name_len` readable bytes when that is not zero.
pub(crate) unsafe fn qos_name(qos: &hypha_node::qos::HyphaQos) -> Result<&str, HyphaStatus> {
    if qos.name_len == 0 {
        return Ok("");
    }
    // SAFETY: the caller guarantees `name_len` readable bytes at `name`.
    unsafe { borrow_str(qos.name, qos.name_len) }
}
