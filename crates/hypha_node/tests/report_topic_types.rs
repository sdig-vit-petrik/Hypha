//! A node's own `~/state` and `~/introspection` carry the report types only.
#![allow(clippy::expect_used)]

use hypha_node::sdk::BusNode;
use hypha_node::sdk::message::MessageIdentity;
use hypha_node::sdk::msgs::hypha_msgs::{NodeIntrospection, NodeState};

const UINT8_MD5: &str = "7c8164229e7d2c17eb95e9231617fdee";

fn node(prefix: &str) -> BusNode {
    let name = format!("{prefix}_{}", std::process::id());
    BusNode::new(&name).expect("a test node")
}

#[test]
fn a_foreign_type_on_the_nodes_own_state_is_refused() {
    let bus = node("t14_state");
    let refused = bus.publish_raw("~/state", "std_msgs/UInt8", UINT8_MD5, "");
    assert!(refused.is_err(), "UInt8 on ~/state must be refused");
    let absolute = format!("/{}/state", bus.name().expect("the node name"));
    let refused = bus.publish_raw(&absolute, "std_msgs/UInt8", UINT8_MD5, "");
    assert!(refused.is_err(), "the absolute name is the same topic");
}

#[test]
fn a_foreign_type_on_the_nodes_own_introspection_is_refused() {
    let bus = node("t14_introspection");
    let refused = bus.publish_raw("~/introspection", "std_msgs/UInt8", UINT8_MD5, "");
    assert!(refused.is_err(), "UInt8 on ~/introspection must be refused");
}

#[test]
fn the_report_types_and_other_topics_are_accepted() {
    let bus = node("t14_accepted");
    let state = bus.publish_raw(
        "~/state",
        <NodeState as MessageIdentity>::TYPE_NAME,
        <NodeState as MessageIdentity>::SCHEMA_HASH,
        "",
    );
    assert!(state.is_ok(), "NodeState on ~/state: {:?}", state.err());
    let manifest = bus.publish_raw(
        "~/introspection",
        <NodeIntrospection as MessageIdentity>::TYPE_NAME,
        <NodeIntrospection as MessageIdentity>::SCHEMA_HASH,
        "",
    );
    assert!(manifest.is_ok(), "NodeIntrospection on ~/introspection: {:?}", manifest.err());
    let other = bus.publish_raw("/another_node/state", "std_msgs/UInt8", UINT8_MD5, "");
    assert!(other.is_ok(), "another node's topic is not checked: {:?}", other.err());
    let health = bus.publish_raw("~/input_health", "std_msgs/UInt8", UINT8_MD5, "");
    assert!(health.is_ok(), "{:?}", health.err());
}
