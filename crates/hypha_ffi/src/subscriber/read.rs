//! The C ABI wrappers of `hypha_node::subscriber::read`.

use crate::boundary::{handle_ref, write_text};
use crate::status::try_guard;
use hypha_node::safe::{HyphaSubscriber, subscriber_topic};
use hypha_node::status::HyphaStatus;
use std::os::raw::c_char;

/// Write the topic `subscriber` resolved at creation, and its length to `out_len`.
/// `TooLarge` when `capacity` is too small; `out_len` then holds the length.
///
/// # Safety
/// `subscriber` is null or live. `out` is null or `capacity` writable bytes. `out_len`
/// points to a writable `size_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_subscriber_topic(
    subscriber: *const HyphaSubscriber,
    out: *mut c_char,
    capacity: usize,
    out_len: *mut usize,
) -> HyphaStatus {
    try_guard(|| {
        let endpoint = unsafe { handle_ref(subscriber) }?;
        unsafe { write_text(subscriber_topic(endpoint), out, capacity, out_len) }
    })
}

/// Release the caller's subscriber handle; a `WaitSet` registration that shares it keeps
/// it alive until the detach. Null does nothing.
///
/// # Safety
/// `subscriber` must come from `hypha_subscribe_typed` and must not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_subscriber_destroy(subscriber: *mut HyphaSubscriber) {
    if subscriber.is_null() {
        return;
    }
    // SAFETY: the handle is one `Arc` owner, used no more by the caller.
    drop(unsafe { std::sync::Arc::from_raw(subscriber.cast_const()) });
}
