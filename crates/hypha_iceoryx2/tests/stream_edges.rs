//! The stream key of topic name overloading (#209) at its edges: the name limit, two type
//! names of one definition, a reader of every schema, a stream without a definition,
//! latching, and what one check of the other streams costs.
#![allow(clippy::expect_used, clippy::print_stdout, clippy::unwrap_used)]

mod support;
use support::sealed_send;

use hypha_core::port::{PortError, QosPolicy, QosRequest, TopicSpec};
use hypha_iceoryx2::Iox2Transport;
use hypha_testkit::unique_name;

const FIRST_MD5: &str = "d9d213e8c146065ec958333c617eb1a6";
const SECOND_MD5: &str = "581e4b1bda6a172aa19e1038a38e01eb";

fn first(topic: &str) -> TopicSpec {
    TopicSpec::new(topic)
        .with_type("overload_test/Kind")
        .with_md5(FIRST_MD5)
        .with_definition("uint32 value")
        .keeping(4)
}

fn second(topic: &str) -> TopicSpec {
    TopicSpec::new(topic)
        .with_type("overload_test/Kind")
        .with_md5(SECOND_MD5)
        .with_definition("int32 value\nstring label\n")
        .keeping(4)
}

fn declared() -> QosRequest {
    QosRequest::Declared(hypha_iceoryx2::qos::default_policy())
}

fn send(publisher: &hypha_iceoryx2::Iox2Publisher, byte: u8) {
    let mut loan = publisher.loan(1).expect("loan");
    if let Some(first) = loan.as_mut_bytes().get_mut(0) {
        *first = byte;
    }
    loan.send_frame(sealed_send::metadata(), 0).expect("send");
}

fn received(subscriber: &hypha_iceoryx2::Iox2Subscriber) -> Vec<u8> {
    let mut bytes = Vec::new();
    while let Some(sample) = subscriber.try_receive().expect("receive") {
        bytes.extend_from_slice(sample.payload());
    }
    bytes
}

/// The name limits hold what a stream needs: 238 bytes after the `/` for its service name
/// (the 17 bytes of `@<16 hex>` come out of iceoryx2's 255), and 126 for an endpoint with
/// an MD5, which writes its record in a directory named by the topic in hex.
#[test]
fn the_name_limits_are_those_of_the_service_and_of_the_record() {
    let transport = Iox2Transport::with_name(&unique_name("rv_long")).expect("transport");
    let name = |bytes: usize| format!("/{}", "a".repeat(bytes));
    let recorded = hypha_core::port::MAX_RECORDED_TOPIC_NAME_BYTES;
    let service = hypha_core::port::MAX_TOPIC_NAME_BYTES;
    assert!(TopicSpec::new(&name(service)).has_valid_public_name());
    assert!(!TopicSpec::new(&name(service + 1)).has_valid_public_name());
    let typed = transport.publish(&first(&name(recorded)));
    assert!(typed.is_ok(), "the longest typed name opens a stream: {:?}", typed.err());
    let refused = transport.publish(&first(&name(recorded + 1)));
    assert!(matches!(refused, Err(PortError::InvalidTopic)));
    let untyped = transport.publish(&TopicSpec::new(&name(service)));
    assert!(untyped.is_ok(), "an untyped endpoint writes no record: {:?}", untyped.err());
}

/// Two type names with one definition have one MD5 and so one stream: the policy is
/// per definition, not per type, so a per-type `QosOf<M>` collides.
#[test]
fn two_type_names_of_one_definition_share_one_stream_and_one_policy() {
    let transport = Iox2Transport::with_name(&unique_name("rv_same_md5")).expect("transport");
    let topic = format!("/{}", unique_name("rv_same_md5"));
    let small = QosRequest::Declared(QosPolicy::new("small", 1, 2, 3));
    let a = TopicSpec::new(&topic)
        .with_type("pkg/A")
        .with_md5(FIRST_MD5)
        .with_definition("uint32 value")
        .with_qos(small);
    let b = TopicSpec::new(&topic)
        .with_type("pkg/B")
        .with_md5(FIRST_MD5)
        .with_definition("uint32 value")
        .with_qos(declared());
    let _a_out = transport.publish(&a).expect("first type");
    let refused = transport.publish(&b);
    println!("second type, same definition, other policy: {:?}", refused.as_ref().err());
    assert!(matches!(refused, Err(PortError::QosMismatch)));
}

/// A reader of every schema reads the streams that exist when it is created: a version
/// that joins later is not read, and nothing says so.
#[test]
fn a_reader_of_every_schema_misses_a_version_that_joins_later() {
    let transport = Iox2Transport::with_name(&unique_name("rv_every_late")).expect("transport");
    let topic = format!("/{}", unique_name("rv_every_late"));
    let v1 = transport.publish(&first(&topic)).expect("v1");
    let every = transport
        .subscribe(&TopicSpec::new(&topic).keeping(4).reading_every_schema())
        .expect("reader of every schema");
    let v2 = transport.publish(&second(&topic)).expect("v2 joins");
    send(&v1, 1);
    send(&v2, 2);
    let bytes = received(&every);
    println!("every-schema reader created before v2 received {bytes:?}");
    assert_eq!(bytes, [1], "v2 is not read");
}

/// Latching per stream: a late reader of its own version gets the latched sample; a late
/// reader of the other version gets nothing; a late reader of every schema gets it.
#[test]
fn latching_is_per_stream() {
    let transport = Iox2Transport::with_name(&unique_name("rv_latch")).expect("transport");
    let topic = format!("/{}", unique_name("rv_latch"));
    let v1 = transport.publish(&first(&topic).latched()).expect("latched v1");
    send(&v1, 7);
    let same = transport.subscribe(&first(&topic)).expect("late v1 reader");
    let other = transport.subscribe(&second(&topic).with_qos(declared())).expect("late v2");
    let every = transport
        .subscribe(&TopicSpec::new(&topic).keeping(4).reading_every_schema())
        .expect("late reader of every schema");
    transport.replay_latched();
    let (same, other, every) = (received(&same), received(&other), received(&every));
    println!("late v1 {same:?}, late v2 {other:?}, late every {every:?}");
    assert_eq!(same, [7]);
    assert_eq!(other, Vec::<u8>::new());
    assert_eq!(every, [7]);
}
