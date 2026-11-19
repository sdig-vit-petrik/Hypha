//! The schema counters of a node: what it owns and what it refused.
#![allow(clippy::expect_used)]

mod common;
use crate::common::{create_node, node_metric, schema_counts, unique};

use std::ptr;

use hypha_ffi::*;

const DATA_TYPE: &str = "std_msgs/String";
const MD5: &str = "992ce8a1687cec8c8bd883ec73ca41d1";
const DEFINITION: &str = "string data\n";

unsafe fn advertise(node: *mut HyphaNode, topic: &str) -> *mut HyphaPublisher {
    let mut publisher: *mut HyphaPublisher = ptr::null_mut();
    let status = unsafe {
        hypha_publish_with_qos(
            node,
            topic.as_ptr().cast(),
            topic.len(),
            DATA_TYPE.as_ptr().cast(),
            DATA_TYPE.len(),
            MD5.as_ptr().cast(),
            MD5.len(),
            DEFINITION.as_ptr().cast(),
            DEFINITION.len(),
            &HyphaPublisherOptions { latch: 0, loaned_samples: 0, qos: crate::common::test_qos() },
            &raw mut publisher,
        )
    };
    assert_eq!(status, HyphaStatus::Ok);
    publisher
}

#[test]
fn health_counts_what_this_node_owns_and_refused() {
    let node = create_node(&unique("health"));
    let topic = format!("/{}", unique("topic"));
    let publisher = unsafe { advertise(node, &topic) };

    let health = schema_counts(node);

    // A lookup nobody can answer is counted as unavailable, not as an error.
    let missing = format!("/{}", unique("absent"));
    let mut needed: usize = 0;
    let status = unsafe {
        hypha_schema_lookup(
            node,
            missing.as_ptr().cast(),
            missing.len(),
            DATA_TYPE.as_ptr().cast(),
            DATA_TYPE.len(),
            MD5.as_ptr().cast(),
            MD5.len(),
            ptr::null_mut(),
            0,
            &raw mut needed,
        )
    };
    assert_eq!(status, HyphaStatus::NotAvailable);

    let after = schema_counts(node);
    assert!(
        after.unavailable_lookups > health.unavailable_lookups,
        "the unanswered lookup was counted"
    );

    assert_eq!(node_metric(ptr::null(), "schema.invalid_records"), None, "no node, no schema");
    assert_eq!(
        unsafe { hypha_node_metrics(node, None, ptr::null_mut()) },
        HyphaStatus::InvalidArgument
    );

    unsafe { hypha_publisher_destroy(publisher) };
    unsafe { hypha_node_destroy(node) };
}
