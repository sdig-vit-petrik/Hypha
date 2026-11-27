//! Public endpoint behaviour of the transport: listing, refusal, delivery, latching.

mod support;
use support::sealed_send;

use std::time::Duration;

use hypha_core::port::{PortError, TopicSpec};
use hypha_iceoryx2::{Iox2Publisher, Iox2Transport, list_nodes};
use hypha_testkit::unique_name;

#[allow(clippy::expect_used)]
fn send(publisher: &mut Iox2Publisher, payload: &[u8]) {
    let mut loan = publisher.loan(payload.len()).expect("loan");
    loan.as_mut_bytes().copy_from_slice(payload);
    loan.send_frame(sealed_send::metadata(), 0).expect("send");
}

#[test]
#[allow(clippy::expect_used)]
fn all_active_nodes_and_services_are_listed() {
    let transport = Iox2Transport::with_name(&unique_name("coverage_registry")).expect("transport");
    let services = transport.list_service_ports().expect("services");
    assert!(services.iter().all(|service| !service.name.is_empty()));
    let nodes = list_nodes().expect("node registry");
    assert!(nodes.iter().any(|node| node.is_alive));
}

#[test]
#[allow(clippy::expect_used)]
fn list_nodes_reports_executable_for_live_transport() {
    let name = unique_name("node_exec");
    let _transport = Iox2Transport::with_name(&name).expect("transport");
    let nodes = list_nodes().expect("node registry");
    let discovered = nodes.iter().find(|node| node.name == name).expect("the node is registered");
    assert!(discovered.is_alive);
    assert!(!discovered.executable.is_empty(), "executable path must not be empty for a live node");
}

#[test]
#[allow(clippy::expect_used)]
fn invalid_topics_are_refused() {
    let transport = Iox2Transport::with_name(&unique_name("coverage_invalid")).expect("transport");
    assert!(matches!(
        transport.publish(&TopicSpec::new("").with_type("std_msgs/String")),
        Err(PortError::InvalidTopic)
    ));
    assert!(matches!(
        transport.subscribe(&TopicSpec::new("").with_type("std_msgs/String")),
        Err(PortError::InvalidTopic)
    ));

    let topic = format!("/{}", unique_name("coverage_limit"));
    let publisher =
        transport.publish(&TopicSpec::new(&topic).with_type("std_msgs/String")).expect("publisher");
    assert_eq!(publisher.topic(), topic);
    assert_eq!(publisher.subscriber_count(), Some(0));
    assert!(publisher.loan(5).is_ok());
}

#[test]
#[allow(clippy::expect_used)]
fn a_published_message_arrives_in_shared_storage() {
    let transport =
        Iox2Transport::with_name(&unique_name("coverage_round_trip")).expect("transport");
    let topic = format!("/{}", unique_name("round_trip"));
    let spec = TopicSpec::new(&topic).with_type("std_msgs/String").keeping(2);
    let mut publisher = transport.publish(&spec).expect("publisher");
    let subscriber = transport.subscribe(&spec).expect("subscriber");
    send(&mut publisher, b"payload");

    let sample = subscriber.try_receive().expect("receive").expect("sample");
    assert_eq!(sample.topic(), topic);
    assert_eq!(sample.payload(), b"payload");
    assert!(subscriber.try_receive().expect("empty receive").is_none());
}

#[test]
#[allow(clippy::expect_used)]
fn a_shrunk_loan_arrives_with_its_first_bytes_and_a_longer_size_is_refused() {
    let transport = Iox2Transport::with_name(&unique_name("loan_shrink")).expect("transport");
    let topic = format!("/{}", unique_name("shrink"));
    let spec = TopicSpec::new(&topic).with_type("std_msgs/String").keeping(2);
    let publisher = transport.publish(&spec).expect("publisher");
    let subscriber = transport.subscribe(&spec).expect("subscriber");

    let mut loan = publisher.loan(8).expect("loan");
    loan.as_mut_bytes().copy_from_slice(b"payload!");
    assert!(matches!(loan.shrink(9), Err(PortError::TooLarge { requested: 9, limit: 8 })));
    assert_eq!(loan.len(), 8);
    loan.shrink(4).expect("shrink");
    assert_eq!(loan.as_bytes(), b"payl");
    loan.shrink(6).expect_err("a shrunk loan does not grow back");
    loan.send_frame(sealed_send::metadata(), 0).expect("send");

    let sample = subscriber.try_receive().expect("receive").expect("sample");
    assert_eq!(sample.payload(), b"payl");
    hypha_wire::frame::verify(sample.topic_code(), &sample.header(), sample.payload())
        .expect("the seal covers the shrunk payload");
}

