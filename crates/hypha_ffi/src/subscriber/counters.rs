//! The C ABI wrapper of `hypha_node::subscriber::subscriber_counters`.

use crate::boundary::{handle_ref, require_non_null, write_out};
use crate::status::try_guard;
use hypha_node::HyphaSubscriberCounters;
use hypha_node::safe::{HyphaSubscriber, subscriber_counters};
use hypha_node::status::HyphaStatus;

/// Write the eight counters of `subscriber` to `*out`, the values of the subscriber
/// metrics of the same names. Each is read atomically, the eight not at one instant. Any
/// thread may call it. `InvalidArgument`, with `*out` unwritten, when `subscriber` or
/// `out` is null.
///
/// # Safety
/// `subscriber` is null or live for the call. `out` is null or one writable
/// `HyphaSubscriberCounters`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_subscriber_counters(
    subscriber: *const HyphaSubscriber,
    out: *mut HyphaSubscriberCounters,
) -> HyphaStatus {
    try_guard(|| {
        require_non_null(out)?;
        // SAFETY: `subscriber` is null or live by the caller's contract.
        let endpoint = unsafe { handle_ref(subscriber) }?;
        let counters = subscriber_counters(endpoint);
        // SAFETY: `out` is non-null (checked above) and writable by the caller's contract.
        unsafe { write_out(out, counters) }
    })
}
