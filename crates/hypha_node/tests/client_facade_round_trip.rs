//! The Rust client facade: raw round trips and the schema filter.

use hypha_node::sdk::{BusNode, PublisherOptions, RawSubscriber, SubscriberOptions};
use hypha_testkit::unique_name as unique;

const TYPE_NAME: &str = "std_msgs/MessageForCoverage";
const MD5: &str = "d9d213e8c146065ec958333c617eb1a6";

/// One named metric of a subscriber, or `None` when the read fails or it is not emitted.
fn metric(subscriber: &RawSubscriber, name: &str) -> Option<u64> {
    subscriber.metrics().ok()?.into_iter().find(|(metric, _)| metric == name).map(|(_, v)| v)
}

#[test]
fn node_raw_pub_sub_and_registry_round_trip() {
    let node_name = unique("node");
    let topic = format!("/hypha/coverage/{}", unique("topic"));
    let node_result = BusNode::new(&node_name);
    assert!(node_result.is_ok());
    let Ok(node) = node_result else { return };
    assert!(node.ok());

    let publisher_result = node.publish_raw_configured(
        &topic,
        "std_msgs/Bytes",
        "992ce8a1687cec8c8bd883ec73ca41d1",
        "string data",
        PublisherOptions::default().latched(),
    );
    assert!(publisher_result.is_ok());
    let Ok(mut publisher) = publisher_result else { return };
    let subscriber_result =
        node.subscribe_raw_with_options(&topic, SubscriberOptions::default().keeping(2));
    assert!(subscriber_result.is_ok());
    let Ok(mut subscriber) = subscriber_result else { return };
    assert!(publisher.publish(b"payload"));
    let sample = subscriber.try_receive();
    assert!(matches!(sample, Ok(Some(_))));
    let Ok(Some(sample)) = sample else { return };
    let identity = sample.identity();
    assert_eq!(identity.as_ref().map(|value| value.topic), Some(topic.as_str()));
    assert_eq!(sample.bytes(), Some(b"payload".as_slice()));
    assert!(identity.is_some_and(|value| value.transport_sent_nanos > 0));
    assert!(matches!(subscriber.try_receive(), Ok(None)));
    let nodes = node.list_nodes();
    assert!(nodes.is_ok());
    let Ok(nodes) = nodes else { return };
    assert!(!nodes.is_empty());
    node.shutdown();
    assert!(!node.ok());
}

#[test]
fn a_schema_filtered_subscription_passes_matching_frames() {
    let suffix = unique("typed");
    let node_result = BusNode::new(&format!("{suffix}_node"));
    assert!(node_result.is_ok());
    let Ok(node) = node_result else { return };
    let topic = format!("/hypha/coverage/{suffix}");
    let valid_result = node.publish_raw(&topic, TYPE_NAME, MD5, "");
    assert!(valid_result.is_ok());
    let Ok(mut valid) = valid_result else { return };
    let subscriber_result = node.subscribe_raw_with_schema(
        &topic,
        TYPE_NAME,
        MD5,
        SubscriberOptions::default().keeping(8),
    );
    assert!(subscriber_result.is_ok());
    let Ok(mut subscriber) = subscriber_result else { return };
    assert!(valid.publish(b"valid"));
    let sample = subscriber.try_receive();
    assert!(matches!(sample, Ok(Some(_))));
    let Ok(Some(sample)) = sample else { return };
    assert_eq!(sample.bytes(), Some(b"valid".as_slice()));
    assert_eq!(metric(&subscriber, "integrity.schema_mismatch"), Some(0));
}

#[test]
fn a_version_skewed_publisher_is_another_stream_that_a_schema_subscription_never_meets() {
    let suffix = unique("skew");
    let node_result = BusNode::new(&format!("{suffix}_node"));
    assert!(node_result.is_ok());
    let Ok(node) = node_result else { return };
    let topic = format!("/hypha/coverage/{suffix}");
    let skew_result = node.publish_raw(&topic, TYPE_NAME, "00000000000000000000000000000000", "");
    let own_result = node.publish_raw(&topic, TYPE_NAME, MD5, "");
    assert!(skew_result.is_ok() && own_result.is_ok());
    let (Ok(mut skew), Ok(_own)) = (skew_result, own_result) else { return };
    let subscriber_result = node.subscribe_raw_with_schema(
        &topic,
        TYPE_NAME,
        MD5,
        SubscriberOptions::default().keeping(8),
    );
    assert!(subscriber_result.is_ok());
    let Ok(mut subscriber) = subscriber_result else { return };
    assert!(skew.publish(b"valid"));
    // The skewed frame travels on its own stream: no refusal, nothing counted.
    assert!(matches!(subscriber.try_receive(), Ok(None)));
    assert_eq!(metric(&subscriber, "integrity.schema_mismatch"), Some(0));
}
