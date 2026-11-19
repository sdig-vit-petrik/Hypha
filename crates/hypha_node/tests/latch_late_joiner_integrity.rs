//! A late subscriber of a latched topic receives the last latched frame once.
#![allow(clippy::expect_used)]

use std::time::Duration;

use hypha_node::HyphaStatus;
use hypha_node::sdk::{BusNode, PublisherOptions, RawSubscriber, SubscriberOptions, WaitSet};
use hypha_testkit::unique_name;

/// The frames the publisher sends before the subscriber exists.
const LATCHED_FRAMES: u8 = 4;

/// How long the node runs, in chunks, with the subscriber draining between them.
const CHUNK: Duration = Duration::from_millis(100);
const CHUNK_COUNT: usize = 10;

/// What one drain of the subscriber queue reported.
#[derive(Debug, Default, PartialEq, Eq)]
struct Drain {
    accepted: usize,
    refused: usize,
}

/// Take every queued frame. A refused frame is reported by its status.
fn drain(subscriber: &mut RawSubscriber) -> Drain {
    let mut result = Drain::default();
    loop {
        let received = subscriber.try_receive();
        let status = received.as_ref().err().map(|error| error.status());
        if status == Some(HyphaStatus::SequenceGap as i32) {
            result.refused = result.refused.saturating_add(1);
            continue;
        }
        assert_eq!(status, None, "the receive is refused");
        let taken = received.ok().flatten();
        if taken.is_none() {
            return result;
        }
        result.accepted = result.accepted.saturating_add(1);
    }
}

fn integrity(subscriber: &RawSubscriber, name: &str) -> u64 {
    let list = subscriber.metrics().expect("metrics");
    let found = list.into_iter().find(|(metric, _)| metric == name);
    found.map(|(_, value)| value).expect("an integrity metric")
}

#[test]
fn a_late_subscriber_of_a_latched_topic_gets_the_last_latched_frame_once() {
    let bus = BusNode::new(&unique_name("latch_join")).expect("node");
    let topic = format!("/hypha_node/latch_join/{}", unique_name("topic"));
    let options = PublisherOptions::default().latched();
    let mut publisher = bus
        .publish_raw_configured(
            &topic,
            "std_msgs/Bytes",
            "d9d213e8c146065ec958333c617eb1a6",
            "",
            options,
        )
        .expect("publisher");
    for index in 0..LATCHED_FRAMES {
        assert!(publisher.publish(&[index, index, index]));
    }

    let options = SubscriberOptions::default().keeping(4);
    let mut subscriber = bus.subscribe_raw_with_options(&topic, options).expect("subscriber");
    let mut waitset = WaitSet::new(&bus).expect("waitset");

    let mut total = Drain::default();
    for _ in 0..CHUNK_COUNT {
        let report = waitset.run_for(CHUNK);
        assert!(report.ok(), "the run fails: {:?}", report.err());
        let chunk = drain(&mut subscriber);
        total.accepted = total.accepted.saturating_add(chunk.accepted);
        total.refused = total.refused.saturating_add(chunk.refused);
    }

    assert_eq!(total.refused, 0, "a repeated latched frame was refused");
    assert_eq!(total.accepted, 1, "the latch keeps one sample, and it arrives once");
    assert_eq!(
        integrity(&subscriber, "integrity.sequence_gap"),
        0,
        "a repeated latched frame is a rejection"
    );
    assert_eq!(integrity(&subscriber, "integrity.invalid_frame"), 0);
    assert_eq!(integrity(&subscriber, "integrity.corrupt_payload"), 0);
    assert_eq!(integrity(&subscriber, "integrity.schema_mismatch"), 0);
    assert_eq!(integrity(&subscriber, "integrity.invalid_timestamp"), 0);
}
