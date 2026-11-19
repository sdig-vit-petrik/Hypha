//! Attachment records for `hypha_waitset_attach`: the fields a kind reads are set,
//! every other field stays cleared.

use std::os::raw::c_void;

use hypha_ffi::*;

pub(crate) fn record(kind: u32) -> HyphaAttachment {
    // SAFETY: every field accepts all-zero bits.
    let mut attachment: HyphaAttachment = unsafe { std::mem::zeroed() };
    attachment.kind = kind;
    attachment
}

/// A framework-time timer of `period_nanos`.
pub(crate) fn timer(
    period_nanos: u64,
    callback: HyphaIntervalCallback,
    context: *mut c_void,
    release: HyphaContextRelease,
) -> HyphaAttachment {
    let mut attachment = record(HYPHA_ATTACH_TIMER);
    attachment.period_nanos = period_nanos;
    attachment.periodic_callback = callback;
    attachment.context = context;
    attachment.release = release;
    attachment
}

/// A subscription, woken by its publishers' notifications.
pub(crate) fn subscription(
    subscriber: *mut HyphaSubscriber,
    callback: HyphaSubscriptionCallback,
    context: *mut c_void,
    release: HyphaContextRelease,
) -> HyphaAttachment {
    let mut attachment = record(HYPHA_ATTACH_SUBSCRIPTION);
    attachment.subscriber = subscriber;
    attachment.subscription_callback = callback;
    attachment.context = context;
    attachment.release = release;
    attachment
}
