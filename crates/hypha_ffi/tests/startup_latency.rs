//! A new node publishes its first message at once: node creation, the publisher and the
//! first send each succeed on the first call, with no discovery handshake to wait for
//! (NFR-11). The time it takes is a speed of the host and is not asserted (#64).

mod common;
use crate::common::c_text_opt as c_text;
use crate::common::unique;

use std::ptr;

use hypha_ffi::*;

#[test]
fn a_new_node_publishes_its_first_message_on_the_first_call() {
    let Some(node_name) = c_text(&unique("startup_latency")) else {
        return;
    };
    let topic_text = format!("/hypha_ffi/startup_latency/{}", unique("topic"));
    let Some(topic) = c_text(&topic_text) else {
        return;
    };
    let Some(data_type) = c_text("test/Bytes") else {
        return;
    };
    let Some(md5) = c_text("d9d213e8c146065ec958333c617eb1a6") else {
        return;
    };
    let Some(definition) = c_text("") else {
        return;
    };

    let mut node = ptr::null_mut();
    let node_status = unsafe {
        hypha_node_create(
            node_name.as_ptr(),
            node_name.as_bytes().len(),
            ptr::null(),
            0,
            ptr::null(),
            &raw mut node,
        )
    };
    assert_eq!(node_status, HyphaStatus::Ok);
    assert!(!node.is_null());

    let publisher_options =
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
            &raw const publisher_options,
            &raw mut publisher,
        )
    };
    assert_eq!(publish_status, HyphaStatus::Ok);

    let mut loan = ptr::null_mut();
    let mut bytes = ptr::null_mut();
    let loan_status =
        unsafe { hypha_publisher_loan_frame(publisher, 7, &raw mut loan, &raw mut bytes) };
    let length = 7;
    assert_eq!(loan_status, HyphaStatus::Ok);
    assert_eq!(length, 7);
    unsafe { std::slice::from_raw_parts_mut(bytes, length).copy_from_slice(b"payload") };

    let send_status = unsafe { hypha_loan_send(loan, -1) };
    assert_eq!(send_status, HyphaStatus::Ok);

    unsafe { hypha_publisher_destroy(publisher) };
    unsafe { hypha_node_destroy(node) };
}
