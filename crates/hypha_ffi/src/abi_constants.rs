//! The `#define` constants of the C header.
//!
//! cbindgen writes only this crate's constants and expands no macros, so each
//! `hypha_node` constant is repeated here as a literal, checked by `same_as_node!`.

/// Asserts at compile time that `$name` equals `hypha_node::$module::$name`.
macro_rules! same_as_node {
    ($name:ident, $($module:ident)::+) => {
        const _: () = assert!($name == hypha_node::$($module::)+$name);
    };
}

/// The SDK default of `cache_nanos`: dynamic transform history per frame.
pub const HYPHA_DEFAULT_TF_CACHE_NANOS: u64 = 10_000_000_000;
same_as_node!(HYPHA_DEFAULT_TF_CACHE_NANOS, transforms);

/// The transform was found and written.
pub const HYPHA_TF_FOUND: u32 = 0;
same_as_node!(HYPHA_TF_FOUND, transforms);

/// A frame name is empty or only `/`.
pub const HYPHA_TF_INVALID_FRAME: u32 = 1;
same_as_node!(HYPHA_TF_INVALID_FRAME, transforms);

/// No transform names one of the frames.
pub const HYPHA_TF_UNKNOWN_FRAME: u32 = 2;
same_as_node!(HYPHA_TF_UNKNOWN_FRAME, transforms);

/// Both frames are known, but no chain of transforms connects them.
pub const HYPHA_TF_NOT_CONNECTED: u32 = 3;
same_as_node!(HYPHA_TF_NOT_CONNECTED, transforms);

/// The time is older than a transform on the chain still holds.
pub const HYPHA_TF_BEFORE_BUFFER: u32 = 4;
same_as_node!(HYPHA_TF_BEFORE_BUFFER, transforms);

/// The time is newer than the newest sample of a transform on the chain.
pub const HYPHA_TF_AFTER_BUFFER: u32 = 5;
same_as_node!(HYPHA_TF_AFTER_BUFFER, transforms);

/// The lookup failed on a frame whose newest transform the buffer refused (not finite,
/// not a unit quaternion, a stamp that is no time, a second parent): its producer is
/// broken, not late. It replaces `UNKNOWN_FRAME`, `NOT_CONNECTED` and `AFTER_BUFFER`
/// while that frame's newest transform is a refused one.
pub const HYPHA_TF_REFUSED: u32 = 6;
same_as_node!(HYPHA_TF_REFUSED, transforms);

/// `hypha_cycle_image_clear`: `index` names an input binding.
pub const HYPHA_CYCLE_CLEAR_INPUT: u32 = 0;
same_as_node!(HYPHA_CYCLE_CLEAR_INPUT, cycle::types);

/// `hypha_cycle_image_clear`: `index` names a TF input (`hypha_cycle_bind_transforms`).
pub const HYPHA_CYCLE_CLEAR_TRANSFORMS: u32 = 1;
same_as_node!(HYPHA_CYCLE_CLEAR_TRANSFORMS, cycle::types);

/// Freshness verdict: no sample admitted since the binding was created.
pub const HYPHA_CYCLE_VERDICT_NEVER_ARRIVED: u32 = 0;
same_as_node!(HYPHA_CYCLE_VERDICT_NEVER_ARRIVED, cycle::types);

/// The newest admitted sample is within the silence limit (for a latest binding with
/// none retained, its held sample). A queued binding with no new sample is usable.
pub const HYPHA_CYCLE_VERDICT_USABLE: u32 = 1;
same_as_node!(HYPHA_CYCLE_VERDICT_USABLE, cycle::types);

/// The newest admitted sample is older than the silence limit or stamped after the
/// capture clock.
pub const HYPHA_CYCLE_VERDICT_STALE: u32 = 2;
same_as_node!(HYPHA_CYCLE_VERDICT_STALE, cycle::types);

/// Fault bit: the input is stale, its newest sample older than the silence limit.
pub const HYPHA_FAULT_STALE: u32 = 1;
same_as_node!(HYPHA_FAULT_STALE, cycle::types);

/// Fault bit: the alive counter unchanged within the stagnation limit.
pub const HYPHA_FAULT_STAGNANT: u32 = 2;
same_as_node!(HYPHA_FAULT_STAGNANT, cycle::types);

/// The SDK default of `HyphaCycleBindingOptions::max_samples_per_iteration`.
pub const HYPHA_DEFAULT_ARRIVALS_PER_STEP: usize = 8;
same_as_node!(HYPHA_DEFAULT_ARRIVALS_PER_STEP, cycle::types);

/// A configuration value that the parameter file holds.
pub const HYPHA_CONFIG_SOURCE_PARAMETER: u32 = 0;
same_as_node!(HYPHA_CONFIG_SOURCE_PARAMETER, introspection);

/// A configuration value that application code set.
pub const HYPHA_CONFIG_SOURCE_APPLICATION: u32 = 1;
same_as_node!(HYPHA_CONFIG_SOURCE_APPLICATION, introspection);

