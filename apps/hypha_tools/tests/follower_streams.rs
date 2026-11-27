//! The follower follows streams, not names (#209): a version that joins a followed name later
//! is read, each stream on its own, and the flags that name a type or an MD5 follow only
//! the streams they select.
#![allow(clippy::expect_used, clippy::print_stdout, clippy::panic)]

use std::time::{Duration, Instant};

use hypha_node::sdk::{BusNode, RawPublisher};
use hypha_testkit::unique_name;
use hypha_tools::stream::{StreamKey, StreamSelector};
use hypha_tools::stream_follower::StreamFollower;

const V1: &str = "b3087778e93fcd34cc8d65bc54e850d1";
const V2: &str = "f79835def461350458ead76f07016dd7";
const DEFINITION_1: &str = "int32 value\n";
const DEFINITION_2: &str = "int32 value\nstring label\nfloat64 weight\n";

fn publish_version(bus: &BusNode, topic: &str, md5: &str, definition: &str) -> RawPublisher {
    bus.publish_raw(topic, "std_msgs/Reading", md5, definition)
        .unwrap_or_else(|error| panic!("publisher of {md5}: {error}"))
}

/// The payloads the follower reads within `seconds`, until `want` came, each with the hash
/// of the stream it came on.
fn read_for(
    follower: &mut StreamFollower,
    bus: &BusNode,
    seconds: u64,
    want: usize,
) -> Vec<(u64, Vec<u8>)> {
    let deadline = Instant::now().checked_add(Duration::from_secs(seconds)).expect("deadline");
    let mut seen = Vec::new();
    while Instant::now() < deadline && seen.len() < want {
        follower.refresh_if_changed(bus).expect("refresh");
        while let Some(sample) = follower.next_sample().expect("receive") {
            let identity = sample.identity().expect("identity");
            seen.push((identity.schema_hash, sample.bytes().expect("bytes").to_vec()));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    seen
}

fn hash(md5: &str) -> u64 {
    hypha_node::sdk::schema_hash_from_md5(md5).expect("md5")
}

/// Waits until the follower reads `streams` streams, by its own graph event, and says how long.
fn wait_for_streams(follower: &mut StreamFollower, bus: &BusNode, streams: usize) -> Duration {
    let started = Instant::now();
    while follower.followed() < streams {
        assert!(started.elapsed() < Duration::from_secs(1), "the stream is followed on its event");
        follower.refresh_if_changed(bus).expect("refresh");
        std::thread::sleep(Duration::from_millis(1));
    }
    started.elapsed()
}

/// The pinned gap of part one, turned round: the version that joins later is read, on its
/// own stream, and the first version is read on as before.
#[test]
fn a_second_version_that_joins_a_followed_topic_is_followed_too() {
    let bus = BusNode::new(&unique_name("rv_follow")).expect("bus node");
    let peer = BusNode::new(&unique_name("rv_follow_peer")).expect("peer");
    let prefix = format!("/{}", unique_name("rv_follow"));
    let topic = format!("{prefix}/odom");
    let mut follower =
        StreamFollower::new(&bus, vec![format!("{prefix}/*")], StreamSelector::default(), 8)
            .expect("follower");
    follower.refresh(&bus).expect("initial refresh");
    assert_eq!(follower.followed(), 0, "nothing matches yet");

    let mut v1 = publish_version(&peer, &topic, V1, DEFINITION_1);
    let first = wait_for_streams(&mut follower, &bus, 1);
    assert!(v1.publish(&[1, 0, 0, 0]));
    assert_eq!(read_for(&mut follower, &bus, 2, 1), vec![(hash(V1), vec![1, 0, 0, 0])]);

    let mut v2 = publish_version(&peer, &topic, V2, DEFINITION_2);
    let second = wait_for_streams(&mut follower, &bus, 2);
    println!("v1 followed {first:?} after it was published, v2 {second:?}");
    assert!(v1.publish(&[2, 0, 0, 0]));
    assert!(v2.publish(&[3, 0, 0, 0, 0, 0, 0, 0]));
    assert!(v1.publish(&[4, 0, 0, 0]));
    assert!(v2.publish(&[5, 0, 0, 0, 0, 0, 0, 0]));
    let after = read_for(&mut follower, &bus, 2, 4);
    let of = |wanted: u64| -> Vec<Vec<u8>> {
        after.iter().filter(|(h, _)| *h == wanted).map(|(_, bytes)| bytes.clone()).collect()
    };
    assert_eq!(of(hash(V1)), vec![vec![2, 0, 0, 0], vec![4, 0, 0, 0]], "v1, in order");
    assert_eq!(of(hash(V2)), vec![vec![3, 0, 0, 0, 0, 0, 0, 0], vec![5, 0, 0, 0, 0, 0, 0, 0]]);
    let key = StreamKey { topic, schema_hash: hash(V1) };
    assert!(follower.shares_its_name(&key), "the name now has two streams");
}

#[test]
fn a_stream_that_leaves_and_comes_back_is_not_followed_twice() {
    let bus = BusNode::new(&unique_name("rv_twice")).expect("bus node");
    let peer = BusNode::new(&unique_name("rv_twice_peer")).expect("peer");
    let topic = format!("/{}/odom", unique_name("rv_twice"));
    let mut follower = StreamFollower::new(&bus, vec![topic.clone()], StreamSelector::default(), 8)
        .expect("follower");
    let first = publish_version(&peer, &topic, V1, DEFINITION_1);
    wait_for_streams(&mut follower, &bus, 1);
    drop(first);
    let mut again = publish_version(&peer, &topic, V1, DEFINITION_1);
    assert!(again.publish(&[7, 0, 0, 0]));
    assert_eq!(read_for(&mut follower, &bus, 2, 1).len(), 1);
    assert_eq!(follower.followed(), 1);
}

#[test]
fn a_type_or_an_md5_flag_follows_only_the_streams_it_selects() {
    let bus = BusNode::new(&unique_name("rv_select")).expect("bus node");
    let peer = BusNode::new(&unique_name("rv_select_peer")).expect("peer");
    let topic = format!("/{}/odom", unique_name("rv_select"));
    let mut v1 = publish_version(&peer, &topic, V1, DEFINITION_1);
    let mut v2 = publish_version(&peer, &topic, V2, DEFINITION_2);

    let by_md5 = StreamSelector { data_type: None, md5: Some(V2[..8].to_owned()) };
    let mut follower = StreamFollower::new(&bus, vec![topic.clone()], by_md5, 8).expect("follower");
    follower.refresh(&bus).expect("refresh");
    assert_eq!(follower.followed(), 1, "one version by its MD5 prefix");
    assert!(v1.publish(&[1, 0, 0, 0]));
    assert!(v2.publish(&[2, 0, 0, 0, 0, 0, 0, 0]));
    let seen = read_for(&mut follower, &bus, 1, 1);
    assert_eq!(seen, vec![(hash(V2), vec![2, 0, 0, 0, 0, 0, 0, 0])]);

    let by_type = StreamSelector { data_type: Some("std_msgs/Other".to_owned()), md5: None };
    let mut none = StreamFollower::new(&bus, vec![topic], by_type, 8).expect("follower");
    none.refresh(&bus).expect("refresh");
    assert_eq!(none.followed(), 0, "no stream of that type");
}
