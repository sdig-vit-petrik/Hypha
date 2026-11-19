//! A Rust node reads the schema counters that the library writes, by name.
#![allow(clippy::expect_used)]

use hypha_node::sdk::BusNode;
use hypha_testkit::unique_name as unique;

/// One named metric of the node. Fails the test when the node does not emit it.
fn metric(bus: &BusNode, name: &str) -> u64 {
    bus.metrics()
        .expect("metrics")
        .into_iter()
        .find(|(metric, _)| metric == name)
        .map(|(_, value)| value)
        .expect("a node metric")
}

#[test]
fn a_fresh_node_and_a_schema_bearing_publisher_report_no_schema_failure() {
    let bus = BusNode::new(&unique("fresh")).expect("node");
    assert_eq!(metric(&bus, "schema.invalid_records"), 0);
    assert_eq!(metric(&bus, "schema.unavailable_lookups"), 0);
    assert_eq!(metric(&bus, "schema.resource_limit_failures"), 0);

    let topic = format!("/{}", unique("topic"));
    let _publisher = bus
        .publish_raw(&topic, "demo/SchemaHealth", "59d6cdadeb71a413b4436c0850ca8d5e", "uint32 v\n")
        .expect("publisher");

    assert_eq!(metric(&bus, "schema.invalid_records"), 0);
    assert_eq!(metric(&bus, "schema.resource_limit_failures"), 0);
}