#[test]
#[allow(clippy::expect_used)]
fn a_wildcard_subscriber_discovers_matching_topics() {
    let transport = Iox2Transport::with_name(&unique_name("coverage_wildcard")).expect("transport");
    let prefix = format!("/{}", unique_name("wildcard"));
    let first_topic = format!("{prefix}/first");
    let second_topic = format!("{prefix}/second");
    let mut first = transport
        .publish(&TopicSpec::new(&first_topic).with_type("std_msgs/String"))
        .expect("first publisher");
    let mut second = transport
        .publish(&TopicSpec::new(&second_topic).with_type("std_msgs/String"))
        .expect("second publisher");
    let pattern = format!("{prefix}/*");
    let subscriber = transport
        .subscribe(&TopicSpec::new(&pattern).with_type("std_msgs/String").keeping(2))
        .expect("wildcard subscriber");
    assert!(subscriber.try_receive().expect("discover services").is_none());
    send(&mut first, b"one");
    send(&mut second, b"two");

    let first_sample = subscriber.try_receive().expect("receive").expect("first sample");
    let second_sample = subscriber.try_receive().expect("receive").expect("second sample");
    assert_ne!(first_sample.topic(), second_sample.topic());
    assert!(subscriber.try_receive().expect("empty receive").is_none());
}

#[test]
#[allow(clippy::expect_used)]
fn a_latched_message_is_replayed_to_a_late_subscriber() {
    let transport = Iox2Transport::with_name(&unique_name("coverage_schema")).expect("transport");
    let topic = format!("/{}", unique_name("schema"));
    let spec = TopicSpec::new(&topic)
        .with_type("std_msgs/String")
        .with_md5("d9d213e8c146065ec958333c617eb1a6")
        .with_definition("uint32 value")
        .keeping(2)
        .latched();
    let mut publisher = transport.publish(&spec).expect("publisher");
    send(&mut publisher, b"data");
    std::thread::sleep(Duration::from_millis(350));
    let late_subscriber = transport.subscribe(&spec).expect("subscriber");
    transport.replay_latched();
    let sample = late_subscriber.try_receive().expect("receive").expect("latched sample");
    assert_eq!(sample.payload(), b"data");
}

/// review/255: a loan shrunk to nothing arrives empty and verifies, and a slot shrunk at
/// every send goes back to the pool whole: a full-size loan is granted on every round.
#[test]
#[allow(clippy::expect_used)]
fn a_loan_shrunk_to_nothing_arrives_empty_and_every_slot_returns_whole() {
    let transport = Iox2Transport::with_name(&unique_name("loan_shrink_pool")).expect("transport");
    let topic = format!("/{}", unique_name("shrink_pool"));
    let spec = TopicSpec::new(&topic).with_type("std_msgs/String").keeping(2);
    let publisher = transport.publish(&spec).expect("publisher");
    let subscriber = transport.subscribe(&spec).expect("subscriber");
    for round in 0..64_usize {
        let mut loan = publisher.loan(4096).expect("a full-size loan on every round");
        assert!(loan.as_bytes().iter().all(|byte| *byte == 0), "round {round}: zero-filled");
        loan.as_mut_bytes().fill(0xAB);
        let kept = round % 3;
        loan.shrink(kept).expect("shrink");
        loan.shrink(kept).expect("a second shrink to the same size");
        loan.send_frame(sealed_send::metadata(), 0).expect("send");
        let sample = subscriber.try_receive().expect("receive").expect("sample");
        assert_eq!(sample.payload().len(), kept);
        hypha_wire::frame::verify(sample.topic_code(), &sample.header(), sample.payload())
            .expect("the seal covers the shrunk payload");
    }
}

