//! Endpoint option builders for publishers and subscribers.

#![allow(clippy::expect_used)]

use hypha_node::sdk::{PayloadCapacity, PublisherOptions, SubscriberOptions};

#[test]
fn publisher_defaults_are_the_restrictive_shape() {
    let options = PublisherOptions::default();

    assert!(!options.latch, "a publisher must not retain history unless asked");
    assert_eq!(options.max_payload, PayloadCapacity::default());
}

#[test]
fn each_publisher_setter_changes_only_its_own_field() {
    let base = PublisherOptions::default();

    assert!(base.latched().latch);
    assert!(base.with_latch(true).latch);
    assert!(!base.with_latch(false).latch);
    assert_eq!(base.latched().max_payload, base.max_payload);
}

#[test]
fn subscriber_defaults_are_the_restrictive_shape() {
    assert_eq!(SubscriberOptions::default().depth, 1);
}

#[test]
fn the_subscriber_depth_setter_sets_the_depth() {
    assert_eq!(SubscriberOptions::default().keeping(8).depth, 8);
}
