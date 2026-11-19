//! The behaviour every transport must show, checked against iceoryx2.
#![allow(clippy::expect_used)]

mod support;
use support::sealed_send;

use std::time::{SystemTime, UNIX_EPOCH};

use hypha_core::port::{PortError, TopicSpec};
use hypha_iceoryx2::{Iox2Publisher, Iox2Transport};

fn session(label: &str) -> Iox2Transport {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let name = format!("conformance_{label}_{}_{nanos}", std::process::id());
    Iox2Transport::with_name(&name).expect("transport: no iceoryx2 segment on this machine")
}

fn send_on(publisher: &mut Iox2Publisher, payload: &[u8]) {
    let mut loan = publisher.loan(payload.len()).expect("loan");
    loan.as_mut_bytes().copy_from_slice(payload);
    loan.send_frame(sealed_send::metadata(), 0).expect("send");
}

#[test]
fn a_published_sample_reads_back_with_the_same_bytes() {
    let transport = session("round_trip");
    let spec = TopicSpec::new("/conformance/round_trip").with_type("hypha_test/Data");
    let mut publisher = transport.publish(&spec).expect("publish");
    let subscriber = transport.subscribe(&spec).expect("subscribe");

    send_on(&mut publisher, b"hello");

    let sample = subscriber.try_receive().expect("receive").expect("a sample is pending");
    assert_eq!(sample.payload(), b"hello", "the bytes read back must be the bytes written");
}

#[test]
fn every_byte_value_survives_the_round_trip() {
    let mut payload = [0u8; 256];
    for (index, slot) in payload.iter_mut().enumerate() {
        *slot = u8::try_from(index).unwrap_or(0);
    }

    let transport = session("bytes");
    let spec = TopicSpec::new("/conformance/bytes").with_type("hypha_test/Data");
    let mut publisher = transport.publish(&spec).expect("publish");
    let subscriber = transport.subscribe(&spec).expect("subscribe");
    send_on(&mut publisher, &payload);

    let sample = subscriber.try_receive().expect("receive").expect("a sample is pending");
    assert_eq!(sample.payload(), &payload[..], "a transport must not alter any byte value");
}

#[test]
fn a_subscriber_with_nothing_pending_answers_none() {
    let transport = session("empty");
    let _publisher = transport
        .publish(&TopicSpec::new("/conformance/empty").with_type("hypha_test/Data"))
        .expect("publish");
    let subscriber = transport
        .subscribe(&TopicSpec::new("/conformance/empty").with_type("hypha_test/Data"))
        .expect("subscribe");

    let pending = subscriber.try_receive().expect("receive");
    assert!(pending.is_none(), "a subscriber with nothing pending must answer None");
}

#[test]
fn every_subscriber_on_a_topic_receives_every_sample() {
    let transport = session("fan_out");
    let spec = TopicSpec::new("/conformance/fan_out").with_type("hypha_test/Data");
    let mut publisher = transport.publish(&spec).expect("publish");
    let first = transport.subscribe(&spec).expect("subscribe first");
    let second = transport.subscribe(&spec).expect("subscribe second");

    send_on(&mut publisher, b"both");

    let to_first = first.try_receive().expect("receive").expect("first subscriber gets it");
    let to_second = second.try_receive().expect("receive").expect("second subscriber gets it");
    assert_eq!(to_first.payload(), b"both");
    assert_eq!(to_second.payload(), b"both");
}

#[test]
fn a_late_subscriber_gets_no_history() {
    let transport = session("history");
    let spec = TopicSpec::new("/conformance/history").with_type("hypha_test/Data");
    let mut publisher = transport.publish(&spec).expect("publish");
    send_on(&mut publisher, b"early");

    let subscriber = transport.subscribe(&spec).expect("subscribe");

    let pending = subscriber.try_receive().expect("receive");
    assert!(
        pending.is_none(),
        "a subscriber must not receive a sample published before it existed"
    );
}

#[test]
fn a_loan_is_exactly_the_size_requested() {
    let transport = session("loan_size");
    let publisher = transport
        .publish(&TopicSpec::new("/conformance/loan_size").with_type("hypha_test/Data"))
        .expect("publish");

    let loan = publisher.loan(37).expect("loan");
    assert_eq!(loan.len(), 37, "a loan must be exactly the length asked for");
    assert!(!loan.is_empty());

    let empty = publisher.loan(0).expect("a loan of zero bytes");
    assert_eq!(empty.len(), 0, "a zero-byte loan must be empty");
    assert!(empty.is_empty());
}

#[test]
fn a_publisher_can_refresh_its_connections() {
    let transport = session("update_connections");
    let publisher = transport
        .publish(&TopicSpec::new("/conformance/update_connections").with_type("hypha_test/Data"))
        .expect("publish");
    publisher.update_connections().expect("update connections");
}

#[test]
fn a_dropped_loan_delivers_nothing() {
    let transport = session("dropped_loan");
    let spec = TopicSpec::new("/conformance/dropped_loan").with_type("hypha_test/Data");
    let publisher = transport.publish(&spec).expect("publish");
    let subscriber = transport.subscribe(&spec).expect("subscribe");

    let loan = publisher.loan(4).expect("loan");
    drop(loan);

    let pending = subscriber.try_receive().expect("receive");
    assert!(pending.is_none(), "dropping a loan must publish nothing");
}

#[test]
fn a_queue_of_depth_one_keeps_the_newest_sample() {
    let transport = session("drop_oldest");
    let spec = TopicSpec::new("/conformance/drop_oldest").with_type("hypha_test/Data").keeping(1);
    let mut publisher = transport.publish(&spec).expect("publish");
    let subscriber = transport.subscribe(&spec).expect("subscribe");

    send_on(&mut publisher, b"1");
    send_on(&mut publisher, b"2");

    let sample = subscriber.try_receive().expect("receive").expect("a sample is pending");
    assert_eq!(sample.payload(), b"2", "drop-oldest must keep the newest sample");
    drop(sample);
}

#[test]
fn an_empty_topic_is_refused_by_both_endpoint_constructors() {
    let transport = session("empty_topic");
    let spec = TopicSpec::new("").with_type("hypha_test/Data");
    assert_eq!(transport.publish(&spec).err(), Some(PortError::InvalidTopic));
    assert_eq!(transport.subscribe(&spec).err(), Some(PortError::InvalidTopic));
}
