//! The missing-sequence count a subscriber keeps for frames it lost.
#![allow(clippy::expect_used)]

mod common;
use crate::common::c_text;
use crate::common::unique;

use std::ptr;

use hypha_ffi::*;

struct Fixture {
    node: *mut HyphaNode,
    publisher: *mut HyphaPublisher,
    subscriber: *mut HyphaSubscriber,
}

impl Fixture {
    /// A publisher and a subscriber that keeps one pending sample.
    fn new() -> Self {
        let name = c_text(&unique("node"));
        let mut node = ptr::null_mut();
        assert_eq!(
            unsafe {
                hypha_node_create(
                    name.as_ptr(),
                    name.as_bytes().len(),
                    ptr::null(),
                    0,
                    ptr::null(),
                    &raw mut node,
                )
            },
            HyphaStatus::Ok
        );

        let topic = c_text(&format!("/hypha_ffi/missing/{}", unique("topic")));
        let data_type = c_text("test/Bytes");
        let md5 = c_text("d9d213e8c146065ec958333c617eb1a6");
        let definition = c_text("");
        let publisher_qos =
            HyphaPublisherOptions { latch: 0, loaned_samples: 0, qos: crate::common::test_qos() };
        let mut publisher = ptr::null_mut();
        assert_eq!(
            unsafe {
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
            },
            HyphaStatus::Ok
        );

        let subscriber_qos = HyphaSubscriberOptions {
            depth: 1,
            no_publisher_warning_after_nanos: 0,
            qos: hypha_ffi::HyphaQos::default(),
        };
        let mut subscriber = ptr::null_mut();
        assert_eq!(
            unsafe {
                hypha_subscribe_typed(
                    node,
                    topic.as_ptr(),
                    topic.as_bytes().len(),
                    std::ptr::null(),
                    0,
                    ptr::null(),
                    0,
                    std::ptr::null(),
                    0,
                    &raw const subscriber_qos,
                    &raw mut subscriber,
                )
            },
            HyphaStatus::Ok
        );

        Self { node, publisher, subscriber }
    }

    fn publish(&self, payload: &[u8]) {
        let mut loan = ptr::null_mut();
        let mut bytes = ptr::null_mut();
        assert_eq!(
            unsafe {
                hypha_publisher_loan_frame(
                    self.publisher,
                    payload.len(),
                    &raw mut loan,
                    &raw mut bytes,
                )
            },
            HyphaStatus::Ok
        );
        let length = payload.len();
        unsafe { std::slice::from_raw_parts_mut(bytes, length).copy_from_slice(payload) };
        assert_eq!(unsafe { hypha_loan_send(loan, -1) }, HyphaStatus::Ok);
    }

    fn receive_one(&self) -> bool {
        let received = unsafe { crate::common::receive::receive(self.subscriber) };
        assert!(received.is_ok(), "the receive is refused: {:?}", received.as_ref().err());
        received.ok().flatten().is_some()
    }

    fn missing(&self) -> u64 {
        let mut count = u64::MAX;
        assert_eq!(
            unsafe { hypha_subscriber_missing_sequences(self.subscriber, &raw mut count) },
            HyphaStatus::Ok
        );
        count
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        unsafe {
            hypha_subscriber_destroy(self.subscriber);
            hypha_publisher_destroy(self.publisher);
            hypha_node_destroy(self.node);
        }
    }
}

#[test]
fn steady_traffic_never_counts_a_missing_sequence() {
    let fixture = Fixture::new();
    assert_eq!(fixture.missing(), 0, "a new subscriber has lost nothing");
    for index in 0_u8..4 {
        fixture.publish(&[index]);
        assert!(fixture.receive_one(), "each frame is read before the next is sent");
    }
    assert_eq!(fixture.missing(), 0, "frames read in order lose nothing");
}

#[test]
fn an_overflowing_queue_counts_the_samples_it_dropped() {
    let fixture = Fixture::new();
    // The first accepted frame is the baseline, so the overflow is measured from it.
    fixture.publish(&[0]);
    assert!(fixture.receive_one());

    for index in 1_u8..=5 {
        fixture.publish(&[index]);
    }
    // The queue keeps only the newest of the five, so four were lost.
    assert!(fixture.receive_one(), "the newest frame survives the overflow");
    assert!(fixture.missing() >= 4, "missing {} of the 4 dropped frames", fixture.missing());
}
