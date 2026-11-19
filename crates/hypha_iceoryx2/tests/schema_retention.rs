//! Per-topic schema resolution over the RPC transport.

#![allow(clippy::expect_used)]

mod support;
use support::sealed_send;

use std::time::Duration;

use hypha_core::port::{SchemaSelector, TopicSpec};
use hypha_iceoryx2::{Iox2Transport, service_name_for};
use hypha_testkit::unique_name;
use hypha_wire::schema_record::parse_schema_record;

const CALL_TIMEOUT: Duration = Duration::from_millis(200);

/// Ask for one exact `(topic, md5)` record and wait a bounded time for it.
fn lookup(
    transport: &Iox2Transport,
    topic: &str,
    md5: &str,
) -> Result<Option<Vec<u8>>, hypha_core::port::PortError> {
    transport.schema_lookup(topic, SchemaSelector::Md5(md5.to_owned()), CALL_TIMEOUT)
}

#[test]
fn data_service_name_is_the_topic_without_its_leading_slash() {
    let topic = "/camera/image";
    assert_eq!(service_name_for(topic), "camera/image");
    assert_eq!(service_name_for("camera/image"), "camera/image");
}

#[test]
fn schema_rpc_client_resolves_definition_from_a_live_publisher() {
    let transport = Iox2Transport::with_name(&unique_name("schema_rpc")).expect("transport");
    let topic = format!("/{}", unique_name("schema_topic"));
    let spec = TopicSpec::new(&topic)
        .with_type("std_msgs/String")
        .with_md5("d9d213e8c146065ec958333c617eb1a6")
        .with_definition("uint32 value");
    let publisher = transport.publish(&spec).expect("publisher");

    let response = lookup(&transport, &topic, &spec.md5)
        .expect("schema call")
        .expect("schema server answered");
    let record = parse_schema_record(&response).expect("schema record");
    assert_eq!(record.data_type(), "std_msgs/String");
    assert_eq!(record.md5(), "d9d213e8c146065ec958333c617eb1a6");
    assert_eq!(record.definition(), "uint32 value");

    // Repeated calls are answered the same way; the server is not consumed.
    let second = lookup(&transport, &topic, &spec.md5)
        .expect("second schema call")
        .expect("schema server answered again");
    assert_eq!(second, response);

    drop(publisher);
}

#[test]
fn a_definition_near_the_schema_payload_cap_round_trips() {
    let transport = Iox2Transport::with_name(&unique_name("schema_large")).expect("transport");
    let topic = format!("/{}", unique_name("schema_large_topic"));
    // Larger than the generic RPC feature's default payload cap.
    let large_definition = format!("uint32 value\n#{}", "x".repeat(60_000));
    let spec = TopicSpec::new(&topic)
        .with_type("std_msgs/String")
        .with_md5("d9d213e8c146065ec958333c617eb1a6")
        .with_definition(&large_definition);
    let publisher = transport.publish(&spec).expect("publisher");

    let response = lookup(&transport, &topic, &spec.md5)
        .expect("schema call")
        .expect("schema server answered");
    let record = parse_schema_record(&response).expect("schema record");
    assert_eq!(record.definition(), large_definition);

    drop(publisher);
}

#[test]
fn different_schema_identities_are_served_independently() {
    let transport = Iox2Transport::with_name(&unique_name("schema_versions")).expect("transport");
    let topic = format!("/{}", unique_name("versioned_topic"));
    let first_spec = TopicSpec::new(&topic)
        .with_type("std_msgs/String")
        .with_md5("2f594660f93a83fc58af2a4a8e891ec2")
        .with_definition("uint32 first");
    let second_spec = TopicSpec::new(&topic)
        .with_type("std_msgs/String")
        .with_md5("0b153b21d0dae7a10c8ed92dc1a76f11")
        .with_definition("uint32 second");
    let first_publisher = transport.publish(&first_spec).expect("first publisher");
    let second_publisher = transport.publish(&second_spec).expect("second publisher");

    let first_response = lookup(&transport, &topic, &first_spec.md5)
        .expect("first schema call")
        .expect("first schema server answered");
    let second_response = lookup(&transport, &topic, &second_spec.md5)
        .expect("second schema call")
        .expect("second schema server answered");

    assert_eq!(
        parse_schema_record(&first_response).expect("first record").definition(),
        "uint32 first"
    );
    assert_eq!(
        parse_schema_record(&second_response).expect("second record").definition(),
        "uint32 second"
    );

    drop(first_publisher);
    drop(second_publisher);
}

