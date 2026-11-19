//! Tests for Foxglove WebSocket v1 protocol and binary encoding.

#![allow(clippy::indexing_slicing)]
#![allow(clippy::unwrap_used)]

use hypha_tools::foxglove::protocol::{
    BINARY_HEADER_SIZE, BinaryOpcode, Channel, create_binary_message, parse_client_message,
    serialize_advertise, serialize_server_info,
};
use std::collections::HashMap;

#[test]
fn serializes_server_info() {
    let mut meta = HashMap::new();
    meta.insert("host".to_string(), "localhost".to_string());
    let json = serialize_server_info(
        "test_server",
        &["clientPublish", "connectionGraph"],
        &["ros1"],
        "123456789",
        &meta,
    );

    assert!(json.contains("\"op\":\"serverInfo\""), "got: {json}");
    assert!(json.contains("\"name\":\"test_server\""), "got: {json}");
    assert!(json.contains("\"sessionId\":\"123456789\""), "got: {json}");
    assert!(json.contains("\"clientPublish\""), "got: {json}");
    assert!(json.contains("\"ros1\""), "got: {json}");
    assert!(json.contains("\"host\":\"localhost\""), "got: {json}");
}

#[test]
fn serializes_advertise() {
    let ch1 = Channel {
        id: 1,
        topic: "/sensors/lidar".to_string(),
        encoding: "ros1".to_string(),
        schema_name: "sensor_msgs/PointCloud2".to_string(),
        schema: "uint32 height
uint32 width
"
        .to_string(),
        schema_encoding: "ros1msg".to_string(),
    };

    let json = serialize_advertise(&[ch1]);
    assert!(json.contains("\"op\":\"advertise\""), "got: {json}");
    assert!(json.contains("\"id\":1"), "got: {json}");
    assert!(json.contains("\"topic\":\"/sensors/lidar\""), "got: {json}");
    assert!(json.contains("\"schemaName\":\"sensor_msgs/PointCloud2\""), "got: {json}");
}

#[test]
fn parses_client_subscribe_and_unsubscribe() {
    let subscribe_json =
        r#"{"op":"subscribe","subscriptions":[{"id":10,"channelId":1},{"id":20,"channelId":2}]}"#;
    let parsed_sub = parse_client_message(subscribe_json).unwrap();
    assert_eq!(parsed_sub.op, "subscribe");
    let subs = parsed_sub.subscriptions.unwrap();
    assert_eq!(subs.len(), 2);
    assert_eq!(subs[0].id, 10);
    assert_eq!(subs[0].channel_id, 1);
    assert_eq!(subs[1].id, 20);
    assert_eq!(subs[1].channel_id, 2);

    let unsubscribe_json = r#"{"op":"unsubscribe","subscriptionIds":[10,20]}"#;
    let parsed_unsub = parse_client_message(unsubscribe_json).unwrap();
    assert_eq!(parsed_unsub.op, "unsubscribe");
    let unsubs = parsed_unsub.subscription_ids.unwrap();
    assert_eq!(unsubs, vec![10, 20]);
}

#[test]
fn formats_binary_message_data() {
    let payload = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE];
    let sub_id = 1001u32;
    let stamp_ns = 1_700_000_000_123_456_789_u64;

    let frame = create_binary_message(sub_id, stamp_ns, &payload);
    assert_eq!(frame.len(), BINARY_HEADER_SIZE + payload.len());
    assert_eq!(frame[0], BinaryOpcode::MessageData as u8);

    let unpacked_sub_id = u32::from_le_bytes(frame[1..5].try_into().unwrap());
    assert_eq!(unpacked_sub_id, sub_id);

    let unpacked_stamp = u64::from_le_bytes(frame[5..13].try_into().unwrap());
    assert_eq!(unpacked_stamp, stamp_ns);

    assert_eq!(&frame[BINARY_HEADER_SIZE..], &payload);
}
