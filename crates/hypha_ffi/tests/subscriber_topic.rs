//! `hypha_subscriber_topic` gives the topic a subscriber resolved at creation.
#![allow(clippy::expect_used)]

mod common;
use crate::common::unique;

use std::ffi::CString;
use std::ptr;

use hypha_ffi::*;

struct Fixture {
    node: *mut HyphaNode,
    node_name: String,
    subscriber: *mut HyphaSubscriber,
}

impl Fixture {
    fn new(topic: &str) -> Self {
        let node_name = unique("subscriber_topic");
        let name = CString::new(node_name.clone()).expect("node name");
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
        let topic = CString::new(topic).expect("topic");
        let options = HyphaSubscriberOptions {
            depth: 1,
            no_publisher_warning_after_nanos: 0,
            qos: crate::common::test_qos(),
        };
        let mut subscriber = ptr::null_mut();
        let subscribe_status = unsafe {
            hypha_subscribe_typed(
                node,
                topic.as_ptr(),
                topic.as_bytes().len(),
                c"hypha_test/Data".as_ptr(),
                "hypha_test/Data".len(),
                ptr::null(),
                0,
                std::ptr::null(),
                0,
                &raw const options,
                &raw mut subscriber,
            )
        };
        assert_eq!(subscribe_status, HyphaStatus::Ok);
        Self { node, node_name, subscriber }
    }

    fn topic(&self) -> String {
        let mut buffer = vec![0_u8; 256];
        let mut length = 0_usize;
        let status = unsafe {
            hypha_subscriber_topic(
                self.subscriber,
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &raw mut length,
            )
        };
        assert_eq!(status, HyphaStatus::Ok);
        buffer.truncate(length);
        String::from_utf8(buffer).expect("UTF-8")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        unsafe { hypha_subscriber_destroy(self.subscriber) };
        unsafe { hypha_node_destroy(self.node) };
    }
}

#[test]
fn a_node_scoped_topic_is_reported_resolved() {
    let fixture = Fixture::new("~/speed");
    assert_eq!(fixture.topic(), format!("/{}/speed", fixture.node_name));
}

#[test]
fn an_absolute_topic_is_reported_as_given() {
    let topic = format!("/{}", unique("absolute"));
    let fixture = Fixture::new(&topic);
    assert_eq!(fixture.topic(), topic);
}

#[test]
fn a_small_buffer_reports_the_length_it_needs() {
    let fixture = Fixture::new("~/speed");
    let expected = format!("/{}/speed", fixture.node_name);
    let mut buffer = [0_u8; 4];
    let mut length = 0_usize;
    let status = unsafe {
        hypha_subscriber_topic(
            fixture.subscriber,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &raw mut length,
        )
    };
    assert_eq!(status, HyphaStatus::TooLarge);
    assert_eq!(length, expected.len());
}
