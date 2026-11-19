//! TCPROS outer length-prefix framing.
//!
//! [`roslibrust::ros1::PublisherAny`] expects a body with the 4-byte length prefix
//! already attached, and [`roslibrust::ros1::SubscriberAny`] returns it included.

use crate::error::BridgeError;

/// Prepends the 4-byte TCPROS outer length prefix.
///
/// # Errors
///
/// Returns an error when the body is too large for a `u32` length prefix.
#[must_use = "the framed packet must be used, not discarded"]
pub fn add_ros_packet_length(body: &[u8]) -> Result<Vec<u8>, BridgeError> {
    let body_length = u32::try_from(body.len())
        .map_err(|_| BridgeError::Codec("ROS 1 body is too large".into()))?;
    Ok([body_length.to_le_bytes().as_slice(), body].concat())
}

/// Validates and removes the 4-byte TCPROS outer length prefix.
///
/// # Errors
///
/// Returns an error when the packet has no prefix or its declared body length
/// does not match the bytes that follow it.
#[must_use = "the unframed ROS body must be used"]
pub fn strip_ros_packet(packet: &[u8]) -> Result<&[u8], BridgeError> {
    let (length_bytes, body) = packet
        .split_first_chunk::<4>()
        .ok_or_else(|| BridgeError::Codec("ROS 1 packet has no length prefix".into()))?;
    let body_length = usize::try_from(u32::from_le_bytes(*length_bytes))
        .map_err(|_| BridgeError::Codec("ROS 1 body length does not fit usize".into()))?;
    if body.len() != body_length {
        return Err(BridgeError::Codec("ROS 1 body length does not match packet".into()));
    }
    Ok(body)
}
