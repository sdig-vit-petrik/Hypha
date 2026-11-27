//! The follower follows streams, not names (#209): a version that joins a followed name later
//! is read, each stream on its own, and the flags that name a type or an MD5 follow only
//! the streams they select.
#![allow(clippy::expect_used, clippy::print_stdout, clippy::panic)]

use std::time::{Duration, Instant};

use hypha_node::sdk::{BusNode, RawPublisher, StreamKey, StreamSelector};
use hypha_testkit::unique_name;

#[path = "support/following.rs"]
mod following;
use following::Following;

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
    follower: &mut Following,
    bus: &BusNode,
    seconds: u64,
    want: usize,
) -> Vec<(u64, Vec<u8>)> {
    let deadline = Instant::now().checked_add(Duration::from_secs(seconds)).expect("deadline");
    let mut seen = Vec::new();
    while Instant::now() < deadline && seen.len() < want {
        follower.settle(bus).expect("refresh");
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

/// Waits until the follower reads `streams` streams, on the graph event: `settle` lists only
/// when an event came, so a lost wake-up or the backstop would not pass it. How long it takes
/// is the listing's, which the domain and the machine decide.
fn wait_for_streams(follower: &mut Following, bus: &BusNode, streams: usize) -> Duration {
    let started = Instant::now();
    while follower.followed() < streams {
        assert!(started.elapsed() < Duration::from_secs(20), "the stream is not followed");
        follower.settle(bus).expect("settle");
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
        Following::new(&bus, vec![format!("{prefix}/*")], StreamSelector::default(), 8);
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
    let mut follower = Following::new(&bus, vec![topic.clone()], StreamSelector::default(), 8);
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

    let by_md5 = StreamSelector { data_type: None, md5_prefix: Some(V2[..8].to_owned()) };
    let mut follower = Following::new(&bus, vec![topic.clone()], by_md5, 8);
    follower.refresh(&bus).expect("refresh");
    assert_eq!(follower.followed(), 1, "one version by its MD5 prefix");
    assert!(v1.publish(&[1, 0, 0, 0]));
    assert!(v2.publish(&[2, 0, 0, 0, 0, 0, 0, 0]));
    let seen = read_for(&mut follower, &bus, 1, 1);
    assert_eq!(seen, vec![(hash(V2), vec![2, 0, 0, 0, 0, 0, 0, 0])]);

    let by_type = StreamSelector { data_type: Some("std_msgs/Other".to_owned()), md5_prefix: None };
    let mut none = Following::new(&bus, vec![topic], by_type, 8);
    none.refresh(&bus).expect("refresh");
    assert_eq!(none.followed(), 0, "no stream of that type");
}

/// The call the tools make on each pass, which lists at most once per spacing, follows a stream
/// that joins too.
#[test]
fn a_spaced_refresh_follows_a_stream_that_joins_after_a_listing() {
    let bus = BusNode::new(&unique_name("rv_spaced")).expect("bus node");
    let peer = BusNode::new(&unique_name("rv_spaced_peer")).expect("peer");
    let topic = format!("/{}/odom", unique_name("rv_spaced"));
    let mut follower = Following::new(&bus, vec![topic.clone()], StreamSelector::default(), 8);
    follower.refresh(&bus).expect("refresh");
    let _v1 = publish_version(&peer, &topic, V1, DEFINITION_1);
    let started = Instant::now();
    while follower.followed() == 0 {
        assert!(started.elapsed() < Duration::from_secs(20), "the stream is not followed");
        follower.settle(&bus).expect("refresh");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The frames a publisher sent before the follower followed its stream are told, so a
/// recording that begins late says so (H1 of the fourth review of #268).
#[test]
fn a_stream_that_joins_after_the_follower_started_tells_what_it_sent_before() {
    let bus = BusNode::new(&unique_name("rv_before")).expect("bus node");
    let peer = BusNode::new(&unique_name("rv_before_peer")).expect("peer");
    let topic = format!("/{}/odom", unique_name("rv_before"));
    let mut follower = Following::new(&bus, vec![topic.clone()], StreamSelector::default(), 8);
    follower.refresh(&bus).expect("first listing");
    let mut v1 = publish_version(&peer, &topic, V1, DEFINITION_1);
    for count in 0_u32..25 {
        assert!(v1.publish(&count.to_le_bytes()));
    }
    wait_for_streams(&mut follower, &bus, 1);
    assert!(v1.publish(&25_u32.to_le_bytes()));
    assert_eq!(read_for(&mut follower, &bus, 2, 1).len(), 1);
    let key = StreamKey { topic, schema_hash: hash(V1) };
    assert_eq!(follower.sent_before_following(), vec![(key, 25)], "25 frames were sent first");
}

/// A publisher that ran before the follower started has a history that is not the recording's.
#[test]
fn a_publisher_that_ran_before_the_follower_is_not_counted() {
    let bus = BusNode::new(&unique_name("rv_ran")).expect("bus node");
    let peer = BusNode::new(&unique_name("rv_ran_peer")).expect("peer");
    let topic = format!("/{}/odom", unique_name("rv_ran"));
    let mut v1 = publish_version(&peer, &topic, V1, DEFINITION_1);
    for count in 0_u32..40 {
        assert!(v1.publish(&count.to_le_bytes()));
    }
    let mut follower = Following::new(&bus, vec![topic], StreamSelector::default(), 8);
    follower.refresh(&bus).expect("first listing");
    assert!(v1.publish(&40_u32.to_le_bytes()));
    assert_eq!(read_for(&mut follower, &bus, 2, 1).len(), 1);
    assert_eq!(follower.sent_before_following(), vec![]);
}

/// A publisher that joins a followed stream is counted from the listing that showed it.
#[test]
fn a_publisher_that_joins_a_followed_stream_is_counted_from_its_listing() {
    let bus = BusNode::new(&unique_name("rv_join")).expect("bus node");
    let peer = BusNode::new(&unique_name("rv_join_peer")).expect("peer");
    let topic = format!("/{}/odom", unique_name("rv_join"));
    let mut first = publish_version(&peer, &topic, V1, DEFINITION_1);
    let mut follower = Following::new(&bus, vec![topic.clone()], StreamSelector::default(), 4);
    follower.refresh(&bus).expect("first listing");
    assert!(first.publish(&[0, 0, 0, 0]));
    assert_eq!(read_for(&mut follower, &bus, 2, 1).len(), 1);

    let mut second = publish_version(&peer, &topic, V1, DEFINITION_1);
    let started = Instant::now();
    while follower.publishers(&StreamKey { topic: topic.clone(), schema_hash: hash(V1) }) != Some(2)
    {
        assert!(started.elapsed() < Duration::from_secs(20), "the second publisher is not listed");
        follower.refresh(&bus).expect("listing");
        std::thread::sleep(Duration::from_millis(10));
    }
    // Twenty frames before a read overrun the small queue: what the follower reads of them
    // starts late, and the frames before the first it accepted are the count.
    for count in 0_u32..20 {
        assert!(second.publish(&count.to_le_bytes()));
    }
    let mut received = 0_u64;
    while let Some(_sample) = follower.next_sample().expect("receive") {
        received += 1;
    }
    assert!(received > 0 && received < 20, "the queue overran: {received} read");
    let key = StreamKey { topic, schema_hash: hash(V1) };
    assert_eq!(follower.sent_before_following(), vec![(key, 20 - received)]);
}

/// A latched publisher hands a late reader its last sample, which is all that is wanted of it.
#[test]
fn a_latched_streams_sample_is_not_counted_as_sent_before_following() {
    let bus = BusNode::new(&unique_name("rv_latch")).expect("bus node");
    let peer = BusNode::new(&unique_name("rv_latch_peer")).expect("peer");
    let topic = format!("/{}/tf_static", unique_name("rv_latch"));
    let mut follower = Following::new(&bus, vec![topic.clone()], StreamSelector::default(), 8);
    follower.refresh(&bus).expect("first listing");
    let mut latched = peer
        .publish_raw_configured(
            &topic,
            "std_msgs/Reading",
            V1,
            DEFINITION_1,
            hypha_node::sdk::PublisherOptions::default().latched(),
        )
        .expect("latched publisher");
    for count in 0_u32..5 {
        assert!(latched.publish(&count.to_le_bytes()));
    }
    wait_for_streams(&mut follower, &bus, 1);
    assert!(latched.publish(&5_u32.to_le_bytes()));
    assert!(!read_for(&mut follower, &bus, 2, 1).is_empty());
    assert_eq!(follower.sent_before_following(), vec![]);
}

/// A flag that matches nothing leaves the streams of the name to be named to the reader.
#[test]
fn the_streams_a_selector_lets_go_are_kept_with_their_schema() {
    let bus = BusNode::new(&unique_name("rv_skip")).expect("bus node");
    let peer = BusNode::new(&unique_name("rv_skip_peer")).expect("peer");
    let topic = format!("/{}/odom", unique_name("rv_skip"));
    let _v1 = publish_version(&peer, &topic, V1, DEFINITION_1);
    let _bare = peer.publish_raw(&topic, "std_msgs/Reading", V2, "").expect("no definition");
    let by_type = StreamSelector { data_type: Some("std_msgs/Other".to_owned()), md5_prefix: None };
    let mut follower = Following::new(&bus, vec![topic], by_type, 8);
    follower.refresh(&bus).expect("refresh");
    assert_eq!(follower.followed(), 0);
    let skipped = follower.skipped();
    assert_eq!(skipped.len(), 2);
    let known: Vec<_> = skipped.iter().filter_map(|(_, schema)| schema.as_ref()).collect();
    assert_eq!(known.len(), 1, "one of them has no definition to name it by");
}
