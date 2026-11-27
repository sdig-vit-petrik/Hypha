//! Node discovery and the domain query.

use super::{NodeInfo, TopicInfo};
use crate::sdk::ffi;
use crate::sdk::status::{named_error, numbered_error};

/// The domain of this process, or an empty string for the domain "none".
///
/// The environment variable `HYPHA_DOMAIN` selects it once per process.
///
/// # Errors
/// Returns an error when `HYPHA_DOMAIN` holds an invalid name.
pub fn domain() -> Result<String, String> {
    ffi::domain().map(str::to_owned).map_err(|error| named_error("cannot read the domain", error))
}

/// Every process the bus registry currently knows about, alive or dead.
///
/// # Errors
/// Returns an error when the backend cannot inspect the node registry.
pub fn list_nodes() -> Result<Vec<NodeInfo>, String> {
    let mut found: Vec<NodeInfo> = Vec::new();
    let mut collect = |node: &ffi::ListedNode<'_>| {
        found.push(NodeInfo {
            name: node.name.to_string_lossy().into_owned(),
            executable: node.executable.to_string_lossy().into_owned(),
            pid: node.pid,
            is_alive: node.alive,
        });
    };
    ffi::list_nodes(&mut collect).map_err(|error| numbered_error("node listing failed", error))?;
    Ok(found)
}

pub(super) fn list_topic_ports(node: &ffi::HyphaNode) -> Result<Vec<TopicInfo>, String> {
    let mut topics: Vec<TopicInfo> = Vec::new();
    let mut collect = |service: &ffi::ListedServicePort<'_>| {
        topics.push(TopicInfo {
            name: service.name.to_string_lossy().into_owned(),
            schema_hash: service.schema_hash,
            published: service.publishers > 0,
            publishers: service.publishers,
            subscribed: service.subscribers > 0,
        });
    };
    ffi::list_service_ports(node, &mut collect)
        .map_err(|error| numbered_error("topic listing failed", error))?;
    Ok(topics)
}

/// A watch of the bus's graph: whether an endpoint came or went since it last asked.
///
/// A tool that follows streams as they appear opens one before it lists the streams, and
/// lists again when [`GraphWatch::changed`] says so, with no timer of its own. It is the
/// event a node's housekeeping hears, read on the caller's thread.
pub struct GraphWatch {
    watch: ffi::GraphWatch,
}

impl GraphWatch {
    /// Whether a publisher or a subscriber of the domain came or went since the last call.
    /// One non-blocking read; it takes every pending notification.
    ///
    /// # Errors
    /// Returns a message when the notification service cannot be read.
    pub fn changed(&self) -> Result<bool, String> {
        self.watch.changed().map_err(|error| numbered_error("graph event failed", error))
    }
}

impl GraphWatch {
    /// Waits up to `timeout` for a publisher or a subscriber of the domain to come or go, and
    /// takes the notifications; true when one came. The thread sleeps in the kernel.
    ///
    /// # Errors
    /// Returns a message when the notification service cannot be read.
    pub fn wait(&self, timeout: std::time::Duration) -> Result<bool, String> {
        self.watch.wait(timeout).map_err(|error| numbered_error("graph event failed", error))
    }
}

pub(super) fn open_graph_watch(node: &ffi::HyphaNode) -> Result<GraphWatch, String> {
    ffi::graph_watch(node)
        .map(|watch| GraphWatch { watch })
        .map_err(|error| numbered_error("cannot watch the graph", error))
}
