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
