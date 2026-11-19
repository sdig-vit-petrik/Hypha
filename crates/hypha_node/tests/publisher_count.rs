//! The SDK view of the publisher count for a raw subscriber.

#![allow(clippy::expect_used)]

use std::time::{Duration, Instant};

use hypha_node::sdk::{BusNode, SubscriberOptions};
use hypha_testkit::unique_name;

/// Polls `publisher_count` until it equals `expected` or one second passes.
fn wait_for_count(
    subscriber: &hypha_node::sdk::RawSubscriber,
    expected: usize,
) -> Result<usize, String> {
    let Some(deadline) = Instant::now().checked_add(Duration::from_secs(1)) else {
        return subscriber.publisher_count();
    };
    loop {
        let current = subscriber.publisher_count()?;
        if current == expected || Instant::now() >= deadline {
            return Ok(current);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn no_publisher_counts_zero() {
    let node = BusNode::new(&unique_name("pubcount_none")).expect("node");
    let topic = format!("/{}", unique_name("pubcount_none_topic"));
    // A publisher opens the service, the subscriber joins it, and the publisher leaves.
    let opener = node
        .publish_raw(&topic, "test/Bytes", "d9d213e8c146065ec958333c617eb1a6", "")
        .expect("opener");
    let subscriber = node
        .subscribe_raw_with_schema(&topic, "test/Bytes", "", SubscriberOptions::default())
        .expect("subscriber");
    drop(opener);

    assert_eq!(subscriber.publisher_count(), Ok(0));
}

#[test]
fn one_publisher_counts_one() {
    let node = BusNode::new(&unique_name("pubcount_one")).expect("node");
    let topic = format!("/{}", unique_name("pubcount_one_topic"));
    let _publisher = node
        .publish_raw(&topic, "test/Bytes", "d9d213e8c146065ec958333c617eb1a6", "")
        .expect("publisher");
    let subscriber = node
        .subscribe_raw_with_schema(&topic, "test/Bytes", "", SubscriberOptions::default())
        .expect("subscriber");

    assert_eq!(wait_for_count(&subscriber, 1), Ok(1));
}
