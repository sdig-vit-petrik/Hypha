//! The C ABI for hypha.
//!
//! A Rust panic ends the process: no entry point catches it. No Rust type crosses:
//! handles are opaque pointers. One create, one destroy. Callback samples live only
//! for the callback; owning receive keeps a transport sample until its explicit release.

pub mod clean;
pub mod clock;
pub mod config;
pub mod cycle;
pub mod domain;
pub mod introspection;
pub mod introspection_abi;
pub mod last_error;
pub mod loan;
pub mod log;
pub mod metrics;
pub mod node;
pub mod node_checks;
pub mod node_discovery;
pub mod node_health;
pub mod node_progress;
pub mod node_state;
pub mod param_topic;
pub mod parameter_constraints;
pub mod parameter_parse;
pub mod port_limits;
pub mod process_stop;
pub mod publisher;
pub mod qos;
pub mod replay_restart;
pub mod safe;
pub mod sample;
pub mod schema;
pub mod sdk;
pub mod service;
pub mod status;
pub mod subscriber;
mod topic_streams;
pub mod transforms;
pub mod waitset;
pub use waitset::{
    HYPHA_ATTACH_SUBSCRIPTION, HYPHA_ATTACH_TIMER, HyphaAttachment, HyphaAttachmentId,
    HyphaCallbackResult, HyphaContextRelease, HyphaDelivery, HyphaIntervalCallback,
    HyphaSubscriptionCallback, HyphaTimerState, HyphaWaitSet, HyphaWaitSetRunReport,
};

pub use config::HyphaConfig;
pub use cycle::{
    HYPHA_CYCLE_VERDICT_NEVER_ARRIVED, HYPHA_CYCLE_VERDICT_STALE, HYPHA_CYCLE_VERDICT_USABLE,
    HYPHA_DEFAULT_ARRIVALS_PER_STEP, HYPHA_FAULT_STAGNANT, HYPHA_FAULT_STALE,
    HyphaClockDriverOptions, HyphaCycleBindingHealth, HyphaCycleBindingOptions,
    HyphaCycleBindingReport, HyphaCycleBindingView, HyphaCycleEmission, HyphaCycleHealth,
    HyphaCycleHeldReport, HyphaCycleImage, HyphaCycleInputMode, HyphaCycleOutputFailure,
    HyphaCycleSampleView, HyphaCycleStep, HyphaCycleTime, HyphaInputSource,
    HyphaInputSupervisionRequest, HyphaInputSupervisionResult, HyphaSupervisionLimits,
    HyphaSupervisionReport,
};

pub use introspection::{
    HYPHA_CONFIG_SOURCE_APPLICATION, HYPHA_CONFIG_SOURCE_COMMAND_LINE, HYPHA_CONFIG_SOURCE_DEFAULT,
    HYPHA_CONFIG_SOURCE_PARAMETER,
};

pub use loan::HyphaLoan;
pub use log::{HyphaLogLevel, HyphaLogModule};
pub use metrics::HyphaMetricCallback;
pub use node::HyphaNode;
pub use node_discovery::{
    HyphaNodeCallback, HyphaNodeInfo, HyphaServicePortCallback, HyphaServicePortInfo,
};

pub use param_topic::{
    HYPHA_PORT_INPUT, HYPHA_PORT_OUTPUT, HYPHA_PORT_TOPIC_OPTIONAL, HYPHA_PORT_TOPIC_REQUIRED,
};
pub use parameter_constraints::HyphaNumberConstraint;
pub use parameter_parse::{
    HYPHA_CONFIG_VALUE_BOOL, HYPHA_CONFIG_VALUE_FLOAT, HYPHA_CONFIG_VALUE_INT,
    HYPHA_CONFIG_VALUE_TEXT, HyphaConfigValue, HyphaParamResult,
};
pub use port_limits::HyphaSupervisionRanges;
pub use publisher::{HyphaPublisher, HyphaPublisherOptions};
pub use qos::HyphaQos;
pub use sample::HyphaReceivedSample;
pub use status::HyphaStatus;
pub use subscriber::{
    HYPHA_DEFAULT_NO_PUBLISHER_WARNING_NANOS, HYPHA_NOT_CHECKED, HyphaSampleIdentity,
    HyphaSubscriber, HyphaSubscriberCounters, HyphaSubscriberOptions,
};
pub use transforms::{
    HYPHA_DEFAULT_TF_CACHE_NANOS, HYPHA_TF_AFTER_BUFFER, HYPHA_TF_BEFORE_BUFFER, HYPHA_TF_FOUND,
    HYPHA_TF_INVALID_FRAME, HYPHA_TF_NOT_CONNECTED, HYPHA_TF_REFUSED, HYPHA_TF_UNKNOWN_FRAME,
    HyphaSentTransform, HyphaTransform, buffer::HyphaTransformBuffer,
};

/// The guard of `mutex`, taken as it is when a holder panicked.
#[inline]
pub(crate) fn locked<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