#[test]
fn wildcard_data_subscription_does_not_discover_schema_services() {
    let transport = Iox2Transport::with_name(&unique_name("schema_wildcard")).expect("transport");
    let prefix = format!("/{}", unique_name("wildcard_scope"));
    let topic = format!("{prefix}/topic");
    let pattern = format!("{prefix}/**");
    let spec = TopicSpec::new(&topic)
        .with_type("std_msgs/String")
        .with_md5("d9d213e8c146065ec958333c617eb1a6")
        .with_definition("uint32 value");
    let publisher = transport.publish(&spec).expect("publisher");
    let wildcard = transport
        .subscribe(&TopicSpec::new(&pattern).with_type("std_msgs/String").keeping(2))
        .expect("wildcard subscriber");
    assert!(wildcard.try_receive().expect("wildcard discovery").is_none());
    let mut loan = publisher.loan(1).expect("loan");
    if let Some(byte) = loan.as_mut_bytes().get_mut(0) {
        *byte = 7;
    }
    loan.send_frame(sealed_send::metadata(), 0).expect("send");
    let sample = wildcard.try_receive().expect("wildcard receive").expect("data sample");
    assert_eq!(sample.topic(), topic);
    assert!(wildcard.try_receive().expect("wildcard empty").is_none());
}

#[test]
fn malformed_schema_identity_is_rejected_before_endpoint_creation() {
    let transport = Iox2Transport::with_name(&unique_name("schema_validation")).expect("transport");
    let topic = format!("/{}", unique_name("validation_topic"));
    let invalid = TopicSpec::new(&topic)
        .with_type("std_msgs/String")
        .with_md5("0123456789abcdef")
        .with_definition("uint32 value");
    assert!(matches!(transport.publish(&invalid), Err(hypha_core::port::PortError::InvalidTopic)));
}

#[test]
fn schema_call_reports_unavailable_before_any_publisher_exists() {
    let transport = Iox2Transport::with_name(&unique_name("schema_missing")).expect("transport");
    let topic = format!("/{}", unique_name("missing_topic"));
    let md5 = "99999999999999999999999999999999";

    assert_eq!(lookup(&transport, &topic, md5).expect("schema call"), None);
    // Calling again must not create the service the resolver was asking about.
    assert_eq!(lookup(&transport, &topic, md5).expect("schema call"), None);
}

#[test]
fn schema_stays_resolvable_while_a_subscriber_holds_the_topic() {
    let transport = Iox2Transport::with_name(&unique_name("schema_lifetime")).expect("transport");
    let topic = format!("/{}", unique_name("lifetime_topic"));
    let spec = TopicSpec::new(&topic)
        .with_type("std_msgs/String")
        .with_md5("d9d213e8c146065ec958333c617eb1a6")
        .with_definition("uint32 value");
    let publisher = transport.publish(&spec).expect("publisher");
    // An untyped subscriber writes no record of its own: it only holds the topic.
    let subscriber = transport.subscribe(&TopicSpec::new(&topic)).expect("subscriber");

    drop(publisher);
    assert!(lookup(&transport, &topic, &spec.md5).expect("schema call after drop").is_some());

    // The last endpoint of the topic takes its records with it.
    let topic_dir = hypha_iceoryx2::schema_file_path(&topic, 0)
        .expect("schema path")
        .parent()
        .expect("topic directory")
        .to_path_buf();
    assert!(topic_dir.exists(), "the record is on disk while the topic is open");
    drop(subscriber);
    assert!(!topic_dir.exists(), "{} is left after the last endpoint closed", topic_dir.display());
    assert_eq!(lookup(&transport, &topic, &spec.md5).expect("schema call"), None);
}

#[test]
fn a_closed_topic_leaves_no_schema_record() {
    let transport = Iox2Transport::with_name(&unique_name("schema_close")).expect("transport");
    let topic = format!("/{}", unique_name("closed_topic"));
    let spec = TopicSpec::new(&topic)
        .with_type("std_msgs/String")
        .with_md5("d9d213e8c146065ec958333c617eb1a6")
        .with_definition("uint32 value");
    let path = hypha_iceoryx2::schema_file_path(&topic, 0).expect("schema path");
    let topic_dir = path.parent().expect("topic directory").to_path_buf();

    let publisher = transport.publish(&spec).expect("publisher");
    assert!(topic_dir.exists(), "the publisher wrote its record");
    drop(publisher);
    assert!(!topic_dir.exists(), "{} is left after the publisher closed", topic_dir.display());
}

#[test]
fn corrupted_schema_file_is_rejected_and_increments_invalid_records() {
    let transport = Iox2Transport::with_name(&unique_name("corrupt_schema")).expect("transport");
    let topic = format!("/{}", unique_name("corrupt_topic"));
    let spec = TopicSpec::new(&topic)
        .with_type("std_msgs/String")
        .with_md5("d9d213e8c146065ec958333c617eb1a6")
        .with_definition("uint32 value");
    let _publisher = transport.publish(&spec).expect("publisher");

    let before_invalid = hypha_iceoryx2::invalid_record_count();
    let schema_hash = hypha_wire::schema_hash_from_md5(&spec.md5).expect("a valid MD5");
    let path = hypha_iceoryx2::schema_file_path(&topic, schema_hash).expect("schema path");
    std::fs::write(&path, b"corrupted schema bytes").expect("write corrupt file");

    let result = lookup(&transport, &topic, &spec.md5).expect("schema lookup");
    assert_eq!(result, None);
    assert_eq!(hypha_iceoryx2::invalid_record_count(), before_invalid + 1);
}
