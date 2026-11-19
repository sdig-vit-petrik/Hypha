//! Bridge error types.

use std::io;

/// Errors that stop bridge startup or a worker loop.
#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    /// A manifest or command line value is invalid.
    #[error("configuration error: {0}")]
    Config(String),
    /// The native ROS 1 node or a ROS 1 endpoint failed to start.
    #[error("ROS 1 connection error: {0}")]
    RosConnection(String),
    /// A ROS publisher reported no usable, consistent type identity.
    #[error("ROS 1 identity error: {0}")]
    Identity(String),
    /// A message could not be decoded or encoded.
    #[error("codec error: {0}")]
    Codec(String),
    /// A worker lost its handoff channel.
    #[error("channel closed: {0}")]
    ChannelClosed(String),
    /// The hypha node or endpoint failed to start or wait.
    #[error("hypha error: {0}")]
    Bus(String),
    /// The process could not register its shutdown handlers.
    #[error("signal setup error: {0}")]
    Signal(String),
    /// The Tokio runtime could not be created.
    #[error("runtime error: {0}")]
    Runtime(String),
    /// A file operation failed.
    #[error(transparent)]
    Io(#[from] io::Error),
}

impl BridgeError {
    /// A ROS 1 endpoint failure, reported with the library error's debug text.
    pub(crate) fn ros_connection(error: impl std::fmt::Debug) -> Self {
        Self::RosConnection(format!("{error:?}"))
    }
}
