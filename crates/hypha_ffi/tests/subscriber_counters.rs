//! The counters of one subscriber through `hypha_subscriber_counters`.
#![expect(clippy::expect_used, reason = "a failed test setup stops the test with its message")]

mod common;
use crate::common::replay::{create_pubsub, send_payload};
use crate::common::{create_node, subscriber_metric, unique};

use std::ptr;

use hypha_ffi::*;

/// The eight counters, read through the C ABI.
fn counters(subscriber: *const HyphaSubscriber) -> HyphaSubscriberCounters {
    let mut out = HyphaSubscriberCounters {
        sequence_gap: u64::MAX,
        invalid_frame: u64::MAX,
        corrupt_payload: u64::MAX,
        schema_mismatch: u64::MAX,
        invalid_timestamp: u64::MAX,
        loan_budget: u64::MAX,
        loan_held: u64::MAX,
        loan_refused: u64::MAX,
        other_schemas: u64::MAX,
    };
    // SAFETY: `subscriber` is live; `out` is a local.
    assert_eq!(unsafe { hypha_subscriber_counters(subscriber, &raw mut out) }, HyphaStatus::Ok);
    out
}

/// The counters, read from the metrics the subscriber emits by name; `other_schemas` has
/// no metric, and a subscriber that no run of its node drove has not checked it.
fn emitted(subscriber: *const HyphaSubscriber) -> HyphaSubscriberCounters {
    // SAFETY: `subscriber` is live.
    let value = |name: &str| unsafe { subscriber_metric(subscriber, name) }.expect(name);
    HyphaSubscriberCounters {
        sequence_gap: value("integrity.sequence_gap"),
        invalid_frame: value("integrity.invalid_frame"),
        corrupt_payload: value("integrity.corrupt_payload"),
        schema_mismatch: value("integrity.schema_mismatch"),
        invalid_timestamp: value("integrity.invalid_timestamp"),
        loan_budget: value("loan.budget"),
        loan_held: value("loan.held"),
        loan_refused: value("loan.refused"),
        other_schemas: hypha_ffi::HYPHA_NOT_CHECKED,
    }
}

fn missing(subscriber: *const HyphaSubscriber) -> u64 {
    let mut count = u64::MAX;
    // SAFETY: `subscriber` is live; `count` is a local.
    let status = unsafe { hypha_subscriber_missing_sequences(subscriber, &raw mut count) };
    assert_eq!(status, HyphaStatus::Ok);
    count
}

/// Take one sample through the owning receive; `false` for an empty queue.
fn receive_one(subscriber: *mut HyphaSubscriber) -> bool {
    // SAFETY: `subscriber` is live.
    let received = unsafe { crate::common::receive::receive(subscriber) };
    received.expect("the receive is accepted").is_some()
}

#[test]
fn the_counters_equal_the_emitted_metrics_through_a_provoked_loss() {
    let node = create_node(&unique("counters"));
    let topic = format!("/hypha_ffi/counters/{}", unique("topic"));
    // SAFETY: `node` is live.
    let (publisher, subscriber) = unsafe { create_pubsub(node, &topic, 1) };
    let fresh = counters(subscriber);
    assert_eq!(fresh, emitted(subscriber));
    assert!(fresh.loan_budget > 0, "a subscriber has a loan budget");

    // The first accepted frame is each publisher's baseline.
    // SAFETY: the publisher is live.
    unsafe { send_payload(publisher, &[0]) };
    assert!(receive_one(subscriber));
    // A queue of one keeps the newest of five frames: four are lost.
    for index in 1_u8..=5 {
        // SAFETY: as above.
        unsafe { send_payload(publisher, &[index]) };
    }
    assert!(receive_one(subscriber));

    let after = counters(subscriber);
    assert_eq!(after, emitted(subscriber), "one source: the C and the Rust numbers agree");
    assert!(missing(subscriber) >= 4, "the loss is a count of missing sequences");
    // `sequence_gap` counts a duplicate or reordered frame, which a loss is not.
    assert_eq!(after.sequence_gap, 0);
    assert_eq!(after.loan_held, 0, "the owning receive returned its samples");

    // SAFETY: every handle is live and released once.
    unsafe {
        hypha_subscriber_destroy(subscriber);
        hypha_publisher_destroy(publisher);
        hypha_node_destroy(node);
    }
}

#[test]
fn a_null_subscriber_or_output_is_refused_and_writes_nothing() {
    let mut out = HyphaSubscriberCounters { sequence_gap: 7, ..HyphaSubscriberCounters::default() };
    // SAFETY: null is refused before anything is read or written.
    let null_subscriber = unsafe { hypha_subscriber_counters(ptr::null(), &raw mut out) };
    assert_eq!((null_subscriber, out.sequence_gap), (HyphaStatus::InvalidArgument, 7));

    let node = create_node(&unique("counters_null"));
    let topic = format!("/hypha_ffi/counters/{}", unique("topic"));
    // SAFETY: `node` is live.
    let (publisher, subscriber) = unsafe { create_pubsub(node, &topic, 1) };
    // SAFETY: the subscriber is live; the null output is refused.
    let null_out = unsafe { hypha_subscriber_counters(subscriber, ptr::null_mut()) };
    assert_eq!(null_out, HyphaStatus::InvalidArgument);
    // SAFETY: every handle is live and released once.
    unsafe {
        hypha_subscriber_destroy(subscriber);
        hypha_publisher_destroy(publisher);
        hypha_node_destroy(node);
    }
}
