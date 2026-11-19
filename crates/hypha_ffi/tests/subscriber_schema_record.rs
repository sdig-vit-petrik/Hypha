//! A typed subscriber that passes its definition puts the schema record in the
//! domain's store.

mod common;
use crate::common::{create_unique_node, unique};

use std::ptr;

use hypha_ffi::*;

const DATA_TYPE: &str = "test/Relayed";
const MD5: &str = "d9d213e8c146065ec958333c617eb1a6";
const DEFINITION: &str = "float64 x\nfloat64 y\n";

/// Subscribes and returns the status with the subscriber, null when refused. The
/// topic's schema records live as long as the topic is open.
fn subscribe(
    node: *mut HyphaNode,
    topic: &str,
    md5: &str,
    definition: &str,
) -> (HyphaStatus, *mut HyphaSubscriber) {
    let options = HyphaSubscriberOptions {
        depth: 1,
        no_publisher_warning_after_nanos: 0,
        qos: crate::common::test_qos(),
    };
    let mut subscriber = ptr::null_mut();
    let status = unsafe {
        hypha_subscribe_typed(
            node,
            topic.as_ptr().cast(),
            topic.len(),
            DATA_TYPE.as_ptr().cast(),
            DATA_TYPE.len(),
            md5.as_ptr().cast(),
            md5.len(),
            definition.as_ptr().cast(),
            definition.len(),
            &raw const options,
            &raw mut subscriber,
        )
    };
    (status, subscriber)
}

#[test]
fn a_typed_subscriber_alone_makes_its_schema_resolvable() {
    let subscriber_node = create_unique_node("schema_sub");
    let reader_node = create_unique_node("schema_reader");
    let topic = format!("/hypha_ffi/subscriber_schema/{}", unique("topic"));
    let (status, subscriber) = subscribe(subscriber_node, &topic, MD5, DEFINITION);
    assert_eq!(status, HyphaStatus::Ok);

    let mut buffer = vec![0_u8; 256];
    let mut length = 0_usize;
    let status = unsafe {
        hypha_schema_lookup(
            reader_node,
            topic.as_ptr().cast(),
            topic.len(),
            DATA_TYPE.as_ptr().cast(),
            DATA_TYPE.len(),
            MD5.as_ptr().cast(),
            MD5.len(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &raw mut length,
        )
    };
    assert_eq!(status, HyphaStatus::Ok);
    assert_eq!(buffer.get(..length), Some(DEFINITION.as_bytes()));
    unsafe { hypha_subscriber_destroy(subscriber) };
    unsafe { hypha_node_destroy(reader_node) };
    unsafe { hypha_node_destroy(subscriber_node) };
}

#[test]
fn a_definition_without_an_md5_is_refused() {
    let node = create_unique_node("schema_no_md5");
    let topic = format!("/hypha_ffi/subscriber_schema/{}", unique("topic"));
    let (status, subscriber) = subscribe(node, &topic, "", DEFINITION);
    assert_eq!(status, HyphaStatus::InvalidSchema);
    assert!(subscriber.is_null());
    unsafe { hypha_node_destroy(node) };
}
