//! How soon a node's housekeeping makes the other streams of its topics known: a round
//! that a pass leaves unfinished brings the run back at once, so an idle node is not held
//! to the one-second backstop for each part of a round.

#![allow(clippy::expect_used, clippy::print_stdout)]

use std::time::{Duration, Instant};

use hypha_node::sdk::{BusNode, RawSubscriber, SubscriberOptions, WaitSet};
use hypha_testkit::unique_name;

const V1: (&str, &str, &str) = ("rv/Reading", "b3087778e93fcd34cc8d65bc54e850d1", "int32 value\n");
const V2: (&str, &str, &str) = (
    "rv/Reading",
    "f79835def461350458ead76f07016dd7",
    "int32 value\nstring label\nfloat64 weight\n",
);

/// `topics` topics, each read by one subscriber of version 1 in a node with nothing else to
/// do and published in version 2 by another node: how long until every subscriber shows
/// the other version, or `None` after `limit`.
fn time_to_fresh(topics: usize, limit: Duration) -> Option<Duration> {
    let bus = BusNode::new(&unique_name("refresh_idle")).expect("node");
    let other = BusNode::new(&unique_name("refresh_idle_pub")).expect("node");
    let names: Vec<String> =
        (0..topics).map(|index| format!("/{}_{index}", unique_name("refresh_idle"))).collect();
    let publish = |kind: (&str, &str, &str)| -> Vec<_> {
        names
            .iter()
            .map(|name| other.publish_raw(name, kind.0, kind.1, kind.2).expect("publisher"))
            .collect()
    };
    let _v2 = publish(V2);
    // Version 1's stream must exist for a subscriber that declares no policy.
    let _v1 = publish(V1);
    let subscribers: Vec<RawSubscriber> = names
        .iter()
        .map(|name| {
            let options = SubscriberOptions::default().keeping(4);
            bus.subscribe_raw_with_schema(name, V1.0, V1.1, options).expect("v1 subscriber")
        })
        .collect();
    let mut run = WaitSet::new(&bus).expect("the node's WaitSet");
    let start = Instant::now();
    std::thread::scope(|scope| {
        scope.spawn(move || {
            let _ = run.run_for(limit);
        });
        loop {
            if subscribers.iter().all(|subscriber| subscriber.other_schemas() == Some(1)) {
                return Some(start.elapsed());
            }
            if start.elapsed() > limit {
                return None;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    })
}

#[test]
fn an_idle_node_makes_every_watched_topic_known_within_a_second_or_two() {
    for topics in [1_usize, 4, 10, 30] {
        let fresh = time_to_fresh(topics, Duration::from_secs(3));
        println!("{topics} watched topics: every one known after {fresh:?}");
        assert!(fresh.is_some_and(|fresh| fresh < Duration::from_secs(3)), "{topics}: {fresh:?}");
    }
}
