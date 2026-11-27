//! The safe Rust functions behind the C ABI, for Rust programs of this workspace.
//!
//! Each exported `hypha_*` function that the Rust SDK uses wraps the function of the same
//! name without the `hypha_` prefix. A Rust program that calls these shares this crate's
//! process-wide state with the C path, so it must not also load the shared library.

pub use crate::clock::{
    clock_advance_sim_time, clock_disable_sim_time, clock_enable_sim_time, clock_is_sim_time,
    clock_now_nanos, clock_set_sim_time, clock_steady_now_nanos,
};
pub use crate::config::{
    HyphaConfig, config_create, config_ignore, config_resolve_key, config_unread_keys, node_config,
};
pub use crate::cycle::{
    CycleId, CycleSnapshots, CycleStepHandler, EmissionView, HyphaInputSource, ImageView,
    cycle_bind_input, cycle_bind_input_supervision, cycle_bind_output, cycle_bind_transforms,
    cycle_binding, cycle_commit, cycle_create, cycle_destroy, cycle_expect_participant,
    cycle_health, cycle_set_class_based, cycle_set_clock_driver, cycle_set_host_clock,
    cycle_set_trigger, cycle_tick, emission_commit, emission_reserve, image_alive, image_binding,
    image_held, image_lookup_transform, image_reject_sample, image_sample, image_set_ignored,
    input_source_create, input_source_destroy,
};
pub use crate::domain::domain;
pub use crate::introspection_abi::{config_record_config, node_introspect, node_record_config};
pub use crate::last_error::last_error;
pub use crate::loan::{
    HyphaLoan, loan_send, loan_shrink, publisher_loan_frame, publisher_reserve_frame,
};
pub use crate::metrics::{node_metrics, subscriber_metrics};
pub use crate::node::{HyphaNode, node_create, node_destroy, node_name, node_ok, node_shutdown};
pub use crate::node_discovery::{
    GraphWatch, ListedNode, ListedServicePort, graph_watch, list_nodes, list_service_ports,
};
pub use crate::node_state::node_state;
pub use crate::param_topic::param_topic;
pub use crate::parameter_parse::{ParamRead, config_list_len, config_list_value, config_value};
pub use crate::publisher::{
    HyphaPublisher, PublisherSettings, publish_with_qos, publisher_min_subscriber_buffer,
    publisher_subscriber_count,
};
pub use crate::qos::Declaration;
pub use crate::sample::{HyphaReceivedSample, sample_identity, sample_payload};
pub use crate::schema::{schema_lookup, schema_resolve_hash, schema_resolve_hash_now};
pub use crate::status::Error;
pub use crate::subscriber::{
    FrameIdentity, HYPHA_DEFAULT_NO_PUBLISHER_WARNING_NANOS, HyphaSubscriber, SubscriberIdentity,
    SubscriberSettings, subscribe_stream, subscribe_typed, subscriber_counters,
    subscriber_publisher_count, subscriber_receive, subscriber_topic,
};
pub use crate::transforms::buffer::{
    HyphaTransformBuffer, transform_buffer_create, transform_buffer_destroy,
    transform_buffer_lookup,
};
pub use crate::transforms::{HyphaSentTransform, TransformLookup, transform_check};
pub use crate::waitset::registrations::waitset_attach_foreign;
pub use crate::waitset::{
    CallbackAction, DeliveryHandler, ForeignCallbacks, HYPHA_RUN_REASON_DEADLINE,
    HYPHA_RUN_REASON_RUNTIME_FAILED, HYPHA_RUN_REASON_SETUP_FAILED, HYPHA_RUN_REASON_STOPPED,
    HyphaTimerState, HyphaWaitSet, HyphaWaitSetRunReport, IntervalHandler, SampleDelivery,
    SubscriptionHandler, WaitSetClaim, WaitSetLease, WaitSetShared, node_default_waitset,
    waitset_attach_subscription, waitset_detach, waitset_request_stop, waitset_run,
    waitset_timer_cancel, waitset_timer_reset, waitset_timer_state,
};
