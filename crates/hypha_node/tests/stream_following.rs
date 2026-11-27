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

/// A late reader of a latched stream gets its last sample, whether it subscribes by name or, as
/// every follower does, by the stream's hash. The publisher's node runs its `WaitSet`, which
/// hands the history to a reader that joins.
fn latched_sample_reaches_a_late_reader(by_hash: bool) -> bool {
    use hypha_node::sdk::{PublisherOptions, WaitSet};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let topic = format!("/{}/tf_static", unique_name("latched"));
    let hash = schema_hash_from_md5(V1).expect("hash");
    let (stop, ready) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
    let publisher = {
        let (stop, ready, topic) = (Arc::clone(&stop), Arc::clone(&ready), topic.clone());
        std::thread::spawn(move || {
            let node = BusNode::new(&unique_name("latched_pub")).expect("node");
            let mut latched = node
                .publish_raw_configured(
                    &topic,
                    "t/T",
                    V1,
                    "",
                    PublisherOptions::default().latched(),
                )
                .expect("latched publisher");
            assert!(latched.publish(&[9, 0, 0, 0]));
            let mut waitset = WaitSet::new(&node).expect("waitset");
            ready.store(true, Ordering::Release);
            while !stop.load(Ordering::Relaxed) {
                let _ = waitset.run_for(Duration::from_millis(20));
            }
        })
    };
    while !ready.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_millis(5));
    }
    std::thread::sleep(Duration::from_millis(200));
    let bus = BusNode::new(&unique_name("latched_reader")).expect("reader");
    let options = SubscriberOptions::default().keeping(0);
    let mut reader = if by_hash {
        bus.subscribe_raw_stream(&topic, hash, options).expect("by hash")
    } else {
        bus.subscribe_raw_with_options(&topic, options).expect("by name")
    };
    let got = within(3, || reader.try_receive().expect("receive").is_some());
    stop.store(true, Ordering::Relaxed);
    publisher.join().expect("publisher thread");
    got
}

#[test]
fn a_late_reader_of_a_latched_stream_gets_its_sample_by_name() {
    assert!(latched_sample_reaches_a_late_reader(false));
}

#[test]
fn a_late_reader_of_a_latched_stream_gets_its_sample_by_hash() {
    assert!(latched_sample_reaches_a_late_reader(true));
}
