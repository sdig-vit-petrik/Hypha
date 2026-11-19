//! The integrity rejection counters a subscriber keeps.
#![allow(clippy::expect_used)]

use hypha_node::sdk::{BusNode, RawPublisher, RawSubscriber, SubscriberOptions};
use hypha_testkit::unique_name;

/// The `integrity.*` metrics of one subscriber: the frames it refused, by reason.
struct Integrity {
    invalid_frame: u64,
    corrupt_payload: u64,
    schema_mismatch: u64,
    sequence_gap: u64,
    invalid_timestamp: u64,
}

fn assert_no_rejections(stats: &Integrity, context: &str) {
    assert_eq!(stats.invalid_frame, 0, "{context}: invalid_frame");
    assert_eq!(stats.corrupt_payload, 0, "{context}: corrupt_payload");
    assert_eq!(stats.schema_mismatch, 0, "{context}: schema_mismatch");
    assert_eq!(stats.sequence_gap, 0, "{context}: sequence_gap");
    assert_eq!(stats.invalid_timestamp, 0, "{context}: invalid_timestamp");
}

struct Fixture {
    // The node outlives its endpoints: fields drop in declaration order.
    publisher: RawPublisher,
    subscriber: RawSubscriber,
    _bus: BusNode,
}

impl Fixture {
    fn new() -> Self {
        let bus = BusNode::new(&unique_name("node")).expect("node");
        let topic = format!("/hypha_node/integrity/{}", unique_name("topic"));
        let publisher = bus
            .publish_raw(&topic, "std_msgs/Bytes", "d9d213e8c146065ec958333c617eb1a6", "")
            .expect("publisher");
        let options = SubscriberOptions::default().keeping(4);
        let subscriber = bus.subscribe_raw_with_options(&topic, options).expect("subscriber");
        Self { publisher, subscriber, _bus: bus }
    }

    fn publish(&mut self, payload: &[u8]) {
        assert!(self.publisher.publish(payload));
    }

    fn stats(&self) -> Integrity {
        let list = self.subscriber.metrics().expect("metrics");
        let value = |name: &str| {
            let found = list.iter().find(|(metric, _)| metric == name);
            found.map(|(_, value)| *value).expect("an integrity metric")
        };
        Integrity {
            invalid_frame: value("integrity.invalid_frame"),
            corrupt_payload: value("integrity.corrupt_payload"),
            schema_mismatch: value("integrity.schema_mismatch"),
            sequence_gap: value("integrity.sequence_gap"),
            invalid_timestamp: value("integrity.invalid_timestamp"),
        }
    }
}

#[test]
fn a_subscriber_that_has_seen_nothing_reports_zero_rejections() {
    let fixture = Fixture::new();
    assert_no_rejections(&fixture.stats(), "a new subscriber");
}

#[test]
fn valid_traffic_never_increments_a_rejection_counter() {
    let mut fixture = Fixture::new();

    for index in 0_u8..4 {
        fixture.publish(&[index, index, index]);
        let received = fixture.subscriber.try_receive().expect("receive");
        assert!(received.is_some(), "a valid frame is accepted");
    }

    // Four well-formed frames in order: every counter stays zero.
    assert_no_rejections(&fixture.stats(), "after four valid frames");
}