/// A configuration value that the command line gives.
pub const HYPHA_CONFIG_SOURCE_COMMAND_LINE: u32 = 2;
same_as_node!(HYPHA_CONFIG_SOURCE_COMMAND_LINE, introspection);

/// A configuration value taken from its default, because the file did not set it.
pub const HYPHA_CONFIG_SOURCE_DEFAULT: u32 = 3;
same_as_node!(HYPHA_CONFIG_SOURCE_DEFAULT, introspection);

/// The port is a subscribing endpoint: its topic is in the `[ports.inputs.<name>]` table.
pub const HYPHA_PORT_INPUT: u32 = 0;
same_as_node!(HYPHA_PORT_INPUT, param_topic);

/// The port is a publishing endpoint: its topic is in the `[ports.outputs.<name>]` table.
pub const HYPHA_PORT_OUTPUT: u32 = 1;
same_as_node!(HYPHA_PORT_OUTPUT, param_topic);

/// The topic must be set and non-empty: the endpoint cannot be switched off.
pub const HYPHA_PORT_TOPIC_REQUIRED: u32 = 0;
same_as_node!(HYPHA_PORT_TOPIC_REQUIRED, param_topic);

/// An unset topic or `topic = ""` gives a disabled endpoint.
pub const HYPHA_PORT_TOPIC_OPTIONAL: u32 = 1;
same_as_node!(HYPHA_PORT_TOPIC_OPTIONAL, param_topic);

/// `HyphaConfigValue::kind` of a boolean.
pub const HYPHA_CONFIG_VALUE_BOOL: u32 = 0;
same_as_node!(HYPHA_CONFIG_VALUE_BOOL, parameter_parse);

/// `HyphaConfigValue::kind` of a whole number.
pub const HYPHA_CONFIG_VALUE_INT: u32 = 1;
same_as_node!(HYPHA_CONFIG_VALUE_INT, parameter_parse);

/// `HyphaConfigValue::kind` of a number with a fraction.
pub const HYPHA_CONFIG_VALUE_FLOAT: u32 = 2;
same_as_node!(HYPHA_CONFIG_VALUE_FLOAT, parameter_parse);

/// `HyphaConfigValue::kind` of text.
pub const HYPHA_CONFIG_VALUE_TEXT: u32 = 3;
same_as_node!(HYPHA_CONFIG_VALUE_TEXT, parameter_parse);

/// The SDK default of `HyphaSubscriberOptions::no_publisher_warning_after_nanos`; the
/// ABI value zero disables the warning.
pub const HYPHA_DEFAULT_NO_PUBLISHER_WARNING_NANOS: u64 = 5_000_000_000;
same_as_node!(HYPHA_DEFAULT_NO_PUBLISHER_WARNING_NANOS, subscriber);

/// `HyphaSubscriberCounters.other_schemas` before the other streams of the subscriber's
/// topic were checked, and for a subscriber of no single stream.
pub const HYPHA_NOT_CHECKED: u64 = u64::MAX;
same_as_node!(HYPHA_NOT_CHECKED, subscriber);

/// What the callback asks the run loop to do next.
pub const HYPHA_ACTION_CONTINUE: u32 = 0;
same_as_node!(HYPHA_ACTION_CONTINUE, waitset);

/// Stop the run and report a requested stop.
pub const HYPHA_ACTION_STOP: u32 = 1;
same_as_node!(HYPHA_ACTION_STOP, waitset);

/// Stop the run and report a failure.
pub const HYPHA_ACTION_FAIL: u32 = 2;
same_as_node!(HYPHA_ACTION_FAIL, waitset);

/// Why a run returned.
pub const HYPHA_RUN_REASON_STOPPED: u32 = 0;
same_as_node!(HYPHA_RUN_REASON_STOPPED, waitset);

/// Building the native attachments failed. No callback ran.
pub const HYPHA_RUN_REASON_SETUP_FAILED: u32 = 1;
same_as_node!(HYPHA_RUN_REASON_SETUP_FAILED, waitset);

/// The run failed after it started.
pub const HYPHA_RUN_REASON_RUNTIME_FAILED: u32 = 2;
same_as_node!(HYPHA_RUN_REASON_RUNTIME_FAILED, waitset);

/// A bounded run reached its time limit. The application asked for no stop.
pub const HYPHA_RUN_REASON_DEADLINE: u32 = 3;
same_as_node!(HYPHA_RUN_REASON_DEADLINE, waitset);

/// A subscription that runs when its subscriber receives a sample.
pub const HYPHA_ATTACH_SUBSCRIPTION: u32 = 0;
same_as_node!(HYPHA_ATTACH_SUBSCRIPTION, waitset::registrations);

/// A periodic registration on framework time, owned by the `WaitSet`.
pub const HYPHA_ATTACH_TIMER: u32 = 2;
same_as_node!(HYPHA_ATTACH_TIMER, waitset::registrations);
