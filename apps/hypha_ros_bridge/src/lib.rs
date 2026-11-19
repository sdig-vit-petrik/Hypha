//! ROS 1 to hypha migration bridge.
//!
//! With `--config`, the bridge validates one complete TOML manifest before it
//! connects to ROS 1. Without a manifest, it creates same-name routes at runtime.
//! It carries every message body as opaque bytes: none is decoded.

#![deny(unsafe_code)]

pub mod automatic;
mod automatic_runtime;
pub mod handoff;
pub mod manifest;
mod message;
pub mod publisher_identity;
pub mod ros_graph;
pub mod routing;
mod runtime;

pub mod error;

pub use error::BridgeError;
pub use manifest::{BridgeDirection, BridgeManifest, BridgeRoute};
pub use message::{add_ros_packet_length, strip_ros_packet};
pub use routing::{BridgeFlow, BridgeMsg, is_latched_topic};
pub use runtime::{BridgeConfig, parse_args, run, run_with_args};
