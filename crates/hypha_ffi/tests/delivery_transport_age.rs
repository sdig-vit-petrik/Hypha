//! A `WaitSet` data delivery reports the transport age of its frame.
#![allow(clippy::expect_used)]

mod common;
use crate::common::attach;
use crate::common::{create_unique_node, unique};

use std::ffi::CString;
use std::os::raw::c_void;
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use hypha_ffi::waitset::*;
use hypha_ffi::*;

/// The time the test holds the sample in the queue before delivery.
const QUEUED_NANOS: u64 = 30_000_000;
/// An age above this is not a queue delay; it is a wrong value.
const IMPLAUSIBLE_AGE_NANOS: u64 = 10_000_000_000;

static DELIVERED_AGE: AtomicU64 = AtomicU64::new(u64::MAX);
static DELIVERED_SENT: AtomicU64 = AtomicU64::new(0);

extern "C" fn record_age(
    delivery: *const HyphaDelivery,
    _context: *mut c_void,
) -> HyphaCallbackResult {
    if !delivery.is_null() {
        let delivered = unsafe { &*delivery };
        DELIVERED_AGE.store(delivered.transport_age_nanos, Ordering::SeqCst);
        DELIVERED_SENT.store(delivered.identity.transport_sent_nanos, Ordering::SeqCst);
    }
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

unsafe fn create_waitset(node: *mut HyphaNode) -> *mut HyphaWaitSet {
    let mut waitset = ptr::null_mut();
    let status = unsafe { hypha_node_default_waitset(node, &raw mut waitset) };
    assert_eq!(status, HyphaStatus::Ok);
    waitset
}

unsafe fn create_subscriber(node: *mut HyphaNode, topic: &CString) -> *mut HyphaSubscriber {
    let options = HyphaSubscriberOptions {
        depth: 4,
        no_publisher_warning_after_nanos: 0,
        qos: crate::common::test_qos(),
    };
    let mut subscriber = ptr::null_mut();
    let status = unsafe {
        hypha_subscribe_typed(
            node,
            topic.as_ptr(),
            topic.as_bytes().len(),
            c"std_msgs/Data".as_ptr(),
            "std_msgs/Data".len(),
            c"d9d213e8c146065ec958333c617eb1a6".as_ptr(),
            32,
            std::ptr::null(),
            0,
            &raw const options,
            &raw mut subscriber,
        )
    };
    assert_eq!(status, HyphaStatus::Ok);
    subscriber
}

unsafe fn create_publisher(node: *mut HyphaNode, topic: &CString) -> *mut HyphaPublisher {
    let data_type = CString::new("std_msgs/Bytes").expect("type");
    let md5 = CString::new("d9d213e8c146065ec958333c617eb1a6").expect("md5");
    let definition = CString::new("").expect("definition");
    let options =
        HyphaPublisherOptions { latch: 0, loaned_samples: 0, qos: crate::common::test_qos() };
    let mut publisher = ptr::null_mut();
    let status = unsafe {
        hypha_publish_with_qos(
            node,
            topic.as_ptr(),
            topic.as_bytes().len(),
            data_type.as_ptr(),
            data_type.as_bytes().len(),
            md5.as_ptr(),
            md5.as_bytes().len(),
            definition.as_ptr(),
            0,
            &raw const options,
            &raw mut publisher,
        )
    };
    assert_eq!(status, HyphaStatus::Ok);
    publisher
}

unsafe fn send_one_byte(publisher: *mut HyphaPublisher) {
    let mut loan = ptr::null_mut();
    let mut bytes = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_publisher_loan_frame(publisher, 1, &raw mut loan, &raw mut bytes) },
        HyphaStatus::Ok
    );
    unsafe { *bytes = 7 };
    assert_eq!(unsafe { hypha_loan_send(loan, -1) }, HyphaStatus::Ok);
}

unsafe fn deliver_once(waitset: *mut HyphaWaitSet, subscriber: *mut HyphaSubscriber) {
    let mut attachment = 0;
    let status = unsafe {
        hypha_waitset_attach(
            waitset,
            &attach::subscription(subscriber, Some(record_age), ptr::null_mut(), None),
            &raw mut attachment,
        )
    };
    assert_eq!(status, HyphaStatus::Ok);
    let mut report =
        HyphaWaitSetRunReport { reason: 9, status: 9, attachment: 9, slow_callbacks: 9 };
    assert_eq!(unsafe { hypha_waitset_run(waitset, 50_000_000, &raw mut report) }, HyphaStatus::Ok);
}

#[test]
fn a_delivery_reports_the_age_of_its_frame() {
    let node = create_unique_node("node");
    let waitset = unsafe { create_waitset(node) };
    let topic = CString::new(format!("/{}", unique("topic"))).expect("topic");
    let subscriber = unsafe { create_subscriber(node, &topic) };
    let publisher = unsafe { create_publisher(node, &topic) };

    unsafe { send_one_byte(publisher) };
    std::thread::sleep(Duration::from_nanos(QUEUED_NANOS));
    DELIVERED_AGE.store(u64::MAX, Ordering::SeqCst);
    unsafe { deliver_once(waitset, subscriber) };
    let host_now = u64::try_from(hypha_clock_steady_now_nanos()).expect("monotonic time");

    let age = DELIVERED_AGE.load(Ordering::SeqCst);
    let sent = DELIVERED_SENT.load(Ordering::SeqCst);
    assert_ne!(age, u64::MAX, "the sample must be delivered");
    assert!(age >= QUEUED_NANOS, "the age covers the time in the queue: {age} ns");
    assert!(age < IMPLAUSIBLE_AGE_NANOS, "the age is a queue delay, not a raw clock: {age} ns");
    assert!(
        sent.saturating_add(age) <= host_now,
        "send time plus age is not later than the host clock"
    );

    unsafe { hypha_publisher_destroy(publisher) };
    unsafe { hypha_subscriber_destroy(subscriber) };
    unsafe { hypha_node_destroy(node) };
}
