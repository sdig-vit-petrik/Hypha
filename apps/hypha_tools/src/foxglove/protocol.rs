//! Foxglove WebSocket protocol v1 (subprotocol "foxglove.websocket.v1") definitions and serialization.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Subprotocol negotiated during WebSocket handshake.
pub const FOXGLOVE_SUBPROTOCOL: &str = "foxglove.websocket.v1";

/// Binary message header size: 1 byte opcode + 4 bytes subscriptionId + 8 bytes timestamp.
pub const BINARY_HEADER_SIZE: usize = 13;

/// Binary opcodes.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOpcode {
    /// Message data packet opcode (0x01).
    MessageData = 0x01,
    /// Time data packet opcode (0x02).
    TimeData = 0x02,
}

/// An advertised Foxglove channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Channel {
    /// Channel ID.
    pub id: u32,
    /// Bus topic name.
    pub topic: String,
    /// Message encoding (default: "ros1").
    pub encoding: String,
    /// Schema name (e.g. "`sensor_msgs/PointCloud2`").
    #[serde(rename = "schemaName")]
    pub schema_name: String,
    /// Full definition text.
    pub schema: String,
    /// Schema encoding (default: "ros1msg").
    #[serde(rename = "schemaEncoding")]
    pub schema_encoding: String,
}

/// Client subscription request entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientSubscription {
    /// Client-assigned subscription ID.
    pub id: u32,
    /// Target channel ID.
    #[serde(rename = "channelId")]
    pub channel_id: u32,
}

/// `ServerInfo` payload sent to newly connected clients.
#[derive(Debug, Clone, Serialize)]
pub struct ServerInfoMsg<'a> {
    /// Opcode string ("serverInfo").
    pub op: &'static str,
    /// Server name identifier.
    pub name: &'a str,
    /// Supported optional server capabilities.
    pub capabilities: Vec<&'a str>,
    /// Supported payload encodings.
    #[serde(rename = "supportedEncodings")]
    pub supported_encodings: Vec<&'a str>,
    /// Unique session identifier.
    #[serde(rename = "sessionId")]
    pub session_id: &'a str,
    /// Optional server metadata key-value map.
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
}

/// Serialize `serverInfo` JSON message.
#[must_use]
pub fn serialize_server_info<S: std::hash::BuildHasher>(
    name: &str,
    capabilities: &[&str],
    supported_encodings: &[&str],
    session_id: &str,
    metadata: &HashMap<String, String, S>,
) -> String {
    let mut concrete_metadata = HashMap::new();
    for (k, v) in metadata {
        concrete_metadata.insert(k.clone(), v.clone());
    }
    let msg = ServerInfoMsg {
        op: "serverInfo",
        name,
        capabilities: capabilities.to_vec(),
        supported_encodings: supported_encodings.to_vec(),
        session_id,
        metadata: concrete_metadata,
    };
    serde_json::to_string(&msg).unwrap_or_default()
}

#[derive(Debug, Serialize)]
struct AdvertiseMsg<'a> {
    op: &'static str,
    channels: &'a [Channel],
}

/// Serialize `advertise` JSON message.
#[must_use]
pub fn serialize_advertise(channels: &[Channel]) -> String {
    let msg = AdvertiseMsg { op: "advertise", channels };
    serde_json::to_string(&msg).unwrap_or_default()
}

/// Inbound client message from Foxglove Studio.
#[derive(Debug, Clone, Deserialize)]
pub struct ClientMessage {
    /// Inbound opcode ("subscribe", "unsubscribe", etc.).
    pub op: String,
    /// Subscriptions list if op is "subscribe".
    pub subscriptions: Option<Vec<ClientSubscription>>,
    /// Subscription IDs list if op is "unsubscribe".
    #[serde(rename = "subscriptionIds")]
    pub subscription_ids: Option<Vec<u32>>,
}

/// Parses an inbound JSON message from a WebSocket client.
#[must_use]
pub fn parse_client_message(json_str: &str) -> Option<ClientMessage> {
    serde_json::from_str(json_str).ok()
}

/// Creates a binary message frame `[0x01][subId:4B LE][timestampNs:8B LE][payload]`.
#[must_use]
pub fn create_binary_message(subscription_id: u32, timestamp_ns: u64, payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(BINARY_HEADER_SIZE.saturating_add(payload.len()));
    frame.push(BinaryOpcode::MessageData as u8);
    frame.extend_from_slice(&subscription_id.to_le_bytes());
    frame.extend_from_slice(&timestamp_ns.to_le_bytes());
    frame.extend_from_slice(payload);
    frame
}

/// Creates a time-broadcast frame `[0x02][timestampNs:8B LE]`.
#[must_use]
pub fn create_time_message(timestamp_ns: u64) -> Vec<u8> {
    let mut frame = Vec::with_capacity(1 + 8);
    frame.push(BinaryOpcode::TimeData as u8);
    frame.extend_from_slice(&timestamp_ns.to_le_bytes());
    frame
}
