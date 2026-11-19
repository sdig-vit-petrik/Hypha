//! A publisher and a subscriber accept the same MD5 text: 32 lower-case hex digits.

mod common;
use crate::common::{create_unique_node, unique};

use std::ptr;

use hypha_ffi::*;

fn subscribe(node: *mut HyphaNode, topic: &str, md5: &str) -> HyphaStatus {
    let options = HyphaSubscriberOptions {
        depth: 1,
        no_publisher_warning_after_nanos: 0,
        qos: crate::common::test_qos(),
    };
    let data_type = "test/Bytes";
    let mut subscriber = ptr::null_mut();
    let status = unsafe {
        hypha_subscribe_typed(
            node,
            topic.as_ptr().cast(),
            topic.len(),
            data_type.as_ptr().cast(),
            data_type.len(),
            md5.as_ptr().cast(),
            md5.len(),
            std::ptr::null(),
            0,
            &raw const options,
            &raw mut subscriber,
        )
    };
    if !subscriber.is_null() {
        unsafe { hypha_subscriber_destroy(subscriber) };
    }
    status
}

fn publish(node: *mut HyphaNode, topic: &str, md5: &str) -> HyphaStatus {
    let options =
        HyphaPublisherOptions { latch: 0, loaned_samples: 0, qos: crate::common::test_qos() };
    let data_type = "test/Bytes";
    let definition = "";
    let mut publisher = ptr::null_mut();
    let status = unsafe {
        hypha_publish_with_qos(
            node,
            topic.as_ptr().cast(),
            topic.len(),
            data_type.as_ptr().cast(),
            data_type.len(),
            md5.as_ptr().cast(),
            md5.len(),
            definition.as_ptr().cast(),
            definition.len(),
            &raw const options,
            &raw mut publisher,
        )
    };
    if !publisher.is_null() {
        unsafe { hypha_publisher_destroy(publisher) };
    }
    status
}

#[test]
fn upper_case_md5_is_refused_by_publisher_and_subscriber_alike() {
    let node = create_unique_node("md5_rule");
    let topic = format!("/hypha_ffi/md5_rule/{}", unique("topic"));
    let upper = "0123456789ABCDEF0123456789ABCDEF";
    assert_eq!(publish(node, &topic, upper), HyphaStatus::InvalidTopic);
    assert_eq!(subscribe(node, &topic, upper), HyphaStatus::InvalidSchema);

    let lower = "d9d213e8c146065ec958333c617eb1a6";
    assert_eq!(publish(node, &topic, lower), HyphaStatus::Ok);
    assert_eq!(subscribe(node, &topic, lower), HyphaStatus::Ok);
    unsafe { hypha_node_destroy(node) };
}
