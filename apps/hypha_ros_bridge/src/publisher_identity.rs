//! A ROS 1 publisher's type identity, read from its TCPROS connection header.
//!
//! A typed hypha consumer accepts a frame only when the md5 matches its own, so the
//! bridge never substitutes a placeholder: a publisher that cannot be probed is not bridged.

use std::collections::BTreeMap;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::error::BridgeError;
use crate::message::add_ros_packet_length;
use crate::ros_graph::RosGraphClient;

/// Longest connection header the bridge reads from a publisher.
pub const MAX_HEADER_BYTES: usize = 1_048_576;
/// Time one publisher has to answer the identity probe.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// The type identity one ROS 1 publisher announces for its topic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RosIdentity {
    /// ROS message type, `package/Name`.
    pub type_name: String,
    /// ROS message md5, 32 lower-case hexadecimal digits.
    pub md5: String,
    /// Expanded ROS message definition. A message without fields has an empty one.
    pub definition: String,
}

/// Encodes the subscriber header of an identity probe, length prefix included.
///
/// The wildcard `md5sum` and `type` make every publisher accept the probe and
/// answer with its own identity.
///
/// # Errors
/// Returns an error when a field is too large for a TCPROS length prefix.
pub fn encode_probe_header(caller_id: &str, topic: &str) -> Result<Vec<u8>, BridgeError> {
    let fields = [("callerid", caller_id), ("topic", topic), ("md5sum", "*"), ("type", "*")];
    let mut body = Vec::new();
    for (key, value) in fields {
        let field = format!("{key}={value}");
        body.extend_from_slice(&add_ros_packet_length(field.as_bytes())?);
    }
    add_ros_packet_length(&body)
}

/// Reads the identity from a publisher's connection header, without its
/// length prefix.
///
/// # Errors
/// Returns [`BridgeError::Identity`] when the publisher reported an error, or
/// when the header is malformed, repeats a field, or lacks a valid `type`,
/// `md5sum` or `message_definition`.
pub fn identity_from_header(header: &[u8]) -> Result<RosIdentity, BridgeError> {
    let fields = header_fields(header)?;
    if let Some(error) = fields.get("error") {
        return Err(BridgeError::Identity(format!("ROS publisher refused the probe: {error}")));
    }
    let type_name = required_field(&fields, "type")?;
    let md5 = required_field(&fields, "md5sum")?;
    let definition = required_field(&fields, "message_definition")?;

    let type_known = !type_name.is_empty() && type_name != "*";
    if !type_known {
        return Err(BridgeError::Identity(format!(
            "ROS publisher reports message type '{type_name}'"
        )));
    }
    if !hypha_node::sdk::is_valid_md5(&md5) {
        return Err(BridgeError::Identity(format!(
            "ROS publisher reports md5 '{md5}', not 32 lower-case hexadecimal digits"
        )));
    }
    Ok(RosIdentity { type_name, md5, definition })
}

/// Returns the one identity that every publisher of a topic reported.
///
/// `graph_type` is the type the ROS master lists for the topic; an empty
/// string means the master listed none.
///
/// # Errors
/// Returns [`BridgeError::Identity`] when no publisher reported an identity,
/// when publishers disagree, or when the master lists a different type.
pub fn agreed_identity(
    graph_type: &str,
    identities: &[RosIdentity],
) -> Result<RosIdentity, BridgeError> {
    let Some(first) = identities.first() else {
        return Err(BridgeError::Identity("no ROS publisher reported an identity".into()));
    };
    let publishers_disagree = identities.iter().any(|identity| identity != first);
    if publishers_disagree {
        return Err(BridgeError::Identity(
            "the ROS publishers of one topic report different identities".into(),
        ));
    }
    let master_disagrees = !graph_type.is_empty() && graph_type != first.type_name;
    if master_disagrees {
        return Err(BridgeError::Identity(format!(
            "the ROS master lists type '{graph_type}', the publisher reports '{}'",
            first.type_name
        )));
    }
    Ok(first.clone())
}

/// Reads the identity of the publisher at `publisher_uri` for `topic`.
///
/// # Errors
/// Returns [`BridgeError::RosConnection`] when the publisher cannot be reached
/// within [`PROBE_TIMEOUT`], and [`BridgeError::Identity`] when its header
/// carries no valid identity.
pub async fn probe_publisher(
    client: &RosGraphClient,
    topic: &str,
    publisher_uri: &str,
) -> Result<RosIdentity, BridgeError> {
    let probe = read_publisher_header(client, topic, publisher_uri);
    tokio::time::timeout(PROBE_TIMEOUT, probe).await.map_err(|_| {
        BridgeError::RosConnection(format!(
            "ROS publisher {publisher_uri} did not answer for {topic} within {PROBE_TIMEOUT:?}"
        ))
    })?
}

async fn read_publisher_header(
    client: &RosGraphClient,
    topic: &str,
    publisher_uri: &str,
) -> Result<RosIdentity, BridgeError> {
    let (host, port) =
        client.request_topic(publisher_uri, topic).await.map_err(BridgeError::RosConnection)?;
    let connection_error = |error: std::io::Error| {
        BridgeError::RosConnection(format!("identity probe to {host}:{port} for {topic}: {error}"))
    };
    let mut stream = TcpStream::connect((host.as_str(), port)).await.map_err(connection_error)?;
    let request = encode_probe_header(client.caller_id(), topic)?;
    stream.write_all(&request).await.map_err(connection_error)?;
    let length = stream.read_u32_le().await.map_err(connection_error)?;
    let length = usize::try_from(length).unwrap_or(usize::MAX);
    if length > MAX_HEADER_BYTES {
        return Err(BridgeError::Identity(format!(
            "ROS publisher {publisher_uri} sent a {length}-byte header for {topic}"
        )));
    }
    let mut header = vec![0_u8; length];
    stream.read_exact(&mut header).await.map_err(connection_error)?;
    identity_from_header(&header)
}

fn header_fields(header: &[u8]) -> Result<BTreeMap<String, String>, BridgeError> {
    let mut fields = BTreeMap::new();
    let mut rest = header;
    while !rest.is_empty() {
        let (length_bytes, after_length) = rest
            .split_first_chunk::<4>()
            .ok_or_else(|| BridgeError::Identity("ROS header field has no length".into()))?;
        let length = usize::try_from(u32::from_le_bytes(*length_bytes)).unwrap_or(usize::MAX);
        let (field, after_field) = after_length
            .split_at_checked(length)
            .ok_or_else(|| BridgeError::Identity("ROS header field is truncated".into()))?;
        let field = std::str::from_utf8(field)
            .map_err(|_| BridgeError::Identity("ROS header field is not UTF-8".into()))?;
        let (key, value) = field.split_once('=').ok_or_else(|| {
            BridgeError::Identity(format!("ROS header field '{field}' has no '='"))
        })?;
        if fields.insert(key.to_owned(), value.to_owned()).is_some() {
            return Err(BridgeError::Identity(format!("ROS header repeats field '{key}'")));
        }
        rest = after_field;
    }
    Ok(fields)
}

fn required_field(fields: &BTreeMap<String, String>, key: &str) -> Result<String, BridgeError> {
    fields
        .get(key)
        .cloned()
        .ok_or_else(|| BridgeError::Identity(format!("ROS publisher header has no '{key}' field")))
}
