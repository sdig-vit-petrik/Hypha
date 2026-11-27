//! What a reader needs to follow the streams of a name as they appear (#209): a watch of the
//! graph event, and a subscription to one stream by its schema hash.
#![allow(clippy::expect_used, clippy::panic)]

use std::time::{Duration, Instant};

use hypha_node::sdk::{BusNode, SubscriberOptions, schema_hash_from_md5};
use hypha_testkit::unique_name;

const V1: &str = "b3087778e93fcd34cc8d65bc54e850d1";
const V2: &str = "f79835def461350458ead76f07016dd7";

fn within(seconds: u64, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now().checked_add(Duration::from_secs(seconds)).expect("deadline");
    while Instant::now() < deadline {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

#[test]
fn a_watch_says_that_an_endpoint_came_and_that_it_went_and_is_quiet_between() {
    let reader = BusNode::new(&unique_name("watch_reader")).expect("reader");
    let peer = BusNode::new(&unique_name("watch_peer")).expect("peer");
    let watch = reader.graph_watch().expect("watch");
    let topic = format!("/{}/odom", unique_name("watch"));
    assert!(!watch.changed().expect("read"), "nothing happened yet");

    let publisher = peer.publish_raw(&topic, "t/T", V1, "").expect("publisher");
    assert!(within(2, || watch.changed().expect("read")), "a publisher came");
    assert!(!watch.changed().expect("read"), "the notification is taken once");

    drop(publisher);
    assert!(within(2, || watch.changed().expect("read")), "a publisher went");
}

#[test]
fn a_stream_is_read_by_its_hash_alone_and_no_other_stream_is() {
    let reader = BusNode::new(&unique_name("stream_reader")).expect("reader");
    let peer = BusNode::new(&unique_name("stream_peer")).expect("peer");
    let topic = format!("/{}/odom", unique_name("stream"));
    // No definition: the stream's record is empty, so no MD5 can be read back from it.
    let mut v1 = peer.publish_raw(&topic, "t/T", V1, "").expect("v1");
    let mut v2 = peer.publish_raw(&topic, "t/T", V2, "").expect("v2");
    let hash_1 = schema_hash_from_md5(V1).expect("hash");

    let mut reading = reader
        .subscribe_raw_stream(&topic, hash_1, SubscriberOptions::default().keeping(8))
        .expect("the stream of v1");
    assert!(v1.publish(&[1, 0, 0, 0]));
    assert!(v2.publish(&[2, 0, 0, 0, 3, 0, 0, 0]));
    assert!(v1.publish(&[4, 0, 0, 0]));

    let mut seen = Vec::new();
    within(2, || {
        while let Some(sample) = reading.try_receive().expect("receive") {
            assert_eq!(sample.identity().expect("identity").schema_hash, hash_1);
            seen.push(sample.bytes().expect("bytes").to_vec());
        }
        seen.len() >= 2
    });
    assert_eq!(seen, vec![vec![1, 0, 0, 0], vec![4, 0, 0, 0]], "the frames of v1, in order");

    let absent = reader.subscribe_raw_stream(&topic, 7, SubscriberOptions::default());
    assert!(absent.is_err(), "a stream nobody opened is refused");
}
