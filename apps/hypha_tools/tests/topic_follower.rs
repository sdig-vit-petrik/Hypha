//! Verifies that a topic follower picks up topics created after it starts.

use std::time::{Duration, Instant};

use hypha_node::sdk::BusNode;
use hypha_testkit::unique_name;
use hypha_tools::topic_follower::TopicFollower;

#[test]
#[allow(clippy::expect_used)]
fn a_topic_created_after_the_follower_is_picked_up() {
    let bus = BusNode::new(&unique_name("follow_rt")).expect("bus node");
    let prefix = format!("/{}", unique_name("follow"));
    let pattern = format!("{prefix}/*");

    let mut follower = TopicFollower::new(vec![pattern], 8, Duration::from_millis(50));
    follower.refresh(&bus).expect("initial refresh");
    assert_eq!(follower.followed(), 0, "nothing matches yet");

    let topic = format!("{prefix}/one");
    let mut publisher = bus
        .publish_raw(&topic, "std_msgs/Type", "d9d213e8c146065ec958333c617eb1a6", "")
        .expect("publisher");

    let deadline = Instant::now().checked_add(Duration::from_secs(3)).unwrap_or_else(Instant::now);
    let mut received = None;
    while Instant::now() < deadline && received.is_none() {
        follower.refresh_if_due(&bus).expect("refresh");
        assert!(publisher.publish(b"hello"));
        received = follower.next_sample().expect("receive");
        if received.is_none() {
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    let sample = received.expect("the follower must reach a topic created after it started");
    assert_eq!(sample.identity().map(|identity| identity.topic), Some(topic.as_str()));
    assert_eq!(follower.followed(), 1);
}

#[test]
#[allow(clippy::expect_used)]
fn a_topic_outside_the_keys_is_not_followed() {
    let bus = BusNode::new(&unique_name("follow_skip_rt")).expect("bus node");
    let wanted = format!("/{}", unique_name("wanted"));
    let other = format!("/{}", unique_name("other"));

    let mut publisher = bus
        .publish_raw(&other, "std_msgs/Type", "d9d213e8c146065ec958333c617eb1a6", "")
        .expect("publisher");
    assert!(publisher.publish(b"hello"));

    let mut follower = TopicFollower::new(vec![wanted], 8, Duration::from_millis(50));
    follower.refresh(&bus).expect("refresh");
    assert_eq!(follower.followed(), 0, "a topic outside the keys must be left alone");
}

#[test]
#[allow(clippy::expect_used)]
fn a_subscription_failure_is_reported() {
    let bus = BusNode::new(&unique_name("follow_limit_rt")).expect("bus node");
    let topic = format!("/{}", unique_name("limited"));
    let _publisher = bus
        .publish_raw(&topic, "std_msgs/Type", "d9d213e8c146065ec958333c617eb1a6", "")
        .expect("publisher");
    let mut subscribers = Vec::new();
    // An undeclared subscriber (a tool) holds only the service's three reserved observer
    // slots (`RESERVED_OBSERVER_SUBSCRIBERS` in hypha_iceoryx2): the fourth is refused. Tools
    // depend on the SDK only, so the count is written here.
    for _ in 0..3 {
        subscribers.push(bus.subscribe_raw(&topic, 1).expect("subscriber"));
    }

    let mut follower = TopicFollower::new(vec![topic.clone()], 1, Duration::from_millis(50));
    let error = follower.refresh(&bus).expect_err("the subscriber limit must be reported");
    assert!(error.contains(&topic));
}

#[test]
#[allow(clippy::expect_used)]
fn a_followed_topic_that_overflows_is_listed_by_lost() {
    let bus = BusNode::new(&unique_name("follow_lost_rt")).expect("bus node");
    let topic = format!("/{}", unique_name("lost"));
    let mut publisher = bus
        .publish_raw(&topic, "std_msgs/Type", "d9d213e8c146065ec958333c617eb1a6", "")
        .expect("publisher");

    let mut follower = TopicFollower::new(vec![topic.clone()], 1, Duration::from_millis(50));
    follower.refresh(&bus).expect("refresh");
    assert!(publisher.publish(&[0]));
    assert!(follower.next_sample().expect("receive").is_some());
    assert!(follower.lost().expect("lost").is_empty(), "the baseline loses nothing");

    for value in 1_u8..=5 {
        assert!(publisher.publish(&[value]));
    }
    assert!(follower.next_sample().expect("receive").is_some(), "the newest frame survives");
    let lost = follower.lost().expect("lost");
    assert_eq!(lost.len(), 1, "one topic overflowed: {lost:?}");
    let (name, missing) = lost.first().expect("the overflowing topic is listed");
    assert_eq!(name, &topic);
    assert!(*missing >= 4, "four frames were dropped, the count is {missing}");
}

#[test]
#[allow(clippy::expect_used)]
fn missing_lists_the_names_that_are_not_followed_yet() {
    let bus = BusNode::new(&unique_name("follow_missing_rt")).expect("bus node");
    let present = format!("/{}", unique_name("present"));
    let absent = format!("/{}", unique_name("absent"));
    let _publisher = bus
        .publish_raw(&present, "std_msgs/Type", "d9d213e8c146065ec958333c617eb1a6", "")
        .expect("publisher");
    let wanted = vec![present.clone(), absent.clone()];
    let mut follower = TopicFollower::new(wanted.clone(), 8, Duration::from_millis(50));
    follower.refresh(&bus).expect("refresh");
    assert_eq!(follower.missing(&wanted), vec![absent]);
}
