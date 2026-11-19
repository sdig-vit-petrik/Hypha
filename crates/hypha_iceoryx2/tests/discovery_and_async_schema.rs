//! Service discovery, and the non-blocking schema primitives above it.
#![allow(clippy::expect_used)]

use std::time::Duration;

use hypha_core::port::{QosPolicy, QosRequest, SchemaSelector, TopicSpec};
use hypha_iceoryx2::Iox2Transport;
use hypha_testkit::unique_name as unique;
use hypha_wire::schema_hash_from_md5;
use hypha_wire::schema_record::parse_schema_record;

const DATA_TYPE: &str = "std_msgs/String";
const MD5: &str = "992ce8a1687cec8c8bd883ec73ca41d1";
const DEFINITION: &str = "string data\n";

fn schema_spec(topic: &str) -> TopicSpec {
    TopicSpec::new(topic).with_type(DATA_TYPE).with_md5(MD5).with_definition(DEFINITION)
}

#[test]
fn service_ports_report_the_endpoints_on_a_public_topic() {
    // The count excludes the observing node's own ports.
    let owner = Iox2Transport::with_name(&unique("ports_owner")).expect("owner transport");
    let observer = Iox2Transport::with_name(&unique("ports_observer")).expect("observer transport");
    let topic = format!("/{}", unique("ports_topic"));

    let before = observer.list_service_ports().expect("ports before");
    let listed_before = before.iter().any(|service| service.name == topic);
    assert!(!listed_before, "the topic does not exist before anything opens it");

    let _publisher =
        owner.publish(&TopicSpec::new(&topic).with_type("std_msgs/String")).expect("publisher");
    let _subscriber =
        owner.subscribe(&TopicSpec::new(&topic).with_type("std_msgs/String")).expect("subscriber");

    let after = observer.list_service_ports().expect("ports after");
    let found = after.iter().find(|service| service.name == topic).expect("the topic is listed");
    assert_eq!(found.publishers, 1, "one publisher is counted");
    assert_eq!(found.subscribers, 1, "one subscriber is counted");

    // Internal services are never offered as topics.
    let leaked: Vec<&String> = after
        .iter()
        .map(|service| &service.name)
        .filter(|name| name.starts_with("hypha/"))
        .collect();
    assert!(leaked.is_empty(), "internal services reached the public list: {leaked:?}");
}

#[test]
fn a_schema_hash_resolves_to_the_identity_that_published_it() {
    let transport = Iox2Transport::with_name(&unique("hash_node")).expect("transport");
    let topic = format!("/{}", unique("hash_topic"));
    let _publisher = transport.publish(&schema_spec(&topic)).expect("publisher");

    let hash = schema_hash_from_md5(MD5).expect("a valid MD5");
    let bytes = transport
        .schema_lookup(&topic, SchemaSelector::Hash(hash), Duration::from_millis(200))
        .expect("resolution")
        .expect("the publisher answers for its own hash");
    let resolved = parse_schema_record(&bytes).expect("schema record");
    assert_eq!(resolved.md5(), MD5);
    assert_eq!(resolved.data_type(), DATA_TYPE);
    assert_eq!(resolved.definition(), DEFINITION);
}

#[test]
fn a_hash_no_publisher_carries_is_not_available() {
    let transport = Iox2Transport::with_name(&unique("miss_node")).expect("transport");
    let topic = format!("/{}", unique("miss_topic"));
    let _publisher = transport.publish(&schema_spec(&topic)).expect("publisher");

    // The topic answers, but not for this hash: not-available, never another definition.
    let resolved = transport
        .schema_lookup(&topic, SchemaSelector::Hash(0xDEAD_BEEF), Duration::from_millis(200))
        .expect("resolution");
    assert!(resolved.is_none(), "a hash nobody publishes resolves to nothing");
}

#[test]
fn a_submitted_schema_request_is_answered_without_blocking() {
    let transport = Iox2Transport::with_name(&unique("submit_node")).expect("transport");
    let topic = format!("/{}", unique("submit_topic"));
    let _publisher = transport.publish(&schema_spec(&topic)).expect("publisher");

    let bytes = transport
        .schema_lookup(&topic, SchemaSelector::Md5(MD5.to_owned()), Duration::ZERO)
        .expect("lookup")
        .expect("the request was answered");
    let record = parse_schema_record(&bytes).expect("a valid schema record");
    assert_eq!(record.md5(), MD5);
    assert_eq!(record.data_type(), DATA_TYPE);
}

#[test]
fn a_submission_for_an_identity_nobody_serves_finds_no_server() {
    let transport = Iox2Transport::with_name(&unique("noserver_node")).expect("transport");
    let topic = format!("/{}", unique("noserver_topic"));

    // Asking must not be the reason the service starts existing.
    let resolved = transport
        .schema_lookup(&topic, SchemaSelector::Md5(MD5.to_owned()), Duration::from_millis(100))
        .expect("resolution");
    assert!(resolved.is_none(), "no server exists, so nothing answers");
}

#[test]
fn a_hash_submission_with_no_candidate_finishes_as_not_available() {
    let transport = Iox2Transport::with_name(&unique("nocand_node")).expect("transport");
    let topic = format!("/{}", unique("nocand_topic"));

    let outcome = transport
        .schema_lookup(&topic, SchemaSelector::Hash(0x1234), Duration::ZERO)
        .expect("lookup");
    assert!(outcome.is_none());
}

#[test]
fn every_identity_on_a_crowded_topic_stays_resolvable() {
    // Several identities on one topic: every one must stay resolvable.
    const IDENTITY_COUNT: usize = 12;

    let transport = Iox2Transport::with_name(&unique("crowded_node")).expect("transport");
    let topic = format!("/{}", unique("crowded_topic"));

    let mut identities: Vec<String> = Vec::new();
    let mut publishers = Vec::new();
    for index in 0..IDENTITY_COUNT {
        // The wire hash is the first eight bytes of the md5.
        let definition = format!("uint32 field_{index}");
        let md5 = hypha_msgdef::md5::md5_from_definition("std_msgs/String", &definition)
            .expect("a complete definition");
        let spec = TopicSpec::new(&topic)
            .with_qos(QosRequest::Declared(QosPolicy::new("crowded", IDENTITY_COUNT, 8, 8)))
            .with_type("std_msgs/String")
            .with_md5(&md5)
            .with_definition(&definition);
        publishers.push(transport.publish(&spec).expect("publisher"));
        identities.push(md5);
    }

    let mut unresolved: Vec<String> = Vec::new();
    for md5 in &identities {
        let hash = schema_hash_from_md5(md5).expect("a valid MD5");
        let answer = transport
            .schema_lookup(&topic, SchemaSelector::Hash(hash), Duration::from_millis(500))
            .expect("resolution");
        match answer {
            Some(bytes) => {
                let resolved = parse_schema_record(&bytes).expect("schema record");
                assert_eq!(resolved.md5(), md5, "a hash resolved to the wrong identity");
            }
            None => unresolved.push(md5.clone()),
        }
    }

    assert!(
        unresolved.is_empty(),
        "{} of {IDENTITY_COUNT} identities on the topic did not resolve: {unresolved:?}",
        unresolved.len()
    );
}
