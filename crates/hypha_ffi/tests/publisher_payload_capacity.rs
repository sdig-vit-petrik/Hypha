//! `HyphaQos::max_payload_dynamic` bounds a publisher of a variable-size type.

mod common;
use crate::common::{create_unique_node, unique};

use std::ptr;

use hypha_ffi::*;

const ONE_MIB: usize = 1 << 20;

fn publish(node: *mut HyphaNode, max_payload_dynamic: u64) -> (HyphaStatus, *mut HyphaPublisher) {
    let topic = format!("/{}", unique("payload_capacity"));
    let data_type = "test/Bytes";
    let md5 = "d9d213e8c146065ec958333c617eb1a6";
    let definition = "";
    let qos = HyphaQos { max_payload_dynamic, ..HyphaQos::default() };
    let options = HyphaPublisherOptions { latch: 0, loaned_samples: 0, qos };
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
    (status, publisher)
}

/// The status of a loan of `size` bytes. A granted loan is dropped again.
fn loan_status(publisher: *mut HyphaPublisher, size: usize) -> HyphaStatus {
    let mut loan = ptr::null_mut();
    let mut payload_bytes = ptr::null_mut();
    let status = unsafe {
        hypha_publisher_loan_frame(publisher, size, &raw mut loan, &raw mut payload_bytes)
    };
    if status == HyphaStatus::Ok {
        unsafe { hypha_loan_drop(loan) };
    }
    status
}

#[test]
fn zero_is_growable() {
    let node = create_unique_node("payload_growable_zero");
    let (status, publisher) = publish(node, 0);
    assert_eq!(status, HyphaStatus::Ok);
    assert_eq!(loan_status(publisher, ONE_MIB), HyphaStatus::Ok);
    unsafe { hypha_publisher_destroy(publisher) };
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn a_bound_limits_the_loan() {
    let node = create_unique_node("payload_bound");
    let limit = 3 * ONE_MIB;
    let (status, publisher) = publish(node, u64::try_from(limit).unwrap_or(u64::MAX));
    assert_eq!(status, HyphaStatus::Ok);
    assert_eq!(loan_status(publisher, limit), HyphaStatus::Ok);
    assert_eq!(loan_status(publisher, limit + 1), HyphaStatus::TooLarge);
    unsafe { hypha_publisher_destroy(publisher) };
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn a_growable_publisher_has_no_limit() {
    let node = create_unique_node("payload_growable");
    let (status, publisher) = publish(node, 0);
    assert_eq!(status, HyphaStatus::Ok);
    assert_eq!(loan_status(publisher, 16 * ONE_MIB), HyphaStatus::Ok);
    unsafe { hypha_publisher_destroy(publisher) };
    unsafe { hypha_node_destroy(node) };
}
