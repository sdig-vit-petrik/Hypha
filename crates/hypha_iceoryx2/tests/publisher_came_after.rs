//! A subscriber tells a publisher port that came after it from one that may have sent before
//! it (#247): the first owed it every frame.

#![allow(clippy::expect_used, reason = "a failed setup or receive is a test failure")]

use hypha_core::port::TopicSpec;
use hypha_iceoryx2::{Iox2Publisher, Iox2Subscriber, Iox2Transport};
use hypha_testkit::unique_name;
use hypha_wire::frame::FrameMetadata;

/// Send one sample whose payload is `value`, stamped now.
fn send(publisher: &Iox2Publisher, value: u8) {
    let now = u64::try_from(hypha_time::steady_now().nanos()).expect("a steady time");
    let mut loan = publisher.loan(1).expect("loan");
    *loan.as_mut_bytes().first_mut().expect("the payload byte") = value;
    let metadata = FrameMetadata {
        schema_hash: 1,
        publisher_id: u64::from(value),
        sequence: 1,
        transport_sent_nanos: now,
        framework_sent_nanos: now,
    };
    loan.send_frame(metadata, 0).expect("send");
}

/// The payload byte of each pending sample, and whether its port came after `subscriber`.
fn drain(subscriber: &Iox2Subscriber) -> Vec<(u8, bool)> {
    let mut seen = Vec::new();
    while let Some(sample) = subscriber.try_receive().expect("receive") {
        let value = *sample.payload().first().expect("the payload byte");
        seen.push((value, subscriber.publisher_came_after(&sample)));
    }
    seen.sort_unstable();
    seen
}

#[test]
fn a_port_opened_after_the_subscriber_came_after_it_and_one_from_before_did_not() {
    let transport = Iox2Transport::with_name(&unique_name("came_after")).expect("transport");
    let spec = TopicSpec::new(&format!("/{}", unique_name("came_after")))
        .with_type("std_msgs/String")
        .keeping(4);
    let before = transport.publish(&spec).expect("publisher");
    let subscriber = transport.subscribe(&spec).expect("subscriber");
    let after = transport.publish(&spec).expect("publisher");
    send(&before, 1);
    send(&after, 2);
    assert_eq!(drain(&subscriber), [(1, false), (2, true)]);
}

#[test]
fn a_latched_port_opened_after_the_subscriber_came_after_it() {
    let transport = Iox2Transport::with_name(&unique_name("came_after_latch")).expect("transport");
    let spec = TopicSpec::new(&format!("/{}", unique_name("came_after_latch")))
        .with_type("std_msgs/String")
        .keeping(4);
    let _opens_the_stream = transport.publish(&spec).expect("publisher");
    let subscriber = transport.subscribe(&spec).expect("subscriber");
    let latched = transport.publish(&spec.clone().latched()).expect("latched publisher");
    send(&latched, 3);
    assert_eq!(drain(&subscriber), [(3, true)]);
}

#[test]
fn each_stream_of_a_wildcard_set_tells_its_own_ports_apart() {
    let transport = Iox2Transport::with_name(&unique_name("came_after_set")).expect("transport");
    let prefix = format!("/{}", unique_name("came_after_set"));
    let spec =
        |name: &str| TopicSpec::new(&format!("{prefix}/{name}")).with_type("std_msgs/String");
    let first_before = transport.publish(&spec("a")).expect("publisher");
    let second_before = transport.publish(&spec("b")).expect("publisher");
    let pattern = TopicSpec::new(&format!("{prefix}/*")).with_type("std_msgs/String").keeping(4);
    let subscriber = transport.subscribe(&pattern).expect("wildcard subscriber");
    let second_after = transport.publish(&spec("b")).expect("publisher");
    send(&first_before, 1);
    send(&second_before, 2);
    send(&second_after, 3);
    assert_eq!(drain(&subscriber), [(1, false), (2, false), (3, true)]);
}
