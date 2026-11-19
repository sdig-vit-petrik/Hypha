//! The hypha to ROS 1 message handoff.
//!
//! The dispatch loop waits on this handoff inside a `select!` against a
//! timeout, so the receive must be cancel safe.

use std::sync::mpsc;
use std::time::Duration;

use tokio::sync::mpsc as async_mpsc;
use tokio::task::JoinHandle;

use crate::error::BridgeError;
use crate::routing::BridgeMsg;

/// Messages the forwarder may hold before the bus worker has to wait.
const HANDOFF_QUEUE_DEPTH: usize = 64;

/// Receives messages the bus worker hands to the ROS 1 dispatch loop.
pub struct HyphaToRosHandoff {
    receiver: async_mpsc::Receiver<BridgeMsg>,
    forwarder: JoinHandle<()>,
}

impl HyphaToRosHandoff {
    /// Takes ownership of the bus worker's receiving end and starts the one
    /// forwarding task that reads it.
    #[must_use]
    pub fn new(receiver: mpsc::Receiver<BridgeMsg>) -> Self {
        let (async_tx, async_rx) = async_mpsc::channel::<BridgeMsg>(HANDOFF_QUEUE_DEPTH);
        let forwarder = tokio::task::spawn_blocking(move || {
            // A blocking receive with no deadline ends when the bus worker
            // drops its sender, which is what shutdown does.
            while let Ok(message) = receiver.recv() {
                if async_tx.blocking_send(message).is_err() {
                    return;
                }
            }
        });
        Self { receiver: async_rx, forwarder }
    }

    /// Waits up to `timeout` for the next message.
    ///
    /// Returns `Ok(None)` when the timeout expires with nothing queued. The
    /// wait is cancel safe: a caller that abandons it loses no message.
    ///
    /// # Errors
    /// Returns an error when the bus worker's sending end is gone.
    pub async fn receive(&mut self, timeout: Duration) -> Result<Option<BridgeMsg>, BridgeError> {
        let Ok(received) = tokio::time::timeout(timeout, self.receiver.recv()).await else {
            return Ok(None);
        };
        received
            .map(Some)
            .ok_or_else(|| BridgeError::ChannelClosed("hypha to ROS 1 handoff".into()))
    }
}

impl Drop for HyphaToRosHandoff {
    fn drop(&mut self) {
        // The forwarder ends on its own once its send fails, which the closed
        // receiver guarantees. Aborting releases the blocking slot sooner.
        self.forwarder.abort();
    }
}
