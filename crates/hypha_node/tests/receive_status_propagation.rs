//! A refused frame must return an error rather than look like an empty queue.

#![allow(clippy::expect_used)]

use hypha_node::sdk::{BusNode, SubscriberOptions};
use hypha_testkit::unique_name;

const TYPE_NAME: &str = "std_msgs/Bytes";
const MD5: &str = "d9d213e8c146065ec958333c617eb1a6";

fn create_node(label: &str) -> BusNode {
    BusNode::new(&unique_name(label)).expect("node creation succeeds")
}

#[test]
fn a_refused_frame_is_reported_and_not_read_as_an_empty_queue() {
    let node = create_node("refused_frame");
    let topic = format!("/hypha/status/{}", unique_name("refused"));
    let mut publisher =
        node.publish_raw(&topic, TYPE_NAME, MD5, "").expect("publisher creation succeeds");
    let mut subscriber = node.subscribe_raw(&topic, 1).expect("subscriber creation succeeds");

    assert!(publisher.publish(b"first"));
    let first = subscriber.try_receive().expect("first receive succeeds");
    assert!(first.is_some());

    assert!(publisher.publish(b"second"));
    assert!(subscriber.try_receive().is_err());
}

#[test]
fn an_empty_queue_reports_no_fault() {
    let node = create_node("empty_queue");
    let topic = format!("/hypha/status/{}", unique_name("empty"));
    // The publisher opens the service; it stays alive so the subscriber finds it.
    let _publisher =
        node.publish_raw(&topic, TYPE_NAME, MD5, "").expect("publisher creation succeeds");
    let mut subscriber = node
        .subscribe_raw_with_schema(&topic, TYPE_NAME, "", SubscriberOptions::default().keeping(8))
        .expect("subscriber creation succeeds");

    assert!(matches!(subscriber.try_receive(), Ok(None)));
}

#[test]
fn accepted_traffic_reports_no_fault() {
    let node = create_node("accepted");
    let topic = format!("/hypha/status/{}", unique_name("accepted"));
    let mut publisher =
        node.publish_raw(&topic, TYPE_NAME, MD5, "").expect("publisher creation succeeds");
    let mut subscriber = node
        .subscribe_raw_with_schema(&topic, TYPE_NAME, "", SubscriberOptions::default().keeping(8))
        .expect("subscriber creation succeeds");

    assert!(publisher.publish(b"telemetry"));

    assert!(matches!(subscriber.try_receive(), Ok(Some(_))));
}
