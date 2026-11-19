//! ROS 1 master graph role merging for automatic bridge mode.

#![allow(clippy::expect_used)]

use std::collections::BTreeMap;

use hypha_ros_bridge::automatic::RosTopicState;
use hypha_ros_bridge::ros_graph::add_roles;

const BRIDGE: &str = "/hypha_ros_bridge";

fn section(entries: &[(&str, &[&str])]) -> Vec<(String, Vec<String>)> {
    entries
        .iter()
        .map(|(topic, nodes)| {
            ((*topic).to_owned(), nodes.iter().map(|node| (*node).to_owned()).collect())
        })
        .collect()
}

#[test]
fn a_publisher_section_marks_the_topic_published() {
    let mut topics = BTreeMap::new();
    let publishers = section(&[("/legacy/text", &["/talker"])]);

    add_roles(&mut topics, Some(&publishers), true, BRIDGE).expect("publishers");

    let state = topics.get("/legacy/text").expect("topic");
    assert!(state.published());
    assert!(!state.subscribed);
}

#[test]
fn a_subscriber_section_marks_the_topic_subscribed() {
    let mut topics = BTreeMap::new();
    let subscribers = section(&[("/legacy/text", &["/listener"])]);

    add_roles(&mut topics, Some(&subscribers), false, BRIDGE).expect("subscribers");

    let state = topics.get("/legacy/text").expect("topic");
    assert!(!state.published());
    assert!(state.subscribed);
}

#[test]
fn the_bridge_never_counts_its_own_endpoint_as_demand() {
    let mut topics = BTreeMap::new();
    let subscribers = section(&[("/legacy/text", &[BRIDGE])]);

    add_roles(&mut topics, Some(&subscribers), false, BRIDGE).expect("subscribers");

    assert!(topics.is_empty(), "the bridge's own port must not create a route");
}

#[test]
fn an_external_node_beside_the_bridge_still_counts() {
    let mut topics = BTreeMap::new();
    let subscribers = section(&[("/legacy/text", &[BRIDGE, "/listener"])]);

    add_roles(&mut topics, Some(&subscribers), false, BRIDGE).expect("subscribers");

    assert!(topics.get("/legacy/text").expect("topic").subscribed);
}

#[test]
fn both_roles_merge_into_one_topic_entry() {
    let mut topics = BTreeMap::new();
    let publishers = section(&[("/legacy/text", &["/talker"])]);
    let subscribers = section(&[("/legacy/text", &["/listener"])]);

    add_roles(&mut topics, Some(&publishers), true, BRIDGE).expect("publishers");
    add_roles(&mut topics, Some(&subscribers), false, BRIDGE).expect("subscribers");

    assert_eq!(topics.len(), 1);
    let state = topics.get("/legacy/text").expect("topic");
    assert!(state.published() && state.subscribed);
}

#[test]
fn every_external_publisher_is_named_and_the_bridge_is_not() {
    let mut topics = BTreeMap::new();
    let publishers = section(&[("/legacy/text", &["/talker", BRIDGE, "/second_talker"])]);

    add_roles(&mut topics, Some(&publishers), true, BRIDGE).expect("publishers");

    let state = topics.get("/legacy/text").expect("topic");
    let names: Vec<&str> = state.publishers.iter().map(String::as_str).collect();
    assert_eq!(names, vec!["/second_talker", "/talker"]);
}

#[test]
fn a_missing_section_is_refused_rather_than_assumed_empty() {
    let mut topics: BTreeMap<String, RosTopicState> = BTreeMap::new();

    let outcome = add_roles(&mut topics, None, true, BRIDGE);

    assert!(outcome.is_err(), "an incomplete master response must not read as an empty graph");
}
