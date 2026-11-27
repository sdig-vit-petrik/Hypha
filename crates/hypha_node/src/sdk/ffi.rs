//! The runtime items this SDK uses, from the safe Rust API of `hypha_node`.
//!
//! The integer constants the SDK compares against are defined from the status variant
//! with the same number.

pub(crate) use crate::HyphaStatus;
pub(crate) use crate::safe::{
    CallbackAction, Declaration, Error, FrameIdentity, HYPHA_DEFAULT_NO_PUBLISHER_WARNING_NANOS,
    HYPHA_RUN_REASON_DEADLINE, HYPHA_RUN_REASON_RUNTIME_FAILED, HYPHA_RUN_REASON_SETUP_FAILED,
    HyphaNode, HyphaPublisher, HyphaReceivedSample, HyphaSubscriber, HyphaWaitSet,
    HyphaWaitSetRunReport, ListedNode, ListedServicePort, PublisherSettings, SampleDelivery,
    SubscriberIdentity, SubscriberSettings, SubscriptionHandler, WaitSetLease,
    clock_disable_sim_time, clock_enable_sim_time, clock_now_nanos, clock_steady_now_nanos, domain,
    list_nodes, list_service_ports, loan_send, node_create, node_default_waitset, node_destroy,
    node_metrics, node_name, node_ok, node_shutdown, publish_with_qos, publisher_loan_frame,
    publisher_min_subscriber_buffer, sample_identity, sample_payload, schema_lookup,
    schema_resolve_hash, schema_resolve_hash_now, subscribe_typed, subscriber_counters,
    subscriber_metrics, subscriber_publisher_count, subscriber_receive, subscriber_topic,
    waitset_attach_subscription, waitset_detach, waitset_request_stop, waitset_run,
};

pub(crate) const HYPHA_STATUS_OK: i32 = HyphaStatus::Ok as i32;
pub(crate) const HYPHA_STATUS_INVALID_ARGUMENT: i32 = HyphaStatus::InvalidArgument as i32;
pub(crate) const HYPHA_STATUS_INVALID_TOPIC: i32 = HyphaStatus::InvalidTopic as i32;
pub(crate) const HYPHA_STATUS_TOO_LARGE: i32 = HyphaStatus::TooLarge as i32;
pub(crate) const HYPHA_STATUS_DISCONNECTED: i32 = HyphaStatus::Disconnected as i32;
pub(crate) const HYPHA_STATUS_BACKEND: i32 = HyphaStatus::Backend as i32;
pub(crate) const HYPHA_STATUS_PANIC: i32 = HyphaStatus::Panic as i32;
pub(crate) const HYPHA_STATUS_NOT_AVAILABLE: i32 = HyphaStatus::NotAvailable as i32;
pub(crate) const HYPHA_STATUS_INVALID_SCHEMA: i32 = HyphaStatus::InvalidSchema as i32;
pub(crate) const HYPHA_STATUS_RESOURCE_LIMIT: i32 = HyphaStatus::ResourceLimit as i32;
pub(crate) const HYPHA_STATUS_QOS_MISMATCH: i32 = HyphaStatus::QosMismatch as i32;
pub(crate) const HYPHA_STATUS_INVALID_STATE: i32 = HyphaStatus::InvalidState as i32;

/// The C ABI status number of `error`, as the SDK's messages and errors name it.
pub(crate) fn status_code(error: Error) -> i32 {
    HyphaStatus::from(error) as i32
}
