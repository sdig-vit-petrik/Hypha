//! Bus discovery across the C ABI.
#![allow(clippy::expect_used)]

mod common;
use crate::common::c_text;
use crate::common::unique;

use std::cell::RefCell;
use std::ffi::CStr;
use std::os::raw::c_void;
use std::ptr;

use hypha_ffi::*;

thread_local! {
    static SEEN_PORTS: RefCell<Vec<(String, usize, usize)>> = const { RefCell::new(Vec::new()) };
}

extern "C" fn record_port(info: *const HyphaServicePortInfo, _user_data: *mut c_void) {
    if info.is_null() {
        return;
    }
    let reported = unsafe { &*info };
    if reported.name.is_null() {
        return;
    }
    let name = unsafe { CStr::from_ptr(reported.name) }.to_string_lossy().into_owned();
    SEEN_PORTS
        .with(|seen| seen.borrow_mut().push((name, reported.publishers, reported.subscribers)));
}

/// A node with one publisher and one subscriber on a private topic.
struct BusFixture {
    node: *mut HyphaNode,
    publisher: *mut HyphaPublisher,
    subscriber: *mut HyphaSubscriber,
    topic: String,
}

impl BusFixture {
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

        let topic_text = format!("/hypha_ffi/discovery/{}", unique("topic"));
        let topic = c_text(&topic_text);
        let data_type = c_text("std_msgs/Bytes");
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
                    &raw const subscriber_qos,
                    &raw mut subscriber,
                )
            },
            HyphaStatus::Ok
        );

        Self { node, publisher, subscriber, topic: topic_text }
    }
}

impl Drop for BusFixture {
    fn drop(&mut self) {
        unsafe {
            hypha_subscriber_destroy(self.subscriber);
            hypha_publisher_destroy(self.publisher);
            hypha_node_destroy(self.node);
        }
    }
}

#[test]
fn service_ports_report_the_endpoints_on_the_topic() {
    let fixture = BusFixture::new();
    SEEN_PORTS.with(|seen| seen.borrow_mut().clear());

    // The count excludes the observing node's own ports, so the endpoints live on one
    // node and the observation happens on another.
    let observer_name = c_text(&unique("observer"));
    let mut observer = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_node_create(
                observer_name.as_ptr(),
                observer_name.as_bytes().len(),
                ptr::null(),
                0,
                ptr::null(),
                &raw mut observer,
            )
        },
        HyphaStatus::Ok
    );

    assert_eq!(
        unsafe { hypha_list_service_ports(observer, Some(record_port), ptr::null_mut()) },
        HyphaStatus::Ok
    );
    unsafe { hypha_node_destroy(observer) };

    let ports = SEEN_PORTS.with(|seen| seen.borrow().clone());
    let (_, publishers, subscribers) = ports
        .iter()
        .find(|(name, _, _)| name == &fixture.topic)
        .expect("the open topic is reported with its ports");
    assert!(*publishers >= 1, "the publisher is counted");
    assert!(*subscribers >= 1, "the subscriber is counted");
}
