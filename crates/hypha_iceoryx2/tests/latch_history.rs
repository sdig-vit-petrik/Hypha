//! A latched publisher hands its last sample to a late subscriber, once; an unlatched one nothing.

use std::thread::sleep;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hypha_core::port::{QosPolicy, QosRequest, TopicSpec};
use hypha_iceoryx2::{Iox2Publisher, Iox2Subscriber, Iox2Transport};
use hypha_wire::frame::FrameMetadata;

fn unique_topic(label: &str) -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |time| time.as_nanos());
    format!("/{label}_{}_{nanos}", std::process::id())
}

/// Send one sample whose payload is `value`, sent now.
#[allow(clippy::expect_used)]
fn send(publisher: &mut Iox2Publisher, sequence: u64, value: u8) {
    let now = u64::try_from(hypha_time::steady_now().nanos()).expect("a steady time");
    let mut loan = publisher.loan(1).expect("loan");
    *loan.as_mut_bytes().first_mut().expect("the payload byte") = value;
    loan.send_frame(
        FrameMetadata {
            schema_hash: 1,
            publisher_id: 7,
            sequence,
            transport_sent_nanos: now,
            framework_sent_nanos: now,
        },
        0,
    )
    .expect("send");
}

/// The payload byte of every pending sample.
#[allow(clippy::expect_used)]
fn drain(subscriber: &mut Iox2Subscriber) -> Vec<u8> {
    let mut values = Vec::new();
    while let Some(sample) = subscriber.try_receive().expect("receive") {
        values.push(*sample.payload().first().expect("the payload byte"));
    }
    values
}

#[allow(clippy::expect_used)]
fn transport(label: &str) -> Iox2Transport {
    Iox2Transport::with_name(&unique_topic(label).replace('/', "")).expect("transport")
}

#[test]
#[allow(clippy::expect_used)]
fn a_latched_sample_carries_the_flag_in_its_verified_header() {
    let transport = transport("latch_flag");
    let latched =
        TopicSpec::new(&unique_topic("latch_flag")).with_type("hypha_test/Data").latched();
    let plain = TopicSpec::new(&unique_topic("latch_flag_plain")).with_type("hypha_test/Data");
    let mut latched_publisher = transport.publish(&latched).expect("latched publisher");
    let mut plain_publisher = transport.publish(&plain).expect("plain publisher");
    send(&mut latched_publisher, 1, 1);
    send(&mut plain_publisher, 1, 2);

    let latched_sub = transport.subscribe(&latched).expect("latched subscriber");
    let plain_sub = transport.subscribe(&plain).expect("plain subscriber");
    transport.replay_latched();
    let sample = latched_sub.try_receive().expect("receive").expect("the latched sample");
    assert!(verified_latch(&sample), "the latched sample carries the flag");

    send(&mut plain_publisher, 2, 3);
    let sample = plain_sub.try_receive().expect("receive").expect("a live plain sample");
    assert!(!verified_latch(&sample), "a plain sample carries no latch flag");
}

/// Verify the header of `sample` against its payload and return its latch flag.
#[allow(clippy::expect_used)]
fn verified_latch(sample: &hypha_iceoryx2::Iox2Sample) -> bool {
    let header = sample.header();
    hypha_wire::frame::verify(sample.topic_code(), &header, sample.payload())
        .expect("the header verifies")
        .latched()
}

#[test]
#[allow(clippy::expect_used)]
fn a_late_subscriber_of_a_latched_topic_receives_the_last_sample_once() {
    let transport = transport("latch_late");
    let spec = TopicSpec::new(&unique_topic("latch_late")).with_type("hypha_test/Data").latched();
    let mut publisher = transport.publish(&spec).expect("publish");
    send(&mut publisher, 1, 1);
    send(&mut publisher, 2, 2);

    let mut subscriber = transport.subscribe(&spec).expect("subscribe");
    transport.replay_latched();
    assert_eq!(drain(&mut subscriber), vec![2]);
    for _ in 0..3 {
        sleep(Duration::from_millis(50));
        transport.replay_latched();
    }
    assert!(drain(&mut subscriber).is_empty(), "a later check repeated the latched sample");
}

#[test]
#[allow(clippy::expect_used)]
fn a_subscriber_that_exists_first_receives_each_latched_sample_once() {
    let transport = transport("latch_first");
    let topic = unique_topic("latch_first");
    let spec = TopicSpec::new(&topic).with_type("hypha_test/Data").latched();
    // This subscriber exists before the publisher, so it declares the service it creates.
    let declared = TopicSpec::new(&topic)
        .with_type("hypha_test/Data")
        .latched()
        .with_qos(QosRequest::Declared(QosPolicy::new("default", 8, 16, 32)));
    let mut subscriber = transport.subscribe(&declared).expect("subscribe");
    let mut publisher = transport.publish(&spec).expect("publish");
    send(&mut publisher, 1, 1);
    transport.replay_latched();
    assert_eq!(drain(&mut subscriber), vec![1]);
}

#[test]
#[allow(clippy::expect_used)]
fn a_subscriber_that_replaces_a_leaving_one_between_checks_receives_the_latched_sample() {
    let transport = transport("latch_replace");
    let spec =
        TopicSpec::new(&unique_topic("latch_replace")).with_type("hypha_test/Data").latched();
    let mut publisher = transport.publish(&spec).expect("publish");
    send(&mut publisher, 1, 1);

    let mut first = transport.subscribe(&spec).expect("first subscriber");
    transport.replay_latched();
    assert_eq!(drain(&mut first), vec![1]);
    drop(first);

    let mut second = transport.subscribe(&spec).expect("second subscriber");
    transport.replay_latched();
    assert_eq!(drain(&mut second), vec![1]);
}

#[test]
#[allow(clippy::expect_used)]
fn a_late_subscriber_of_an_unlatched_topic_receives_nothing_old() {
    let transport = transport("latch_none");
    let spec = TopicSpec::new(&unique_topic("latch_none")).with_type("hypha_test/Data");
    let mut publisher = transport.publish(&spec).expect("publish");
    send(&mut publisher, 1, 1);
    sleep(Duration::from_millis(20));

    let mut subscriber = transport.subscribe(&spec).expect("subscribe");
    transport.replay_latched();
    assert!(drain(&mut subscriber).is_empty(), "an old sample reached a late subscriber");

    // The next send brings the old sample along as history; only the new one is kept.
    send(&mut publisher, 2, 2);
    assert_eq!(drain(&mut subscriber), vec![2]);
}

#[test]
#[allow(clippy::expect_used)]
fn an_unlatched_publisher_beside_a_latched_one_stays_unlatched() {
    let transport = transport("latch_mixed");
    let topic = unique_topic("latch_mixed");
    let latched = TopicSpec::new(&topic).with_type("hypha_test/Data").latched();
    let plain = TopicSpec::new(&topic).with_type("hypha_test/Data");
    let mut latched_publisher = transport.publish(&latched).expect("latched publisher");
    let mut plain_publisher = transport.publish(&plain).expect("plain publisher");
    send(&mut latched_publisher, 1, 1);
    send(&mut plain_publisher, 1, 2);
    sleep(Duration::from_millis(20));

    let mut subscriber = transport.subscribe(&plain).expect("subscribe");
    transport.replay_latched();
    assert_eq!(drain(&mut subscriber), vec![1], "only the latched sample is old news");
}
