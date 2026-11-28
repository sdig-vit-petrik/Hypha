//! Automatic same-name route reconciliation tests.

use std::collections::BTreeSet;

use hypha_ros_bridge::automatic::{AutoFlow, HyphaTopicState, RosTopicState, desired_flows};
use hypha_ros_bridge::is_latched_topic;

fn ros(published: bool, subscribed: bool) -> RosTopicState {
    let mut publishers = BTreeSet::new();
    if published {
        publishers.insert("/talker".to_owned());
    }
    RosTopicState { publishers, subscribed, type_name: "std_msgs/String".into() }
}

fn hypha(published: bool, subscribed: bool) -> HyphaTopicState {
    HyphaTopicState { published, subscribed }
}

#[test]
fn creates_each_cross_side_pipe_only_when_both_endpoints_exist() {
    let flows = desired_flows(&ros(true, true), &hypha(true, true));
    assert_eq!(flows, vec![AutoFlow::RosToHypha, AutoFlow::HyphaToRos]);

    let flows = desired_flows(&ros(true, false), &hypha(false, true));
    assert_eq!(flows, vec![AutoFlow::RosToHypha]);

    let flows = desired_flows(&ros(false, true), &hypha(true, false));
    assert_eq!(flows, vec![AutoFlow::HyphaToRos]);
}

#[test]
fn does_not_create_a_pipe_without_a_remote_consumer() {
    assert_eq!(desired_flows(&ros(true, false), &hypha(true, false)).len(), 0);
    assert_eq!(desired_flows(&ros(false, true), &hypha(false, true)).len(), 0);
}

#[test]
fn a_topic_with_only_one_side_is_not_bridged() {
    assert_eq!(desired_flows(&ros(true, false), &HyphaTopicState::default()).len(), 0);
    assert_eq!(desired_flows(&RosTopicState::default(), &hypha(true, false)).len(), 0);
}

#[test]
fn only_tf_static_uses_the_ros_latched_topic_convention() {
    assert!(is_latched_topic("/tf_static"));
    assert!(!is_latched_topic("/tf"));
    assert!(!is_latched_topic("/map"));
}

#[test]
fn the_flags_of_one_stream_of_a_topic_do_not_hide_those_of_another() {
    use hypha_node::sdk::StreamKey;
    use hypha_ros_bridge::automatic::topic_states;
    use std::collections::BTreeMap;

    let stream = |topic: &str, schema_hash: u64| StreamKey { topic: topic.to_owned(), schema_hash };
    // Version 2 is published and read; version 1 only sits there: the order of the two in
    // the listing must not decide what the name is.
    for (first, second) in [(1_u64, 2_u64), (2, 1)] {
        let streams = BTreeMap::from([
            (stream("/odom", first), hypha(first == 2, first == 2)),
            (stream("/odom", second), hypha(second == 2, second == 2)),
            (stream("/scan", 1), hypha(false, true)),
        ]);
        let topics = topic_states(&streams);
        assert_eq!(topics.get("/odom"), Some(&hypha(true, true)));
        assert_eq!(topics.get("/scan"), Some(&hypha(false, true)));
    }
}