/// A growable topic has no byte limit of its own, so the frame's `payload_len` (a `u32`)
/// is the limit: a longer loan is `TooLarge` before anything is allocated, never a frame
/// whose `payload_len` its receivers would refuse.
#[test]
#[allow(clippy::expect_used)]
fn a_loan_past_what_payload_len_can_say_is_too_large_on_a_growable_topic() {
    use hypha_wire::frame::MAX_PAYLOAD_LEN;
    let transport = Iox2Transport::with_name(&unique_name("loan_max")).expect("transport");
    let topic = format!("/{}", unique_name("loan_max"));
    let spec = TopicSpec::new(&topic).with_type("std_msgs/String").keeping(2);
    let publisher = transport.publish(&spec).expect("publisher");
    let refused = publisher.loan(MAX_PAYLOAD_LEN + 1).err();
    assert!(
        matches!(refused, Some(PortError::TooLarge { requested, limit })
            if requested == MAX_PAYLOAD_LEN + 1 && limit == MAX_PAYLOAD_LEN),
        "{refused:?}"
    );
}

/// A shrunk loan dropped unsent returns its slot whole: far more rounds than the pool has
/// slots or the publisher has loans, each followed by a full-size loan.
#[test]
#[allow(clippy::expect_used)]
fn a_shrunk_loan_dropped_unsent_returns_its_slot() {
    let transport = Iox2Transport::with_name(&unique_name("loan_drop")).expect("transport");
    let topic = format!("/{}", unique_name("loan_drop"));
    let spec = TopicSpec::new(&topic).with_type("std_msgs/String").keeping(2);
    let publisher = transport.publish(&spec).expect("publisher");
    let _subscriber = transport.subscribe(&spec).expect("subscriber");
    for round in 0..64_usize {
        let mut loan = publisher.loan(4096).expect("a full-size loan on every round");
        loan.shrink(round % 3).expect("shrink");
        drop(loan);
    }
    let whole = publisher.loan(4096).expect("a full-size loan after the drops");
    assert_eq!(whole.len(), 4096);
}

/// A loan shrunk at the send is, header and payload, the frame of a loan of exactly the
/// bytes it kept: with the same metadata the two headers are equal field by field, the CRC
/// and `payload_len` included, and the payloads are equal.
#[test]
#[allow(clippy::expect_used)]
fn a_shrunk_loan_sends_the_frame_of_a_loan_of_its_kept_size() {
    let transport = Iox2Transport::with_name(&unique_name("loan_shrink_same")).expect("transport");
    let topic = format!("/{}", unique_name("shrink_same"));
    let spec = TopicSpec::new(&topic).with_type("std_msgs/String").keeping(4);
    let publisher = transport.publish(&spec).expect("publisher");
    let subscriber = transport.subscribe(&spec).expect("subscriber");
    let message: Vec<u8> = (0..40_u8).collect();
    let metadata = sealed_send::metadata();

    let mut shrunk = publisher.loan(64).expect("a loan with room");
    shrunk.as_mut_bytes().get_mut(..40).expect("40 bytes").copy_from_slice(&message);
    shrunk.shrink(40).expect("shrink");
    shrunk.send_frame(metadata, 0).expect("send the shrunk loan");
    let mut exact = publisher.loan(40).expect("a loan of the kept size");
    exact.as_mut_bytes().copy_from_slice(&message);
    exact.send_frame(metadata, 0).expect("send the exact loan");

    let first = subscriber.try_receive().expect("receive").expect("the shrunk frame");
    let second = subscriber.try_receive().expect("receive").expect("the exact frame");
    assert_eq!(first.header(), second.header());
    assert_eq!(first.header().payload_len, 40);
    assert_eq!(first.payload(), message.as_slice());
    assert_eq!(second.payload(), message.as_slice());
}
