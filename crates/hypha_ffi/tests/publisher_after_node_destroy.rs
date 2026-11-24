//! A publisher whose node was destroyed sends nothing: its loan and its send are refused
//! with `InvalidState` (#97). Before, it went on reporting sends after its node left.
#![allow(clippy::expect_used, reason = "a failed test setup should stop the test with its message")]

mod common;
use common::replay::{create_pubsub, send_payload};
use common::{create_node, unique};

use std::ptr;

use hypha_ffi::*;

#[test]
fn a_loan_or_a_send_after_the_node_was_destroyed_is_refused() {
    let node = create_node(&unique("publisher_left"));
    let topic = format!("/{}", unique("publisher_left"));
    // SAFETY: `node` is live.
    let (publisher, subscriber) = unsafe { create_pubsub(node, &topic, 4) };
    // SAFETY: the publisher is live.
    unsafe { send_payload(publisher, &[1, 0, 0, 0]) };

    // A loan taken while the node lives and sent after it left.
    let mut early = ptr::null_mut();
    let mut early_bytes = ptr::null_mut();
    // SAFETY: the publisher is live; the outputs are locals.
    let lent =
        unsafe { hypha_publisher_loan_frame(publisher, 4, &raw mut early, &raw mut early_bytes) };
    assert_eq!(lent, HyphaStatus::Ok);

    // SAFETY: `node` is live and not used afterwards; its publisher and subscriber stay.
    unsafe { hypha_node_destroy(node) };

    let mut loan = ptr::dangling_mut::<HyphaLoan>();
    let mut bytes = ptr::dangling_mut::<u8>();
    // SAFETY: the publisher handle is still live; the outputs are locals.
    let refused =
        unsafe { hypha_publisher_loan_frame(publisher, 4, &raw mut loan, &raw mut bytes) };
    assert_eq!(refused, HyphaStatus::InvalidState);
    assert!(loan.is_null() && bytes.is_null(), "a refused loan writes null");
    // SAFETY: `early` came from the loan call above and is not used afterwards.
    let sent = unsafe { hypha_loan_send(early, -1) };
    assert_eq!(sent, HyphaStatus::InvalidState, "a loan taken before is not sent after");

    // SAFETY: each handle is live and released once.
    unsafe {
        hypha_publisher_destroy(publisher);
        hypha_subscriber_destroy(subscriber);
    }
}
