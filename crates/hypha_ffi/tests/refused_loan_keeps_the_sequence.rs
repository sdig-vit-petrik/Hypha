//! A loan the publisher refuses takes no sequence number.
#![allow(clippy::expect_used)]

mod common;
use crate::common::c_text;
use crate::common::unique;

use std::ptr;

use hypha_ffi::*;

/// The payload bound of the test publisher. A loan over it is refused as `TooLarge`.
const BOUND: usize = 64;

struct Fixture {
    node: *mut HyphaNode,
    publisher: *mut HyphaPublisher,
    subscriber: *mut HyphaSubscriber,
}

impl Fixture {
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

        let topic = c_text(&format!("/hypha_ffi/refused_loan/{}", unique("topic")));
        let data_type = c_text("test/Bytes");
        let md5 = c_text("d9d213e8c146065ec958333c617eb1a6");
        let definition = c_text("");
        let publisher_options = HyphaPublisherOptions {
            latch: 0,
            loaned_samples: 0,
            qos: HyphaQos { max_payload_dynamic: BOUND as u64, ..HyphaQos::default() },
        };
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
                    &raw const publisher_options,
                    &raw mut publisher,
                )
            },
            HyphaStatus::Ok
        );

        let subscriber_options = HyphaSubscriberOptions {
            depth: 4,
            no_publisher_warning_after_nanos: 0,
            qos: HyphaQos::default(),
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
                    &raw const subscriber_options,
                    &raw mut subscriber,
                )
            },
            HyphaStatus::Ok
        );
        Self { node, publisher, subscriber }
    }

    /// The status of a loan of `size` bytes. A granted loan is sent with `payload`.
    fn send(&self, size: usize, payload: &[u8]) -> HyphaStatus {
        let mut loan = ptr::null_mut();
        let mut bytes = ptr::null_mut();
        let status = unsafe {
            hypha_publisher_loan_frame(self.publisher, size, &raw mut loan, &raw mut bytes)
        };
        if status != HyphaStatus::Ok {
            return status;
        }
        let frame = unsafe { std::slice::from_raw_parts_mut(bytes, size) };
        frame
            .get_mut(..payload.len())
            .expect("the loan holds the payload")
            .copy_from_slice(payload);
        assert_eq!(unsafe { hypha_loan_send(loan, -1) }, HyphaStatus::Ok);
        HyphaStatus::Ok
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
fn a_loan_refused_as_too_large_leaves_no_gap_at_the_subscriber() {
    let fixture = Fixture::new();
    assert_eq!(fixture.send(4, b"one!"), HyphaStatus::Ok);
    assert!(fixture.receive_one());
    assert_eq!(fixture.missing(), 0);

    assert_eq!(
        fixture.send(BOUND + 1, b"too large"),
        HyphaStatus::TooLarge,
        "the loan over the bound is refused"
    );

    assert_eq!(fixture.send(4, b"two!"), HyphaStatus::Ok);
    assert!(fixture.receive_one(), "the frame after the refused loan arrives");
    assert_eq!(fixture.missing(), 0, "the refused loan took no sequence number");
}
