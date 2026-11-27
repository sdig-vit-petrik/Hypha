//! A subscription callback keeps its sample past the call without a copy
//! (`hypha_delivery_retain`): the payload stays in place until the release, within the
//! subscriber's loan budget, and a refusal is counted.
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::as_conversions)]

mod common;
use crate::common::attach;
use crate::common::{create_unique_node, unique};

use std::ffi::CString;
use std::os::raw::c_void;
use std::ptr;
use std::sync::Mutex;

use hypha_ffi::waitset::*;
use hypha_ffi::*;

/// What one delivery saw: the payload address and the status of the keep.
struct Seen {
    payload: *const u8,
    len: usize,
    retain: HyphaStatus,
    again: HyphaStatus,
    kept: *mut HyphaReceivedSample,
}

#[derive(Default)]
struct Context {
    seen: Mutex<Vec<Seen>>,
}

extern "C" fn keep(delivery: *const HyphaDelivery, context: *mut c_void) -> HyphaCallbackResult {
    let context = unsafe { &*context.cast::<Context>() };
    let delivered = unsafe { &*delivery };
    let mut kept = ptr::null_mut();
    let retain = unsafe { hypha_delivery_retain(delivery, &raw mut kept) };
    // A second keep of a kept sample; none after a refusal, which would count again.
    let mut second = ptr::null_mut();
    let again = if retain == HyphaStatus::Ok {
        unsafe { hypha_delivery_retain(delivery, &raw mut second) }
    } else {
        retain
    };
    assert!(second.is_null(), "a refused keep writes null");
    context.seen.lock().expect("seen").push(Seen {
        payload: delivered.payload,
        len: delivered.payload_len,
        retain,
        again,
        kept,
    });
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

unsafe fn endpoints(
    node: *mut HyphaNode,
    topic: &CString,
) -> (*mut HyphaSubscriber, *mut HyphaPublisher) {
    let qos = crate::common::test_qos();
    let options = HyphaSubscriberOptions { depth: 4, no_publisher_warning_after_nanos: 0, qos };
    let md5 = c"d9d213e8c146065ec958333c617eb1a6";
    let mut subscriber = ptr::null_mut();
    let status = unsafe {
        hypha_subscribe_typed(
            node,
            topic.as_ptr(),
            topic.as_bytes().len(),
            c"std_msgs/Data".as_ptr(),
            "std_msgs/Data".len(),
            md5.as_ptr(),
            32,
            ptr::null(),
            0,
            &raw const options,
            &raw mut subscriber,
        )
    };
    assert_eq!(status, HyphaStatus::Ok);
    let options = HyphaPublisherOptions { latch: 0, loaned_samples: 0, qos };
    let mut publisher = ptr::null_mut();
    let status = unsafe {
        hypha_publish_with_qos(
            node,
            topic.as_ptr(),
            topic.as_bytes().len(),
            c"std_msgs/Data".as_ptr(),
            "std_msgs/Data".len(),
            md5.as_ptr(),
            32,
            c"".as_ptr(),
            0,
            &raw const options,
            &raw mut publisher,
        )
    };
    assert_eq!(status, HyphaStatus::Ok);
    (subscriber, publisher)
}

unsafe fn send(publisher: *mut HyphaPublisher, value: u8) {
    let mut loan = ptr::null_mut();
    let mut bytes = ptr::null_mut();
    let status =
        unsafe { hypha_publisher_loan_frame(publisher, 64, &raw mut loan, &raw mut bytes) };
    assert_eq!(status, HyphaStatus::Ok);
    unsafe { ptr::write_bytes(bytes, value, 64) };
    assert_eq!(unsafe { hypha_loan_send(loan, -1) }, HyphaStatus::Ok);
}

unsafe fn run(waitset: *mut HyphaWaitSet) {
    let mut report =
        HyphaWaitSetRunReport { reason: 9, status: 9, attachment: 9, slow_callbacks: 9 };
    assert_eq!(unsafe { hypha_waitset_run(waitset, 50_000_000, &raw mut report) }, HyphaStatus::Ok);
}

fn counters(subscriber: *const HyphaSubscriber) -> HyphaSubscriberCounters {
    let mut counters = HyphaSubscriberCounters::default();
    assert_eq!(
        unsafe { hypha_subscriber_counters(subscriber, &raw mut counters) },
        HyphaStatus::Ok
    );
    counters
}

#[test]
fn a_kept_delivery_stays_in_place_within_the_budget_and_a_refusal_is_counted() {
    let node = create_unique_node("node");
    let mut waitset = ptr::null_mut();
    assert_eq!(unsafe { hypha_node_default_waitset(node, &raw mut waitset) }, HyphaStatus::Ok);
    let topic = CString::new(format!("/{}", unique("retain"))).expect("topic");
    let (subscriber, publisher) = unsafe { endpoints(node, &topic) };
    let context = Box::into_raw(Box::new(Context::default()));
    let mut attachment = 0;
    let record = attach::subscription(subscriber, Some(keep), context.cast(), None);
    assert_eq!(
        unsafe { hypha_waitset_attach(waitset, &raw const record, &raw mut attachment) },
        HyphaStatus::Ok
    );

    unsafe { send(publisher, 7) };
    unsafe { run(waitset) };
    unsafe { send(publisher, 8) };
    unsafe { run(waitset) };

    let context_ref = unsafe { &*context };
    let seen = context_ref.seen.lock().expect("seen");
    assert_eq!(seen.len(), 2, "both frames are delivered, the refused keep included");
    assert_eq!(seen[0].retain, HyphaStatus::Ok, "the first keep is within the budget");
    assert_eq!(seen[0].again, HyphaStatus::InvalidState, "a sample is kept once");
    assert_eq!(seen[1].retain, HyphaStatus::BorrowLimit, "the budget is one kept sample");
    assert!(seen[1].kept.is_null(), "a refused keep gives no sample");
    // The first payload is in place after its callback and a later delivery.
    let first = unsafe { std::slice::from_raw_parts(seen[0].payload, seen[0].len) };
    assert!(first.iter().all(|byte| *byte == 7), "the kept payload is unchanged");
    let held = counters(subscriber);
    assert_eq!((held.loan_budget, held.loan_held, held.loan_refused), (1, 1, 1));

    // Any thread releases it, and the budget is free again.
    let kept = seen[0].kept as usize;
    drop(seen);
    std::thread::spawn(move || unsafe {
        hypha_received_sample_release(kept as *mut HyphaReceivedSample);
    })
    .join()
    .expect("release thread");
    assert_eq!(counters(subscriber).loan_held, 0, "the release returns the budget");
    unsafe { send(publisher, 9) };
    unsafe { run(waitset) };
    let seen = context_ref.seen.lock().expect("seen");
    assert_eq!(seen[2].retain, HyphaStatus::Ok, "a free budget keeps the next delivery");
    let third = unsafe { std::slice::from_raw_parts(seen[2].payload, seen[2].len) };
    assert!(third.iter().all(|byte| *byte == 9));
    unsafe { hypha_received_sample_release(seen[2].kept) };
    drop(seen);

    unsafe { hypha_received_sample_release(ptr::null_mut()) };
    let mut out = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_delivery_retain(ptr::null(), &raw mut out) },
        HyphaStatus::InvalidArgument
    );
    assert_eq!(unsafe { hypha_waitset_detach(waitset, attachment) }, HyphaStatus::Ok);
    unsafe { hypha_publisher_destroy(publisher) };
    unsafe { hypha_subscriber_destroy(subscriber) };
    unsafe { hypha_node_destroy(node) };
    drop(unsafe { Box::from_raw(context) });
}
