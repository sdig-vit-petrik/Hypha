//! The transport port: the values and the rules that a transport must follow.
//!
//! It names no transport: it holds what a caller passes to one ([`TopicSpec`],
//! [`ServiceSpec`], [`SchemaSelector`]), the names it accepts, the shared wildcard matcher, [`PortError`]
//! and the run values in [`dispatch`].

pub mod dispatch;
mod error;
mod service;
mod spec;

pub use error::PortError;
pub use service::{
    CallSendError, DEFAULT_SERVICE_PAYLOAD, MAX_POLICY_CLIENTS, MAX_POLICY_OUTSTANDING,
    MAX_SERVICE_NAME_BYTES, MAX_SERVICE_PAYLOAD, MAX_SERVICE_TIMEOUT, ReplyError,
    SERVICE_OBSERVER_CLIENTS, ServerHolder, ServiceError, ServicePolicy, ServiceSpec,
};
pub use spec::{
    CLI_NODE_PREFIX, FRAMEWORK_TOPIC_NAMESPACES, MAX_NODE_NAME_LENGTH, MAX_POLICY_DEPTH,
    MAX_POLICY_NAME_LENGTH, MAX_POLICY_PUBLISHERS, MAX_POLICY_SUBSCRIBERS,
    MAX_RECORDED_TOPIC_NAME_BYTES, MAX_TOPIC_NAME_BYTES, PUBLISHER_LOANED_SAMPLES, PayloadCapacity,
    QosPolicy, QosRequest, SchemaScope, SchemaSelector, TopicSpec, is_cli_node_name, is_identifier,
    is_node_name, is_reserved_topic_name, is_valid_topic_name, is_valid_topic_pattern,
    resolve_node_name, topic_matches_pattern,
};
