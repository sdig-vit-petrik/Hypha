//! Foxglove WebSocket bridge protocol and server implementations.

pub mod bridge;
pub mod protocol;
pub mod server;

pub use bridge::{BridgeConfig, FoxgloveBridge};
pub use protocol::*;
pub use server::{FoxgloveServer, ServerOptions};
