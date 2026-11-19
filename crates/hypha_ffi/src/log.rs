//! The C ABI wrappers of `hypha_node::log`.

use crate::boundary::{borrow_bytes, borrow_str, require_non_null, write_out};
use crate::status::try_guard;
use hypha_log::{Level, ModuleId};
use hypha_node::HyphaLogModule;
use hypha_node::status::HyphaStatus;
use std::os::raw::c_char;

/// Register a module by name; the same name gives the same identifier. Once 256 modules
/// (`default` included) exist, a new name gives `HYPHA_STATUS_OK` and the identifier of
/// `default` (0). A name has 1 to 64 bytes from `A-Z`, `a-z`, `0-9`, `_` and `/`.
/// `InvalidArgument` for a null `out` or an invalid name.
///
/// # Safety
/// `name` holds `name_len` bytes. `out` points to writable storage for one identifier.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_log_module_register(
    name: *const c_char,
    name_len: usize,
    out: *mut HyphaLogModule,
) -> HyphaStatus {
    try_guard(|| {
        require_non_null(out)?;
        let name = unsafe { borrow_str(name, name_len) }?;
        match hypha_log::module_or_default(name) {
            Ok(ModuleId(id)) => unsafe { write_out(out, id) },
            Err(_) => Err(HyphaStatus::InvalidArgument),
        }
    })
}

/// One when a line at `level` (a [`HyphaLogLevel`] value) reaches at least one target,
/// otherwise zero, as for an unknown module or an invalid level. Lock-free.
#[unsafe(no_mangle)]
pub extern "C" fn hypha_log_enabled(module: HyphaLogModule, level: u32) -> i32 {
    let Some(level) = Level::from_raw(level) else {
        return 0;
    };
    i32::from(hypha_log::enabled(ModuleId(module), level))
}

/// Write one line at `level`, a [`HyphaLogLevel`] value. No status: a rejected line (an
/// unknown module, an invalid level, the level off, or a null `text` with a length above
/// zero) is counted. The text need not be UTF-8; a message above 16 KiB is cut.
///
/// # Safety
/// `text` must be null, or point to `text_len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_log_write(
    module: HyphaLogModule,
    level: u32,
    text: *const c_char,
    text_len: usize,
) {
    let bytes: &[u8] = if text_len == 0 {
        &[]
    } else if let Ok(bytes) = unsafe { borrow_bytes(text.cast::<u8>(), text_len) } {
        bytes
    } else {
        hypha_log::count_rejected_line();
        return;
    };
    let Some(level) = Level::from_raw(level) else {
        hypha_log::count_rejected_line();
        return;
    };
    hypha_log::write(ModuleId(module), level, bytes);
}
