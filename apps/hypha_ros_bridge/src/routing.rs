//! Direction-independent handoff values, routing predicates, and the ROS 1
//! receive and publish loops that the manifest and automatic modes share.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use hypha_node::sdk::ResolvedSchema;
use roslibrust::ros1::{PublisherAny, SubscriberAny};
use tokio::sync::watch;

use crate::error::BridgeError;
use crate::message::{add_ros_packet_length, strip_ros_packet};

/// Longest wait for a ROS 1 sample before the loop looks at `stop` again.
pub(crate) const ROS_RECEIVE_TIMEOUT: Duration = Duration::from_millis(50);

/// One of the two bridge data flows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeFlow {
    /// ROS 1 to hypha.
    Ros1ToHypha,
    /// hypha to ROS 1.
    HyphaToRos1,
}

/// A message handed from one worker to the other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeMsg {
    /// Route identity from the validated manifest, or `None` for automatic mode.
    pub route_id: Option<usize>,
    /// Topic associated with the receiving side.
    pub topic: Arc<str>,
    /// ROS type, MD5 and expanded definition, shared by every message of a route.
    pub identity: Arc<ResolvedSchema>,
    /// hypha publish timestamp.
    pub stamp_nanos: u64,
    /// ROS 1 message body without the TCPROS outer length prefix.
    pub bytes: Vec<u8>,
}

/// Returns whether ROS 1 should retain the publisher's last message.
#[must_use]
pub fn is_latched_topic(topic: &str) -> bool {
    topic == "/tf_static"
}

/// Ends a worker whose handoff peer is gone: a clean end when `stop` is set,
/// otherwise a [`BridgeError::ChannelClosed`] naming `handoff`.
///
/// # Errors
/// Returns the error when `stop` is not set.
pub(crate) fn handoff_closed(stop: &AtomicBool, handoff: &str) -> Result<(), BridgeError> {
    if stop.load(Ordering::Acquire) {
        return Ok(());
    }
    Err(BridgeError::ChannelClosed(handoff.into()))
}

/// Hands every ROS 1 sample of `subscriber` to the bus worker as a [`BridgeMsg`]
/// for `topic`, until `stop` is set.
///
/// Each sample carries the identity `identity` holds when it arrives; while it
/// holds `None`, samples are dropped. A malformed sample is dropped with a warning.
///
/// # Errors
/// Returns an error when the subscriber closes or the bus worker is gone
/// before `stop` is set.
pub(crate) async fn forward_from_ros(
    route_id: Option<usize>,
    topic: Arc<str>,
    identity: watch::Receiver<Option<Arc<ResolvedSchema>>>,
    mut subscriber: SubscriberAny,
    sender: mpsc::Sender<BridgeMsg>,
    stop: Arc<AtomicBool>,
) -> Result<(), BridgeError> {
    while !stop.load(Ordering::Acquire) {
        let result = tokio::select! {
            result = subscriber.next() => result,
            () = tokio::time::sleep(ROS_RECEIVE_TIMEOUT) => continue,
        };
        let Some(received) = result else {
            return Err(BridgeError::ChannelClosed(format!("ROS 1 subscriber for {topic}")));
        };
        let packet = match received {
            Ok(packet) => packet,
            Err(error) => {
                log::warn!("dropping malformed ROS 1 sample on {topic}: {error}");
                continue;
            }
        };
        let body = match strip_ros_packet(&packet) {
            Ok(body) => body,
            Err(error) => {
                log::warn!("dropping malformed ROS 1 sample on {topic}: {error}");
                continue;
            }
        };
        // The ROS subscriber does not report which connection a sample came from, so
        // samples received before the next graph poll may carry the old identity.
        let Some(current) = identity.borrow().clone() else {
            log::debug!("dropping a ROS sample on {topic} while its identity is read again");
            continue;
        };
        let Ok(stamp_nanos) = u64::try_from(hypha_node::sdk::clock::now_nanos()) else {
            log::warn!("dropping a ROS sample on {topic}: the hypha clock is negative");
            continue;
        };
        let message = BridgeMsg {
            route_id,
            topic: Arc::clone(&topic),
            identity: current,
            stamp_nanos,
            bytes: body.to_vec(),
        };
        if sender.send(message).is_err() {
            return handoff_closed(&stop, "ROS 1 to hypha handoff");
        }
    }
    Ok(())
}

/// Publishes one hypha sample on a ROS 1 publisher. A body too large for a
/// TCPROS length prefix is dropped with a warning.
///
/// # Errors
/// Returns an error when the ROS 1 publisher fails.
pub(crate) async fn publish_to_ros(
    publisher: &PublisherAny,
    message: &BridgeMsg,
) -> Result<(), BridgeError> {
    let packet = match add_ros_packet_length(&message.bytes) {
        Ok(packet) => packet,
        Err(error) => {
            log::warn!("dropping malformed hypha sample on {}: {error}", message.topic);
            return Ok(());
        }
    };
    publisher.publish(packet).await.map_err(BridgeError::ros_connection)
}
