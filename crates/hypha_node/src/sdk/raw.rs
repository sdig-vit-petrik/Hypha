//! Raw pub/sub: a topic identity plus bytes, with no compiled-in message type.

use std::sync::Arc;

mod discovery;
mod publisher;
pub use discovery::{GraphWatch, domain, list_nodes};
use discovery::{list_topic_ports, open_graph_watch};
/// True when `name` is a CLI tool node name, `_hyphacli_<tool>`. Any number of
/// processes run under one such name, and the tools do not show them.
pub use hypha_core::port::is_cli_node_name;
/// True when `topic` is matched by the subscription `pattern`: `*` is one
/// segment, a trailing `**` the remaining ones, `**` alone every topic.
pub use hypha_core::port::topic_matches_pattern;
pub use publisher::RawPublisher;

use crate::sdk::ffi;
use crate::sdk::loan::{RawSubscriber, StreamSubscribeError, subscribe_on, subscribe_stream_on};
use crate::sdk::options::{PublisherOptions, SubscriberOptions};
use crate::sdk::status::numbered_error;

/// Why one wait for work of a caller's own wait loop returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitResult {
    /// A subscription may have queued data.
    Notified,
    /// The monotonic timeout elapsed.
    TimedOut,
    /// The application ended the wait because its node stopped. The library never
    /// returns this value.
    Interrupted,
}

/// One process registered on the bus.
#[derive(Debug, Clone)]
pub struct NodeInfo {
    /// Node registration name.
    pub name: String,
    /// Executable name, empty when the registry carries none.
    pub executable: String,
    /// Process ID.
    pub pid: i32,
    /// Whether the process is currently alive.
    pub is_alive: bool,
}

/// A stream currently open on the hypha bus: one topic and one schema. A topic of two
/// schemas is two of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopicInfo {
    /// Topic name.
    pub name: String,
    /// The schema hash of the stream's frames, as [`crate::sdk::SampleIdentity::schema_hash`]; 0 for
    /// the stream of no schema.
    pub schema_hash: u64,
    /// At least one publisher exists outside the caller.
    pub published: bool,
    /// How many publishers exist outside the caller.
    pub publishers: usize,
    /// At least one subscriber exists outside the caller.
    pub subscribed: bool,
    /// How many subscribers exist outside the caller.
    pub subscribers: usize,
}

/// A session on the bus: the handle every endpoint is made from.
pub struct BusNode {
    pub(crate) node: Arc<ffi::HyphaNode>,
}

impl BusNode {
    /// Joins the bus under `name`.
    ///
    /// A CLI tool joins under `_hyphacli_<tool>`: any number of its processes can run
    /// at once, and it has no `~/` names.
    /// # Errors
    /// Returns a message when the library refuses the session.
    pub fn new(name: &str) -> Result<Self, String> {
        ffi::node_create(name, None, None)
            .map(|node| Self { node })
            .map_err(|error| numbered_error(format!("cannot join the bus as '{name}'"), error))
    }

    /// The node name: the name given to [`BusNode::new`].
    ///
    /// # Errors
    /// Returns a message when the library refuses the read.
    pub fn name(&self) -> Result<String, String> {
        Ok(ffi::node_name(&self.node).to_owned())
    }

    /// Subscribes to a literal topic or a wildcard pattern (`"**"`).
    ///
    /// A `*` must be a whole segment, and `**` must be the last one. It names no schema, so
    /// it reads every stream of each matching topic, one per schema, that exists when it is
    /// created; it is refused for a topic nobody has opened yet. A `depth` of zero keeps as
    /// many samples as the topic's service policy allows.
    /// # Errors
    /// Returns a message when the library refuses the subscription.
    pub fn subscribe_raw(
        &self,
        topic_pattern: &str,
        depth: usize,
    ) -> Result<RawSubscriber, String> {
        self.subscribe_raw_with_options(topic_pattern, SubscriberOptions::default().keeping(depth))
    }

    /// Subscribes with explicit queue and notification `QoS`, as
    /// [`BusNode::subscribe_raw`] does.
    ///
    /// # Errors
    /// Returns a message when the library refuses the subscription.
    pub fn subscribe_raw_with_options(
        &self,
        topic_pattern: &str,
        options: SubscriberOptions,
    ) -> Result<RawSubscriber, String> {
        subscribe_on(&self.node, topic_pattern, "", "", options)
    }

