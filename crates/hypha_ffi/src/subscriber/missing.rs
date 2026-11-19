//! The C ABI wrappers of `hypha_node::subscriber::missing`.

use crate::boundary::{handle_ref, write_out};
use crate::status::try_guard;
use hypha_node::safe::HyphaSubscriber;
use hypha_node::status::HyphaStatus;

/// Write the cumulative count of frames this subscriber lost before it read them (the
/// sequence values skipped between accepted frames, per publisher) to `out`.
/// `InvalidArgument`, with `out` unwritten, when `subscriber` or `out` is null.
///
/// # Safety
/// `subscriber` is null or live. `out` is null or one writable `uint64_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_subscriber_missing_sequences(
    subscriber: *const HyphaSubscriber,
    out: *mut u64,
) -> HyphaStatus {
    try_guard(|| {
        let missing = unsafe { handle_ref(subscriber) }?.missing_sequences();
        unsafe { write_out(out, missing) }
    })
}
