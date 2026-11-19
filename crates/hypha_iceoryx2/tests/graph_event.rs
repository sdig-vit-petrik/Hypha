//! The graph event wakes a `WaitSet` on a new subscriber, and the latched sample reaches it.

use std::time::Duration;

use hypha_core::port::{QosPolicy, QosRequest, TopicSpec};
use hypha_iceoryx2::{Iox2Publisher, Iox2Subscriber, Iox2Transport};
use hypha_testkit::unique_name as unique;
use hypha_wire::frame::FrameMetadata;

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

#[test]
#[allow(clippy::expect_used)]
fn creating_a_subscriber_fires_the_graph_event() {
    let transport = Iox2Transport::with_name(&unique("graph_fires")).expect("transport");
    let listener = transport.graph_listener().expect("graph listener");
    let spec = TopicSpec::new(&format!("/{}", unique("graph_fires_topic")))
        .with_type("hypha_test/Data")
        .with_qos(QosRequest::Declared(QosPolicy::new("default", 8, 16, 32)));

    let _subscriber = transport.subscribe(&spec).expect("subscribe");

    let fired = listener.timed_wait(|_| {}, Duration::from_secs(1)).expect("wait");
    assert!(fired > 0, "creating a subscriber must notify the graph event");
}

#[test]
#[allow(clippy::expect_used)]
fn a_late_subscriber_receives_the_latched_sample_once_the_graph_event_fires() {
    let transport = Iox2Transport::with_name(&unique("graph_latch")).expect("transport");
    let listener = transport.graph_listener().expect("graph listener");
    let spec = TopicSpec::new(&format!("/{}", unique("graph_latch_topic")))
        .with_type("hypha_test/Data")
        .latched();
    let mut publisher = transport.publish(&spec).expect("publish");
    send(&mut publisher, 1, 1);
    send(&mut publisher, 2, 2);

    let mut subscriber = transport.subscribe(&spec).expect("subscribe");
    listener.timed_wait(|_| {}, Duration::from_secs(1)).expect("the graph event");
    transport.replay_latched();

    assert_eq!(drain(&mut subscriber), vec![2]);
}