    /// Subscribes to the stream of `md5` on the topic: messages of `data_type` with that
    /// MD5, and no frame of another schema of the topic. A stream that no publisher has
    /// opened is `NotAvailable`, except on a framework topic.
    ///
    /// # Errors
    /// Returns a message when the library refuses the subscription.
    pub fn subscribe_raw_with_schema(
        &self,
        topic_pattern: &str,
        data_type: &str,
        md5: &str,
        options: SubscriberOptions,
    ) -> Result<RawSubscriber, String> {
        subscribe_on(&self.node, topic_pattern, data_type, md5, options)
    }

    /// Subscribes to the one stream of `schema_hash` on the literal `topic`, which the caller
    /// knows from [`BusNode::list_topics`]: no MD5 is needed, so a stream whose publisher gave
    /// no definition is read too. A stream that does not exist is `NotAvailable`.
    ///
    /// # Errors
    /// [`StreamSubscribeError::SlotsTaken`] when other tools hold every observer slot of the
    /// stream, and a message when the library refuses the subscription otherwise.
    pub fn subscribe_raw_stream(
        &self,
        topic: &str,
        schema_hash: u64,
        options: SubscriberOptions,
    ) -> Result<RawSubscriber, StreamSubscribeError> {
        subscribe_stream_on(&self.node, topic, schema_hash, options)
    }

    /// Watches the graph: whether an endpoint of the domain came or went. See [`GraphWatch`].
    ///
    /// # Errors
    /// Returns a message when the notification service cannot be opened.
    pub fn graph_watch(&self) -> Result<GraphWatch, String> {
        open_graph_watch(&self.node)
    }

    /// Publishes `topic` under an explicit type identity.
    ///
    /// # Errors
    /// Returns a message when the library refuses the publication.
    pub fn publish_raw(
        &self,
        topic: &str,
        data_type: &str,
        md5: &str,
        definition: &str,
    ) -> Result<RawPublisher, String> {
        self.publish_raw_configured(topic, data_type, md5, definition, PublisherOptions::default())
    }

    /// Publishes with explicit latch and notification `QoS`.
    ///
    /// # Errors
    /// Returns a message when the library refuses the publication.
    pub fn publish_raw_configured(
        &self,
        topic: &str,
        data_type: &str,
        md5: &str,
        definition: &str,
        options: PublisherOptions,
    ) -> Result<RawPublisher, String> {
        let payload = options
            .checked_max_payload()
            .map_err(|cause| format!("cannot publish '{topic}': {cause}"))?;
        let settings = ffi::PublisherSettings {
            latch: options.latch,
            loaned_samples: 0,
            declaration: ffi::Declaration::undeclared(payload),
        };
        ffi::publish_with_qos(&self.node, topic, data_type, md5, definition, settings)
            .map(|publisher| RawPublisher { publisher })
            .map_err(|error| numbered_error(format!("publish failed for '{topic}'"), error))
    }

    /// Whether this session has not been asked to stop.
    ///
    /// SIGINT or SIGTERM asks every session of the process to stop instead of ending
    /// the process; a second signal ends it. A loop checks this after each wait.
    #[must_use]
    pub fn ok(&self) -> bool {
        ffi::node_ok(&self.node)
    }

    /// Asks this session to stop, and releases a blocked waiter.
    pub fn shutdown(&self) {
        ffi::node_shutdown(&self.node);
    }

    /// Every process the bus registry currently knows about, alive or dead.
    ///
    /// # Errors
    /// Returns an error when the backend cannot inspect the node registry.
    pub fn list_nodes(&self) -> Result<Vec<NodeInfo>, String> {
        list_nodes()
    }

    /// Lists the streams currently open on the bus: a topic of two schemas is two entries.
    ///
    /// The result does not include the caller's own ports.
    ///
    /// # Errors
    /// Returns an error when the backend cannot inspect the service registry.
    pub fn list_topics(&self) -> Result<Vec<TopicInfo>, String> {
        list_topic_ports(&self.node)
    }
}

impl Drop for BusNode {
    fn drop(&mut self) {
        ffi::node_shutdown(&self.node);
        ffi::node_destroy(&self.node);
    }
}
