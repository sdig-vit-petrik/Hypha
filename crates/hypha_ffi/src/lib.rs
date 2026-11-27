//! The C ABI for hypha.
//!
//! Every function here is callable from C. The rules the boundary keeps:
//!
//! * A Rust panic ends the process: no entry point catches it. A foreign callback
//!   that fails by throwing is reported as [`HyphaStatus::Panic`].
//! * No Rust type crosses. Handles are opaque pointers; everything else is a
//!   primitive or a `#[repr(C)]` struct.
//! * One create, one destroy. A borrowed buffer is released by sending it or by
//!   dropping it, never by forgetting it.
//! * Callback samples live only for the callback.
//!
//! C and C++ consumers include the installed `hypha_ffi.h`. This crate exports the
//! shared library and is not a safe Rust client API.

mod abi;
mod abi_constants;
mod boundary;
mod callbacks;
mod clock;
mod config;
mod cycle;
mod introspection_abi;
mod last_error;
mod loan;
mod log;
mod metrics;
mod node;
mod node_discovery;
mod node_state;
mod param_topic;
mod parameter_constraints;
mod parameter_parse;
mod publisher;
mod schema;
mod status;
mod subscriber;
mod transform_buffer;
pub mod waitset;

pub use abi::hypha_abi_revision;
pub use clock::{
    hypha_clock_advance_sim_time, hypha_clock_disable_sim_time, hypha_clock_enable_sim_time,
    hypha_clock_is_sim_time, hypha_clock_now_nanos, hypha_clock_set_sim_time,
    hypha_clock_steady_now_nanos,
};
pub use config::{
    hypha_config_create, hypha_config_destroy, hypha_config_ignore, hypha_config_resolve_key,
    hypha_config_unread_keys, hypha_node_config,
};
pub use cycle::{HyphaCycle, cycle_timer_tick};
pub use cycle::{
    hypha_cycle_bind_input, hypha_cycle_bind_input_supervision, hypha_cycle_bind_output,
    hypha_cycle_bind_transforms, hypha_cycle_binding, hypha_cycle_commit, hypha_cycle_create,
    hypha_cycle_destroy, hypha_cycle_emission_commit, hypha_cycle_emission_reserve,
    hypha_cycle_expect_participant, hypha_cycle_health, hypha_cycle_image_alive,
    hypha_cycle_image_binding, hypha_cycle_image_held, hypha_cycle_image_lookup_transform,
    hypha_cycle_image_reject_sample, hypha_cycle_image_sample, hypha_cycle_image_set_ignored,
    hypha_cycle_set_class_based, hypha_cycle_set_clock_driver, hypha_cycle_set_host_clock,
    hypha_cycle_set_trigger, hypha_input_source_create, hypha_input_source_destroy,
};
pub use hypha_node::config::HyphaConfig;
pub use hypha_node::cycle::{
    HyphaClockDriverOptions, HyphaCycleBindingHealth, HyphaCycleBindingOptions,
    HyphaCycleBindingReport, HyphaCycleBindingView, HyphaCycleEmission, HyphaCycleHealth,
    HyphaCycleHeldReport, HyphaCycleImage, HyphaCycleInputMode, HyphaCycleOutputFailure,
    HyphaCycleSampleView, HyphaCycleStep, HyphaCycleTime, HyphaInputSource,
    HyphaInputSupervisionRequest, HyphaInputSupervisionResult, HyphaSupervisionLimits,
    HyphaSupervisionReport,
};

pub use hypha_node::loan::HyphaLoan;
pub use hypha_node::log::{HyphaLogLevel, HyphaLogModule};
pub use hypha_node::metrics::HyphaMetricCallback;
pub use hypha_node::node::HyphaNode;
pub use hypha_node::node_discovery::{
    HyphaNodeCallback, HyphaNodeInfo, HyphaServicePortCallback, HyphaServicePortInfo,
};
pub use introspection_abi::{
    hypha_config_record_config, hypha_node_introspect, hypha_node_record_config,
};
pub use last_error::hypha_last_error;
pub use loan::{
    hypha_loan_drop, hypha_loan_send, hypha_loan_shrink, hypha_publisher_loan_frame,
    hypha_publisher_reserve_frame,
};
pub use log::{hypha_log_enabled, hypha_log_module_register, hypha_log_write};
pub use metrics::hypha_node_metrics;
pub use node::{
    hypha_node_create, hypha_node_destroy, hypha_node_name, hypha_node_ok, hypha_node_shutdown,
};
pub use node_discovery::{
    hypha_list_nodes, hypha_list_service_ports, hypha_topic_validate_public_name,
};
pub use node_state::hypha_node_state;
pub use param_topic::hypha_param_topic;

