//! Raw endpoint, schema and registry behavior.

use std::time::{Duration, Instant};

use hypha_node::sdk::{BusNode, PublisherOptions, SchemaError, SubscriberOptions, WaitSet};
use hypha_testkit::unique_name;

const TYPE_NAME: &str = "std_msgs/Bytes";
const MD5: &str = "f43a8e1b362b75baa741461b46adc7e0";

fn create_node(label: &str) -> BusNode {
    let result = BusNode::new(&unique_name(label));
    assert!(result.is_ok());
    #[allow(clippy::expect_used)]
    result.expect("the asserted node creation result is successful")
}

#[test]
fn raw_endpoints_publish_empty_and_nonempty_payloads() {
    let node = create_node("raw_endpoint");
    let topic = format!("/hypha/raw/{}", unique_name("payload"));
    let publisher = node.publish_raw(&topic, TYPE_NAME, MD5, "uint8[] data");
    assert!(publisher.is_ok());
    let Ok(mut publisher) = publisher else { return };
    let subscriber = node.subscribe_raw(&topic, 2);
    assert!(subscriber.is_ok());
    let Ok(mut subscriber) = subscriber else { return };

    assert!(publisher.publish(&[]));
    assert!(publisher.publish(b"payload"));

    let first = subscriber.try_receive();
    assert!(matches!(first, Ok(Some(_))));
    let Ok(Some(first)) = first else { return };
    let first_identity = first.identity();
    assert_eq!(first_identity.as_ref().map(|value| value.topic), Some(topic.as_str()));
    assert!(first_identity.as_ref().is_some_and(|value| value.transport_sent_nanos > 0));
    assert_eq!(first.bytes(), Some(&[][..]));

    let second = subscriber.try_receive();
    assert!(matches!(second, Ok(Some(_))));
    let Ok(Some(second)) = second else { return };
    assert_eq!(second.bytes(), Some(b"payload".as_slice()));
    assert!(second.identity().is_some_and(|value| {
        value.transport_sent_nanos >= first_identity.map_or(0, |first| first.transport_sent_nanos)
    }));
    drop(second);
    drop(first);
    let third = subscriber.try_receive();
    assert!(matches!(third, Ok(None)));
}

#[test]
fn publisher_refuses_a_hash_that_exceeds_the_wire_header_limit() {
    let node = create_node("oversized_hash");
    let topic = format!("/hypha/raw/{}", unique_name("oversized_hash"));
    let md5 = "h".repeat(usize::from(u16::MAX) + 1);
    let publisher = node.publish_raw(&topic, TYPE_NAME, &md5, "");
    assert!(publisher.is_err());
}

#[test]
fn notified_endpoints_wake_a_waiting_node() {
    let node = create_node("notified_endpoint");
    let topic = format!("/hypha/raw/{}", unique_name("notified"));
    let publisher =
        node.publish_raw_configured(&topic, TYPE_NAME, MD5, "", PublisherOptions::default());
    assert!(publisher.is_ok());
    let Ok(mut publisher) = publisher else { return };
    let subscriber = node.subscribe_raw_with_options(&topic, SubscriberOptions::default());
    assert!(subscriber.is_ok());
    let Ok(mut subscriber) = subscriber else { return };
    assert!(publisher.publish(b"event"));
    // This subscriber is attached to no WaitSet, so the test reads it by hand.
    assert!(matches!(subscriber.try_receive(), Ok(Some(_))));
}

#[test]
fn latched_publisher_replays_to_a_late_subscriber() {
    let node = create_node("latched_endpoint");
    let topic = format!("/hypha/raw/{}", unique_name("latched"));
    let publisher = node.publish_raw_configured(
        &topic,
        TYPE_NAME,
        MD5,
        "uint8[] data",
        PublisherOptions::default().latched(),
    );
    assert!(publisher.is_ok());
    let Ok(mut publisher) = publisher else { return };
    assert!(publisher.publish(b"retained"));

    let subscriber =
        node.subscribe_raw_with_options(&topic, SubscriberOptions::default().keeping(1));
    assert!(subscriber.is_ok());
    let Ok(mut subscriber) = subscriber else { return };
    // Replay has no thread: the node's WaitSet replays on the join event.
    let waitset = WaitSet::new(&node);
    assert!(waitset.is_ok());
    let Ok(mut waitset) = waitset else { return };
    let started = Instant::now();
    let mut sample = None;
    while sample.is_none() && started.elapsed() < Duration::from_secs(2) {
        assert!(waitset.run_for(Duration::from_millis(20)).ok());
        sample = subscriber.try_receive().ok().flatten();
    }
    assert!(sample.is_some(), "the latched sample was not replayed");
    assert_eq!(sample.and_then(|s| s.bytes().map(<[u8]>::to_vec)), Some(b"retained".to_vec()));
}

#[test]
fn invalid_endpoint_inputs_are_refused() {
    let node = create_node("invalid_endpoint");
    assert!(node.subscribe_raw("", 1).is_err());
    assert!(node.publish_raw("", TYPE_NAME, MD5, "").is_err());
    assert!(node.publish_raw("/valid", "", MD5, "").is_err());
    assert!(node.publish_raw("/valid", TYPE_NAME, "", "").is_err());
}

