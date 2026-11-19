//! The exact ABI revision exported for SDK startup checks.

include!(concat!(env!("OUT_DIR"), "/abi_revision.rs"));

/// Returns the ABI revision compiled into this shared library.
///
/// C and C++ clients must compare this exactly with `HYPHA_ABI_REVISION` before any
/// other ABI call; no other call checks the caller's revision.
#[unsafe(no_mangle)]
pub extern "C" fn hypha_abi_revision() -> u32 {
    ABI_REVISION
}
