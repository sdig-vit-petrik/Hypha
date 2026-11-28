//! Verifies that a stream follower picks up streams created after it starts.

use std::time::{Duration, Instant};

use hypha_node::sdk::{BusNode, StreamSelector};
use hypha_testkit::unique_name;

#[path = "support/following.rs"]
mod following;
use following::Following;

#[test]
#[allow(clippy::expect_used)]
fn a_topic_created_after_the_follower_is_picked_up() {
    let bus = BusNode::new(&unique_name("follow_rt")).expect("bus node");
    let prefix = format!("/{}", unique_name("follow"));
    let pattern = format!("{prefix}/*");

    let mut follower = Following::new(&bus, vec![pattern], StreamSelector::default(), 8);
    follower.refresh(&bus).expect("initial refresh");
    assert_eq!(follower.followed(), 0, "nothing matches yet");

    let topic = format!("{prefix}/one");
    let mut publisher = bus
        .publish_raw(&topic, "std_msgs/Type", "d9d213e8c146065ec958333c617eb1a6", "")
        .expect("publisher");

    let deadline = Instant::now().checked_add(Duration::from_secs(3)).unwrap_or_else(Instant::now);
    let mut received = None;
    while Instant::now() < deadline && received.is_none() {
        follower.settle(&bus).expect("refresh");
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

    let mut follower = Following::new(&bus, vec![wanted], StreamSelector::default(), 8);
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
    // An undeclared subscriber (a tool) holds only the service's reserved observer slots:
    // the one after them is refused.
    for _ in 0..hypha_node::sdk::UNDECLARED_SUBSCRIBER_SLOTS {
        subscribers.push(bus.subscribe_raw(&topic, 1).expect("subscriber"));
    }

    let mut follower = Following::new(&bus, vec![topic.clone()], StreamSelector::default(), 1);
    let error = follower.refresh(&bus).expect_err("the subscriber limit must be reported");
    assert!(error.to_string().contains(&topic), "{error}");
    assert!(
        matches!(error, hypha_node::sdk::FollowError::Refused(ref refusals) if refusals.iter().all(|refusal| refusal.cause == hypha_node::sdk::StreamSubscribeError::SlotsTaken)),
        "{error:?}"
    );
}

#[test]
#[allow(clippy::expect_used)]
fn a_followed_topic_that_overflows_is_listed_by_lost() {
    let bus = BusNode::new(&unique_name("follow_lost_rt")).expect("bus node");
    let topic = format!("/{}", unique_name("lost"));
    let mut publisher = bus
        .publish_raw(&topic, "std_msgs/Type", "d9d213e8c146065ec958333c617eb1a6", "")
        .expect("publisher");

    let mut follower = Following::new(&bus, vec![topic.clone()], StreamSelector::default(), 1);
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
    let (stream, missing) = lost.first().expect("the overflowing topic is listed");
    assert_eq!(stream.topic, topic);
    assert!(*missing >= 4, "four frames were dropped, the count is {missing}");
}

#[test]
#[allow(clippy::expect_used)]
fn missing_lists_the_names_that_have_no_followed_stream_with_a_publisher() {
    let bus = BusNode::new(&unique_name("follow_missing_rt")).expect("bus node");
    let peer = BusNode::new(&unique_name("follow_missing_peer")).expect("peer");
    let present = format!("/{}", unique_name("present"));
    let absent = format!("/{}", unique_name("absent"));
    let _publisher = peer
        .publish_raw(&present, "std_msgs/Type", "d9d213e8c146065ec958333c617eb1a6", "")
        .expect("publisher");
    let wanted = vec![present.clone(), absent.clone()];
    let mut follower = Following::new(&bus, wanted.clone(), StreamSelector::default(), 8);
    follower.refresh(&bus).expect("refresh");
    let missing = follower.missing(&wanted);
    assert!(missing.contains(&absent), "{missing:?}");
    assert!(!missing.contains(&present), "{missing:?}");
}