#[test]
fn schema_api_reports_unavailable_invalid_and_displayed_errors() {
    let node = create_node("schema_errors");
    let absent = node.lookup_definition("/missing", TYPE_NAME, MD5);
    assert_eq!(absent, Ok(None));
    assert!(node.lookup_definition("", TYPE_NAME, MD5).is_err());

    let errors = [
        SchemaError::InvalidArgument,
        SchemaError::InvalidTopic,
        SchemaError::InvalidSchema,
        SchemaError::ResourceLimit,
        SchemaError::TooLarge,
        SchemaError::Disconnected,
        SchemaError::Backend,
        SchemaError::Unknown(99),
    ];
    for error in errors {
        assert_ne!(error.to_string(), "");
    }
    assert!(node.metrics().is_ok());
}

#[test]
fn the_topic_list_names_another_node_s_ports_and_their_direction() {
    let publishing = create_node("listed_publisher");
    let subscribing = create_node("listed_subscriber");
    let observer = create_node("topic_observer");
    let published_topic = format!("/hypha/raw/{}", unique_name("listed_out"));
    let subscribed_topic = format!("/hypha/raw/{}", unique_name("listed_in"));
    let publisher = publishing.publish_raw(&published_topic, TYPE_NAME, MD5, "uint8[] data");
    assert!(publisher.is_ok(), "a publisher on {published_topic}");
    // An opener creates the service the subscriber joins, then leaves.
    let opener = subscribing.publish_raw(&subscribed_topic, TYPE_NAME, MD5, "uint8[] data");
    assert!(opener.is_ok(), "an opener on {subscribed_topic}");
    let subscriber = subscribing.subscribe_raw_with_schema(
        &subscribed_topic,
        TYPE_NAME,
        MD5,
        SubscriberOptions::default(),
    );
    assert!(subscriber.is_ok(), "a subscriber on {subscribed_topic}");
    drop(opener);

    let topics = observer.list_topics();
    assert!(topics.is_ok(), "the topic list: {topics:?}");
    let Ok(topics) = topics else { return };
    let mut out_listing = None;
    let mut in_listing = None;
    for info in &topics {
        if info.name == published_topic {
            out_listing = Some((info.published, info.subscribed));
        }
        if info.name == subscribed_topic {
            in_listing = Some((info.published, info.subscribed));
        }
    }
    assert_eq!(out_listing, Some((true, false)), "{published_topic} in {topics:?}");
    assert_eq!(in_listing, Some((false, true)), "{subscribed_topic} in {topics:?}");
}

#[test]
fn a_subscriber_reports_the_topic_it_resolved() {
    let node = create_node("resolved_topic");
    let node_name = node.name();
    assert!(node_name.is_ok());
    let Ok(node_name) = node_name else { return };
    // The publisher opens the service the subscriber resolves to.
    let _opener = node.publish_raw("~/resolved", TYPE_NAME, MD5, "");
    let subscriber =
        node.subscribe_raw_with_schema("~/resolved", TYPE_NAME, MD5, SubscriberOptions::default());
    assert!(subscriber.is_ok());
    let Ok(subscriber) = subscriber else { return };

    assert_eq!(subscriber.topic(), Ok(format!("/{node_name}/resolved")));
}

#[test]
fn a_bounded_publisher_refuses_a_zero_limit_and_a_larger_message() {
    let node = create_node("bounded_payload");
    let topic = format!("/hypha/raw/{}", unique_name("bounded"));
    let zero = node.publish_raw_configured(
        &topic,
        TYPE_NAME,
        MD5,
        "uint8[] data",
        PublisherOptions::default().bounded(0),
    );
    assert!(zero.is_err(), "a zero payload limit is refused");

    let publisher = node.publish_raw_configured(
        &topic,
        TYPE_NAME,
        MD5,
        "uint8[] data",
        PublisherOptions::default().bounded(8),
    );
    assert!(publisher.is_ok());
    let Ok(mut publisher) = publisher else { return };
    assert!(publisher.publish(&[7_u8; 8]), "a message at the limit is sent");
    assert!(!publisher.publish(&[7_u8; 9]), "a message over the limit is refused");
}

#[test]
fn a_growable_publisher_sends_a_message_larger_than_its_first_segment() {
    let node = create_node("growable_payload");
    let topic = format!("/hypha/raw/{}", unique_name("growable"));
    let publisher = node.publish_raw_configured(
        &topic,
        TYPE_NAME,
        MD5,
        "uint8[] data",
        PublisherOptions::default().growable(),
    );
    assert!(publisher.is_ok());
    let Ok(mut publisher) = publisher else { return };
    let subscriber = node.subscribe_raw(&topic, 2);
    assert!(subscriber.is_ok());
    let Ok(mut subscriber) = subscriber else { return };

    let large = vec![3_u8; 1 << 20];
    assert!(publisher.publish(&[1_u8]));
    assert!(publisher.publish(&large));

    let first = subscriber.try_receive();
    assert!(matches!(first, Ok(Some(_))));
    let Ok(Some(first)) = first else { return };
    assert_eq!(first.bytes(), Some(&[1_u8][..]));
    let second = subscriber.try_receive();
    assert!(matches!(second, Ok(Some(_))));
    let Ok(Some(second)) = second else { return };
    assert_eq!(second.bytes().map(<[u8]>::len), Some(large.len()));
}
