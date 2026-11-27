//! Topic name overloading (#209): a topic name carries one stream for each schema, and
//! each stream is a service of its own, with its own policy, queues and publishers.

#![allow(clippy::expect_used)]

mod support;
use support::sealed_send;

use hypha_core::port::{PortError, QosPolicy, QosRequest, TopicSpec};
use hypha_iceoryx2::{Iox2Transport, PublishedSchema};
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

#[test]
fn each_schema_of_a_name_is_its_own_stream() {
    let transport = Iox2Transport::with_name(&unique_name("overload_streams")).expect("transport");
    let topic = format!("/{}", unique_name("overload_streams"));
    let first_out = transport.publish(&first(&topic)).expect("first publisher");
    let second_out = transport.publish(&second(&topic)).expect("second publisher");
    let first_in = transport.subscribe(&first(&topic)).expect("first subscriber");
    let second_in = transport.subscribe(&second(&topic)).expect("second subscriber");

    send(&first_out, 1);
    send(&second_out, 2);
    send(&first_out, 3);

    assert_eq!(received(&first_in), [1, 3], "no frame of the other schema reaches a reader");
    assert_eq!(received(&second_in), [2]);
    assert_eq!(first_in.publisher_count().expect("count"), 1, "its own stream's publisher");
}

#[test]
fn a_reader_of_every_schema_reads_each_stream_that_exists() {
    let transport = Iox2Transport::with_name(&unique_name("overload_every")).expect("transport");
    let topic = format!("/{}", unique_name("overload_every"));
    let first_out = transport.publish(&first(&topic)).expect("first publisher");
    let second_out = transport.publish(&second(&topic)).expect("second publisher");
    let every = TopicSpec::new(&topic).keeping(4).reading_every_schema();
    let reader = transport.subscribe(&every).expect("reader of every schema");

    send(&first_out, 1);
    send(&second_out, 2);

    let mut bytes = received(&reader);
    bytes.sort_unstable();
    assert_eq!(bytes, [1, 2]);
    let listeners = reader.open_listeners().expect("listeners");
    assert_eq!(listeners.len(), 2, "a reader of every schema listens to each stream");
}

#[test]
fn a_reader_of_every_schema_of_a_topic_with_no_stream_is_not_available() {
    let transport = Iox2Transport::with_name(&unique_name("overload_none")).expect("transport");
    let topic = format!("/{}", unique_name("overload_none"));
    let every = TopicSpec::new(&topic).reading_every_schema();

    assert!(matches!(transport.subscribe(&every), Err(PortError::NotAvailable)));
}

#[test]
fn a_wildcard_with_a_schema_reads_only_that_schema() {
    let transport = Iox2Transport::with_name(&unique_name("overload_pattern")).expect("transport");
    let prefix = format!("/{}", unique_name("overload_pattern"));
    let topic = format!("{prefix}/joint");
    let first_out = transport.publish(&first(&topic)).expect("first publisher");
    let second_out = transport.publish(&second(&topic)).expect("second publisher");
    let pattern = first(&format!("{prefix}/*"));
    let reader = transport.subscribe(&pattern).expect("wildcard of the first schema");

    send(&second_out, 2);
    send(&first_out, 1);

    assert_eq!(received(&reader), [1]);
}

#[test]
fn the_listing_has_one_entry_for_each_stream() {
    let transport = Iox2Transport::with_name(&unique_name("overload_list")).expect("transport");
    let observer = Iox2Transport::with_name(&unique_name("overload_observer")).expect("observer");
    let topic = format!("/{}", unique_name("overload_list"));
    let _first_out = transport.publish(&first(&topic)).expect("first publisher");
    let _second_out = transport.publish(&second(&topic)).expect("second publisher");

    let mut hashes: Vec<u64> = observer
        .list_service_ports()
        .expect("listing")
        .into_iter()
        .filter(|service| service.name == topic && service.publishers == 1)
        .map(|service| service.schema_hash)
        .collect();
    hashes.sort_unstable();
    let mut expected = [
        hypha_wire::schema_hash_from_md5(FIRST_MD5).expect("hash"),
        hypha_wire::schema_hash_from_md5(SECOND_MD5).expect("hash"),
    ];
    expected.sort_unstable();
    assert_eq!(hashes, expected);
}

#[test]
fn two_schemas_of_a_name_keep_their_own_policies() {
    let transport = Iox2Transport::with_name(&unique_name("overload_qos")).expect("transport");
    let topic = format!("/{}", unique_name("overload_qos"));
    let small = QosRequest::Declared(QosPolicy::new("small", 1, 2, 3));
    let _first_out = transport.publish(&first(&topic).with_qos(small)).expect("first publisher");
    let _second_out =
        transport.publish(&second(&topic).with_qos(declared())).expect("second, on the default");

    let refused = transport.publish(&first(&topic).with_qos(declared()));
    assert!(matches!(refused, Err(PortError::QosMismatch)), "a stream keeps its own policy");
}

#[test]
fn a_subscriber_learns_which_other_schemas_of_its_topic_are_published() {
    let transport = Iox2Transport::with_name(&unique_name("overload_beside")).expect("transport");
    let topic = format!("/{}", unique_name("overload_beside"));
    let reader = transport.subscribe(&first(&topic).with_qos(declared())).expect("reader");
    assert!(!reader.has_publisher_ports());
    assert_eq!(reader.other_published_schemas().expect("schemas"), []);

    let second_out = transport.publish(&second(&topic)).expect("second publisher");
    assert!(!reader.has_publisher_ports(), "the other stream's publisher is not its own");
    assert_eq!(
        reader.other_published_schemas().expect("schemas"),
        [PublishedSchema {
            schema_hash: hypha_wire::schema_hash_from_md5(SECOND_MD5).expect("hash"),
            data_type: "overload_test/Kind".to_owned(),
            md5: SECOND_MD5.to_owned()
        }]
    );

    let first_out = transport.publish(&first(&topic).with_qos(declared())).expect("first");
    assert!(reader.has_publisher_ports());
    drop(second_out);
    assert_eq!(reader.other_published_schemas().expect("schemas"), []);
    drop(first_out);
}

fn declared() -> QosRequest {
    QosRequest::Declared(hypha_iceoryx2::qos::default_policy())
}
