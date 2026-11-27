//! Keeping a delivered sample past its callback: the C ABI of
//! `hypha_node::sample::delivery_retain`.

use crate::boundary::{take_box, write_out};
use crate::status::try_guard;
use hypha_node::HyphaDelivery;
use hypha_node::safe::{HyphaReceivedSample, delivery_retain};
use hypha_node::status::HyphaStatus;

/// Keep the sample of `delivery` after its callback returns, without a copy: its payload
/// stays valid, at `delivery->payload`, until `hypha_received_sample_release`.
///
/// A subscriber's callbacks keep at most the subscriber's loan budget of samples at once
/// (`HyphaSubscriberCounters::loan_budget`; one sample stays free for the next delivery).
/// Valid only during the callback that received `delivery`, on its thread. On `Ok`, `*out`
/// is the kept sample; on any other status it is null. `BorrowLimit` while the
/// subscriber's callbacks hold their budget, counted in `loan_refused`: the sample is then
/// released at the return as usual. `InvalidState`: the sample was kept already, or the
/// delivery cannot be kept (`holder` null). `InvalidArgument`: a null `delivery` or `out`.
///
/// # Safety
/// `delivery` is null or the pointer a subscription callback received, during that call.
/// `out` is null or points to one writable pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_delivery_retain(
    delivery: *const HyphaDelivery,
    out: *mut *mut HyphaReceivedSample,
) -> HyphaStatus {
    try_guard(|| {
        unsafe { write_out(out, std::ptr::null_mut()) }?;
        // SAFETY: null or the live delivery of the running callback.
        let delivery = unsafe { delivery.as_ref() }.ok_or(HyphaStatus::InvalidArgument)?;
        // SAFETY: the holder lives for the callback that received `delivery`.
        let holder = unsafe { delivery.holder.as_ref() }.ok_or(HyphaStatus::InvalidState)?;
        let kept = delivery_retain(holder)?;
        unsafe { write_out(out, Box::into_raw(Box::new(kept))) }
    })
}

/// Release a sample `hypha_delivery_retain` kept: its slot returns to the publisher and its
/// payload is invalid from here. Any thread may release it. Null does nothing.
///
/// # Safety
/// `sample` is null or comes from `hypha_delivery_retain` and is not used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_received_sample_release(sample: *mut HyphaReceivedSample) {
    drop(unsafe { take_box(sample) });
}
