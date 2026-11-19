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
