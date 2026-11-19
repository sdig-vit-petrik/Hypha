//! C ABI publisher/subscriber lifecycle: framed sample exchange and envelope rejection.
#![allow(clippy::expect_used, reason = "a frame that does not arrive is a test failure")]

mod common;
use crate::common::c_text_opt as c_text;
use crate::common::{create_node, subscriber_metric, unique};

use std::ptr;

use hypha_ffi::*;

#[test]
#[allow(clippy::too_many_lines)]
fn publisher_and_subscriber_exchange_framed_samples() {
    assert_eq!(
        unsafe {
            hypha_publish_with_qos(
                ptr::null_mut(),
                ptr::null(),
                0,
                ptr::null(),
                0,
                ptr::null(),
                0,
                ptr::null(),
                0,
                ptr::null(),
                ptr::null_mut(),
            )
        },
        HyphaStatus::InvalidArgument
    );
    let mut loan = ptr::null_mut();
    let mut bytes = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_publisher_loan_frame(ptr::null_mut(), 1, &raw mut loan, &raw mut bytes) },
        HyphaStatus::InvalidArgument
    );
    assert_eq!(
        unsafe { hypha_publisher_loan_frame(ptr::null_mut(), 1, ptr::null_mut(), &raw mut bytes) },
        HyphaStatus::InvalidArgument
    );
    assert_eq!(
        unsafe { hypha_publisher_loan_frame(ptr::null_mut(), 1, &raw mut loan, ptr::null_mut()) },
        HyphaStatus::InvalidArgument
    );
    assert_eq!(unsafe { hypha_loan_send(ptr::null_mut(), -1) }, HyphaStatus::InvalidArgument);
    unsafe { hypha_loan_drop(ptr::null_mut()) };
    assert_eq!(unsafe { hypha_publisher_subscriber_count(ptr::null(), ptr::null_mut()) }, 0);
    unsafe { hypha_publisher_destroy(ptr::null_mut()) };
    let node = create_node(&unique("flow"));
    assert!(!node.is_null());
    let topic_text = format!("/hypha_ffi/coverage/{}", unique("topic"));
    let topic = c_text(&topic_text);
    let data_type = c_text("std_msgs/Bytes");
    let md5 = c_text("d9d213e8c146065ec958333c617eb1a6");
    let definition = c_text("");
    let Some(topic) = topic else { return };
    let Some(data_type) = data_type else { return };
    let Some(md5) = md5 else { return };
    let Some(definition) = definition else { return };
    let options =
        HyphaPublisherOptions { latch: 1, loaned_samples: 0, qos: crate::common::test_qos() };
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
                &raw const options,
                &raw mut publisher,
            )
        },
        HyphaStatus::Ok
    );
    let subscriber_options = HyphaSubscriberOptions {
        depth: 2,
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
                &raw const subscriber_options,
                &raw mut subscriber,
            )
        },
        HyphaStatus::Ok
    );
    let mut owner_subscriber = ptr::null_mut();
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
                &raw mut owner_subscriber,
            )
        },
        HyphaStatus::Ok
    );
    let mut count = 0;
    let count_status = unsafe { hypha_publisher_subscriber_count(publisher, &raw mut count) };
    assert_eq!(count_status, 1);
    assert_eq!(count, 2);
    let mut loan = ptr::null_mut();
    let mut bytes = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_publisher_loan_frame(publisher, 7, &raw mut loan, &raw mut bytes) },
        HyphaStatus::Ok
    );
    let length = 7;
    unsafe { std::slice::from_raw_parts_mut(bytes, length).copy_from_slice(b"payload") };
    assert_eq!(unsafe { hypha_loan_send(loan, -1) }, HyphaStatus::Ok);
    let received = unsafe { crate::common::receive::receive(owner_subscriber) };
    let frame = received.ok().flatten().expect("the sent frame is queued");
    assert_eq!(frame.payload.len(), 7);
    let host_now = u64::try_from(hypha_clock_steady_now_nanos()).unwrap_or(0);
    assert!(frame.sent.transport_sent_nanos > 0, "the publisher writes the send time");
    assert!(frame.sent.transport_sent_nanos <= host_now, "the send time is not in the future");
    assert!(
        unsafe { subscriber_metric(owner_subscriber, "loan.budget") }
            .is_some_and(|budget| budget > 0)
    );
    unsafe { hypha_subscriber_destroy(owner_subscriber) };
    unsafe { hypha_publisher_destroy(publisher) };
    unsafe { hypha_subscriber_destroy(subscriber) };
    unsafe { hypha_node_shutdown(node) };
    unsafe { hypha_node_destroy(node) };
}