pub use abi_constants::{
    HYPHA_ACTION_CONTINUE, HYPHA_ACTION_FAIL, HYPHA_ACTION_STOP, HYPHA_ATTACH_SUBSCRIPTION,
    HYPHA_ATTACH_TIMER, HYPHA_CONFIG_SOURCE_APPLICATION, HYPHA_CONFIG_SOURCE_COMMAND_LINE,
    HYPHA_CONFIG_SOURCE_DEFAULT, HYPHA_CONFIG_SOURCE_PARAMETER, HYPHA_CONFIG_VALUE_BOOL,
    HYPHA_CONFIG_VALUE_FLOAT, HYPHA_CONFIG_VALUE_INT, HYPHA_CONFIG_VALUE_TEXT,
    HYPHA_CYCLE_VERDICT_NEVER_ARRIVED, HYPHA_CYCLE_VERDICT_STALE, HYPHA_CYCLE_VERDICT_USABLE,
    HYPHA_DEFAULT_ARRIVALS_PER_STEP, HYPHA_DEFAULT_NO_PUBLISHER_WARNING_NANOS,
    HYPHA_DEFAULT_TF_CACHE_NANOS, HYPHA_FAULT_STAGNANT, HYPHA_FAULT_STALE, HYPHA_NOT_CHECKED,
    HYPHA_PORT_INPUT, HYPHA_PORT_OUTPUT, HYPHA_PORT_TOPIC_OPTIONAL, HYPHA_PORT_TOPIC_REQUIRED,
    HYPHA_RUN_REASON_DEADLINE, HYPHA_RUN_REASON_RUNTIME_FAILED, HYPHA_RUN_REASON_SETUP_FAILED,
    HYPHA_RUN_REASON_STOPPED, HYPHA_TF_AFTER_BUFFER, HYPHA_TF_BEFORE_BUFFER, HYPHA_TF_FOUND,
    HYPHA_TF_INVALID_FRAME, HYPHA_TF_NOT_CONNECTED, HYPHA_TF_REFUSED, HYPHA_TF_UNKNOWN_FRAME,
};
pub use hypha_node::parameter_constraints::HyphaNumberConstraint;
pub use hypha_node::parameter_parse::{HyphaConfigValue, HyphaParamResult};
pub use hypha_node::port_limits::HyphaSupervisionRanges;
pub use hypha_node::publisher::{HyphaPublisher, HyphaPublisherOptions};
pub use hypha_node::qos::HyphaQos;
pub use hypha_node::status::HyphaStatus;
pub use hypha_node::subscriber::{
    HyphaSampleIdentity, HyphaSubscriber, HyphaSubscriberCounters, HyphaSubscriberOptions,
};
pub use hypha_node::transforms::buffer::HyphaTransformBuffer;
pub use hypha_node::transforms::{HyphaSentTransform, HyphaTransform};
pub use parameter_constraints::{hypha_param_validate_nonempty, hypha_param_validate_number};
pub use parameter_parse::{hypha_config_list_len, hypha_config_list_value, hypha_config_value};
pub use publisher::{
    hypha_publish_with_qos, hypha_publisher_destroy, hypha_publisher_min_subscriber_buffer,
    hypha_publisher_subscriber_count,
};
pub use schema::hypha_schema_lookup;
pub use subscriber::{
    hypha_subscribe_typed, hypha_subscriber_counters, hypha_subscriber_destroy,
    hypha_subscriber_missing_sequences, hypha_subscriber_publisher_count, hypha_subscriber_topic,
};
pub use transform_buffer::{
    hypha_transform_buffer_create, hypha_transform_buffer_destroy, hypha_transform_buffer_lookup,
    hypha_transform_check,
};
pub use waitset::{
    HyphaAttachment, HyphaAttachmentId, HyphaCallbackResult, HyphaContextRelease, HyphaDelivery,
    HyphaIntervalCallback, HyphaSubscriptionCallback, HyphaTimerState, HyphaWaitSet,
    HyphaWaitSetRunReport, hypha_node_default_waitset, hypha_waitset_attach, hypha_waitset_detach,
    hypha_waitset_request_failure, hypha_waitset_request_stop, hypha_waitset_run,
    hypha_waitset_timer_cancel, hypha_waitset_timer_reset, hypha_waitset_timer_state,
};
