//! The recorder's follower keys by topic name, a stream by name and schema: what it does
//! not record yet (part two of #209).
#![allow(clippy::expect_used, clippy::print_stdout, clippy::panic)]

use std::time::{Duration, Instant};

use hypha_node::sdk::BusNode;
use hypha_testkit::unique_name;
use hypha_tools::topic_follower::TopicFollower;

const V1: &str = "d9d213e8c146065ec958333c617eb1a6";
const V2: &str = "f79835def461350458ead76f07016dd7";

fn hashes_seen(follower: &mut TopicFollower, bus: &BusNode, rounds: u32) -> Vec<u64> {
    let mut seen = Vec::new();
    for _ in 0..rounds {
        follower.refresh_if_due(bus).expect("refresh");
        while let Some(sample) = follower.next_sample().expect("receive") {
            let hash = sample.identity().expect("identity").schema_hash;
            if !seen.contains(&hash) {
                seen.push(hash);
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    seen
}

/// The common case: one type, created after the recorder started, is recorded.
/// A second version that joins the followed name later is never recorded, and the
/// follower says nothing about it.
#[test]
fn a_second_version_that_joins_a_followed_topic_is_not_recorded() {
    let bus = BusNode::new(&unique_name("rv_follow")).expect("bus node");
    let peer = BusNode::new(&unique_name("rv_follow_peer")).expect("peer");
    let prefix = format!("/{}", unique_name("rv_follow"));
    let topic = format!("{prefix}/odom");
    let mut follower =
        TopicFollower::new(vec![format!("{prefix}/*")], 8, Duration::from_millis(50));
    follower.refresh(&bus).expect("initial refresh");

    let mut v1 = peer
        .publish_raw(&topic, "std_msgs/Type", V1, "")
        .unwrap_or_else(|error| panic!("v1: {error}"));
    let deadline = Instant::now().checked_add(Duration::from_secs(5)).expect("deadline");
    while follower.followed() == 0 {
        assert!(Instant::now() < deadline, "the v1 topic is never followed");
        follower.refresh_if_due(&bus).expect("refresh");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(v1.publish(&[1, 0, 0, 0]));
    let before = hashes_seen(&mut follower, &bus, 10);

    let mut v2 = peer
        .publish_raw(&topic, "std_msgs/Type", V2, "")
        .unwrap_or_else(|error| panic!("v2: {error}"));
    for _ in 0..20 {
        assert!(v1.publish(&[2, 0, 0, 0]));
        assert!(v2.publish(&[3, 0, 0, 0, 4, 0, 0, 0]));
    }
    let after = hashes_seen(&mut follower, &bus, 25);
    let listed: Vec<u64> = bus
        .list_topics()
        .expect("listing")
        .into_iter()
        .filter(|entry| entry.name == topic && entry.published)
        .map(|entry| entry.schema_hash)
        .collect();
    println!(
        "followed {} topic(s); hashes before v2 {before:x?}, after {after:x?}; listed {listed:x?}",
        follower.followed()
    );
    assert_eq!(before.len(), 1, "the single-type topic created after the start is recorded");
    assert_eq!(listed.len(), 2, "the bus lists both versions");
    assert_eq!(after.len(), 1, "only the stream that existed when the name was first followed");
}
