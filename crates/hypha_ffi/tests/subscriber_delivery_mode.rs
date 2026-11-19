//! One subscriber has one delivery style: the second claim is refused with `InvalidOperation`.
#![allow(clippy::expect_used)]

mod common;
use crate::common::attach;
use crate::common::unique;

use std::ffi::CString;
use std::os::raw::c_void;
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};

use hypha_ffi::waitset::*;
use hypha_ffi::*;

const MILLISECOND: u64 = 1_000_000;

/// Counts the data deliveries and the context releases of one fixture.
#[derive(Default)]
struct Counters {
    deliveries: AtomicU64,
    releases: AtomicU64,
}

fn counters_of<'a>(context: *mut c_void) -> Option<&'a Counters> {
    unsafe { context.cast::<Counters>().as_ref() }
}

extern "C" fn count_data(
    _delivery: *const HyphaDelivery,
    context: *mut c_void,
) -> HyphaCallbackResult {
    if let Some(counters) = counters_of(context) {
        counters.deliveries.fetch_add(1, Ordering::SeqCst);
    }
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

extern "C" fn record_release(context: *mut c_void) {
    if let Some(counters) = counters_of(context) {
        counters.releases.fetch_add(1, Ordering::SeqCst);
    }
}

/// One node with one publisher, one subscriber and its `WaitSet`.
struct Fixture {
    node: *mut HyphaNode,
    publisher: *mut HyphaPublisher,
    subscriber: *mut HyphaSubscriber,
    waitset: *mut HyphaWaitSet,
    counters: Box<Counters>,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let name = CString::new(unique(label)).expect("node name");
        let mut node = ptr::null_mut();
        let created = unsafe {
            hypha_node_create(
                name.as_ptr(),
                name.as_bytes().len(),
                ptr::null(),
                0,
                ptr::null(),
                &raw mut node,
            )
        };
        assert_eq!(created, HyphaStatus::Ok);

        let topic = CString::new(format!("/{}", unique("topic"))).expect("topic");
        let data_type = CString::new("std_msgs/Bytes").expect("type");
        let md5 = CString::new("d9d213e8c146065ec958333c617eb1a6").expect("md5");
        let definition = CString::new("").expect("definition");
        let publisher_qos =
            HyphaPublisherOptions { latch: 0, loaned_samples: 0, qos: crate::common::test_qos() };
        let mut publisher = ptr::null_mut();
        let publish_status = unsafe {
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
                &raw const publisher_qos,
                &raw mut publisher,
            )
        };
        assert_eq!(publish_status, HyphaStatus::Ok);

        let subscriber_qos = HyphaSubscriberOptions {
            depth: 4,
            no_publisher_warning_after_nanos: 0,
            qos: hypha_ffi::HyphaQos::default(),
        };
        let mut subscriber = ptr::null_mut();
        let subscribe_status = unsafe {
            hypha_subscribe_typed(
                node,
                topic.as_ptr(),
                topic.as_bytes().len(),
                std::ptr::null(),
                0,
                md5.as_ptr(),
                md5.as_bytes().len(),
                std::ptr::null(),
                0,
                &raw const subscriber_qos,
                &raw mut subscriber,
            )
        };
        assert_eq!(subscribe_status, HyphaStatus::Ok);

        let mut waitset = ptr::null_mut();
        let waitset_created = unsafe { hypha_node_default_waitset(node, &raw mut waitset) };
        assert_eq!(waitset_created, HyphaStatus::Ok);
        Self { node, publisher, subscriber, waitset, counters: Box::default() }
    }

    fn publish(&self, byte: u8) {
        let mut loan = ptr::null_mut();
        let mut bytes = ptr::null_mut();
        let loaned =
            unsafe { hypha_publisher_loan_frame(self.publisher, 1, &raw mut loan, &raw mut bytes) };
        assert_eq!(loaned, HyphaStatus::Ok);
        unsafe { *bytes = byte };
        assert_eq!(unsafe { hypha_loan_send(loan, -1) }, HyphaStatus::Ok);
    }

    /// Attach the subscriber for event-driven dispatch with this fixture's counters.
    fn attach(&self) -> (HyphaStatus, HyphaAttachmentId) {
        let waitset = self.waitset;
        let mut attachment = 99;
        let context = ptr::from_ref(self.counters.as_ref()).cast_mut().cast::<c_void>();
        let status = unsafe {
            hypha_waitset_attach(
                waitset,
                &attach::subscription(
                    self.subscriber,
                    Some(count_data),
                    context,
                    Some(record_release),
                ),
                &raw mut attachment,
            )
        };
        (status, attachment)
    }

    /// An owning receive, the Rust SDK's: its status.
    fn receive(&self) -> HyphaStatus {
        let received = unsafe { crate::common::receive::receive(self.subscriber) };
        match received {
            Ok(_) => HyphaStatus::Ok,
            Err(status) => status,
        }
    }

    fn run_for(&self, limit_nanos: u64) -> HyphaWaitSetRunReport {
        let mut report =
            HyphaWaitSetRunReport { reason: 9, status: 9, attachment: 9, slow_callbacks: 9 };
        let status = unsafe { hypha_waitset_run(self.waitset, limit_nanos, &raw mut report) };
        assert_eq!(status, HyphaStatus::Ok);
        report
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        unsafe { hypha_subscriber_destroy(self.subscriber) };
        unsafe { hypha_publisher_destroy(self.publisher) };
        unsafe { hypha_node_destroy(self.node) };
    }
}

#[test]
fn an_attached_subscriber_refuses_an_owning_receive_and_keeps_the_sample() {
    let fixture = Fixture::new("attach_receive");
    assert_eq!(fixture.attach().0, HyphaStatus::Ok);
    fixture.publish(7);

    assert_eq!(fixture.receive(), HyphaStatus::InvalidOperation);

    let report = fixture.run_for(100 * MILLISECOND);
    assert_eq!(report.reason, HYPHA_RUN_REASON_DEADLINE);
    assert_eq!(
        fixture.counters.deliveries.load(Ordering::SeqCst),
        1,
        "the refused receive consumed nothing"
    );
}

#[test]
fn a_subscriber_read_by_owning_receive_refuses_an_attach() {
    let fixture = Fixture::new("receive_attach");
    assert_eq!(fixture.receive(), HyphaStatus::Ok, "an empty receive claims the style");

    let (status, attachment) = fixture.attach();

    assert_eq!(status, HyphaStatus::InvalidOperation);
    assert_eq!(attachment, 0, "a refused attach fills no id");
    assert_eq!(
        fixture.counters.releases.load(Ordering::SeqCst),
        0,
        "a refused attach transfers no context"
    );
}

#[test]
fn a_subscriber_read_by_hand_still_receives() {
    let fixture = Fixture::new("hand");
    fixture.publish(7);

    assert_eq!(fixture.receive(), HyphaStatus::Ok);
}

#[test]
fn one_waitset_does_not_attach_one_subscriber_twice() {
    let fixture = Fixture::new("same_waitset_twice");
    assert_eq!(fixture.attach().0, HyphaStatus::Ok);

    assert_eq!(fixture.attach().0, HyphaStatus::InvalidState);
}

#[test]
fn a_detached_subscriber_can_attach_again() {
    let fixture = Fixture::new("reattach");
    let (first, attachment) = fixture.attach();
    assert_eq!(first, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_waitset_detach(fixture.waitset, attachment) }, HyphaStatus::Ok);

    let (second, _) = fixture.attach();

    assert_eq!(second, HyphaStatus::Ok, "dispatch stays the style of this subscriber");
}
