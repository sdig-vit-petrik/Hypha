//! Integration tests for the owning receive API.

use hypha_node::sdk::{BusNode, RawSubscriber, SubscriberOptions};
use hypha_testkit::unique_name;

/// One named metric of a subscriber. Fails the test when the subscriber does not emit it.
#[allow(clippy::expect_used, reason = "a missing metric is a test setup failure")]
fn metric(subscriber: &RawSubscriber, name: &str) -> u64 {
    subscriber
        .metrics()
        .expect("metrics")
        .into_iter()
        .find(|(metric, _)| metric == name)
        .map(|(_, value)| value)
        .expect("a subscriber metric")
}

#[test]
#[allow(clippy::expect_used)]
fn loan_round_trip_reports_payload_and_identity() {
    let bus = BusNode::new(&unique_name("loan_rust")).expect("node");
    let mut publisher = bus
        .publish_raw("/rust/loan", "std_msgs/Bytes", "d9d213e8c146065ec958333c617eb1a6", "")
        .expect("publisher");
    let mut subscriber = bus
        .subscribe_raw_with_options("/rust/loan", SubscriberOptions::default().keeping(2))
        .expect("subscriber");
    assert!(publisher.publish(b"rust loan"));

    let sample = subscriber.try_receive().expect("receive").expect("sample");
    assert_eq!(sample.bytes(), Some(b"rust loan".as_slice()));
    let identity = sample.identity().expect("identity");
    assert_eq!(identity.topic, "/rust/loan");
    assert!(identity.transport_sent_nanos > 0);
    assert_eq!(metric(&subscriber, "loan.budget"), 2);
    assert_eq!(metric(&subscriber, "loan.held"), 1);
    drop(sample);
    assert_eq!(metric(&subscriber, "loan.held"), 0);
}
