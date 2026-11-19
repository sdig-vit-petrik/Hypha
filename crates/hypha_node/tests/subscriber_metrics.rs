//! The metrics of one subscriber: every name, in order. The names are part of the SDK.
#![allow(clippy::expect_used)]

use hypha_node::sdk::{BusNode, SubscriberOptions};
use hypha_testkit::unique_name;

const SUBSCRIBER_NAMES: [&str; 8] = [
    "integrity.invalid_frame",
    "integrity.corrupt_payload",
    "integrity.schema_mismatch",
    "integrity.sequence_gap",
    "integrity.invalid_timestamp",
    "loan.budget",
    "loan.held",
    "loan.refused",
];

#[test]
fn subscriber_metrics_list_integrity_and_loan_names() {
    let bus = BusNode::new(&unique_name("metrics_sub")).expect("node");
    let topic = format!("/hypha_node/metrics/{}", unique_name("topic"));
    // A subscription without a type opens only a topic that exists.
    let _publisher = bus
        .publish_raw(&topic, "std_msgs/Data", "d9d213e8c146065ec958333c617eb1a6", "")
        .expect("publisher");
    let options = SubscriberOptions::default().keeping(4);
    let subscriber = bus.subscribe_raw_with_options(&topic, options).expect("subscriber");

    let list = subscriber.metrics().expect("metrics");

    let names: Vec<&str> = list.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, SUBSCRIBER_NAMES.to_vec());
    let budget = list.iter().find(|(name, _)| name == "loan.budget").map(|(_, value)| *value);
    assert!(budget.is_some_and(|value| value > 0), "a subscriber has a loan budget: {budget:?}");
}
