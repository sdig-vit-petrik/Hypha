//! The SDK view of the missing-sequence count for a raw subscriber.

use hypha_node::sdk::{BusNode, SubscriberOptions};
use hypha_testkit::unique_name;

#[test]
#[allow(clippy::expect_used)]
fn steady_traffic_reports_no_missing_sequence() {
    let bus = BusNode::new(&unique_name("missing_steady_rust")).expect("node");
    let topic = format!("/{}", unique_name("missing_steady"));
    let mut publisher = bus
        .publish_raw(&topic, "test/Bytes", "d9d213e8c146065ec958333c617eb1a6", "")
        .expect("publisher");
    let mut subscriber = bus
        .subscribe_raw_with_options(&topic, SubscriberOptions::default().keeping(1))
        .expect("subscriber");
    assert_eq!(subscriber.missing_sequences().expect("count"), 0);

    for value in 0_u8..4 {
        assert!(publisher.publish(&[value]));
        assert!(subscriber.try_receive().expect("receive").is_some());
    }
    assert_eq!(subscriber.missing_sequences().expect("count"), 0);
}

#[test]
#[allow(clippy::expect_used)]
fn an_overflowing_queue_reports_the_samples_it_dropped() {
    let bus = BusNode::new(&unique_name("missing_overflow_rust")).expect("node");
    let topic = format!("/{}", unique_name("missing_overflow"));
    let mut publisher = bus
        .publish_raw(&topic, "test/Bytes", "d9d213e8c146065ec958333c617eb1a6", "")
        .expect("publisher");
    let mut subscriber = bus
        .subscribe_raw_with_options(&topic, SubscriberOptions::default().keeping(1))
        .expect("subscriber");

    // The first accepted frame is the baseline the overflow is measured from.
    assert!(publisher.publish(&[0]));
    assert!(subscriber.try_receive().expect("receive").is_some());
    for value in 1_u8..=5 {
        assert!(publisher.publish(&[value]));
    }
    assert!(subscriber.try_receive().expect("receive").is_some(), "the newest frame survives");

    let missing = subscriber.missing_sequences().expect("count");
    assert!(missing >= 4, "four frames were dropped, the count reports {missing}");
}
