//! Node introspection: the facts only the engine knows about one node. This crate
//! renders nothing.

mod registry;

pub use registry::{IntrospectionRegistry, IntrospectionRow, Row};

/// What identifies the node.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Identity {
    /// The node name.
    pub node: String,
    /// The domain, or empty when the node has none.
    pub domain: String,
    /// The process ID.
    pub pid: i32,
    /// The wall-clock creation time, in ISO 8601 UTC.
    pub created: String,
    /// The version of the hypha framework.
    pub hypha: String,
}

/// Where a configuration value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSource {
    /// Read by hypha from the parameter file.
    Parameter,
    /// Set by application code, such as a policy the code chose.
    Application,
    /// Given on the command line.
    CommandLine,
    /// Taken from its default, because the source did not set it.
    Default,
    /// Set on purpose and not read; the row's value is the reason.
    Ignored,
}

/// One configuration value. `default` is `None` when no default exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigRow {
    /// The configuration key.
    pub key: String,
    /// The value in use, as text.
    pub value: String,
    /// The default, as text, or `None` when no default exists.
    pub default: Option<String>,
    /// Where the value came from.
    pub source: ConfigSource,
}

/// How an input binding captures its samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    /// The newest sample only.
    Latest,
    /// A bounded queue, examined a few samples per iteration.
    Queued {
        /// Queued samples examined per iteration; older pending samples are dropped.
        max_samples_per_iteration: usize,
    },
}

/// What a cycle output does when its publisher refuses a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFailure {
    /// Fail the run on any refusal.
    FailRun,
    /// Drop and count pool exhaustion; fail on every other refusal.
    DropPoolExhausted,
}

/// The cycle output that owns a publisher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CycleOutputRow {
    /// The cycle name.
    pub cycle: String,
    /// What the output does when the publisher refuses a message.
    pub failure: OutputFailure,
}

/// One topic endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointRow {
    /// The id of this row while its endpoint lives. Never 0.
    pub id: u64,
    /// The topic name.
    pub topic: String,
    /// The message type, such as `std_msgs/Empty`.
    pub data_type: String,
    /// The receive queue depth of a subscriber; 0 otherwise.
    pub queue_depth: usize,
    /// A publisher that latches its last message.
    pub latched: bool,
    /// The cycle output that owns this publisher, or `None`.
    pub output: Option<CycleOutputRow>,
}

impl EndpointRow {
    /// A row for `topic` of type `data_type` with no other facts set.
    #[must_use]
    pub fn new(topic: impl Into<String>, data_type: impl Into<String>) -> Self {
        Self {
            id: 0,
            topic: topic.into(),
            data_type: data_type.into(),
            queue_depth: 0,
            latched: false,
            output: None,
        }
    }
}

/// One cycle. `period_nanos` is `None` when the period is unknown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CycleRow {
    /// The cycle name.
    pub name: String,
    /// The cycle period in nanoseconds.
    pub period_nanos: Option<u64>,
    /// The number of input bindings.
    pub inputs: usize,
    /// The number of output bindings.
    pub outputs: usize,
    /// The topics a triggered cycle runs on, in port order; empty for a rate cycle.
    pub triggers: Vec<String>,
}

/// Required or optional role of one cycle input binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputRole {
    /// Node health judges this binding.
    Required,
    /// Node health excludes this binding.
    Optional,
}

/// The silence and stagnation supervision of one input binding. A zero limit
/// means that check is off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Supervision {
    /// The longest time without a sample, in nanoseconds.
    pub silence_nanos: u64,
    /// The longest time without a changed sample, in nanoseconds.
    pub stagnation_nanos: u64,
    /// The application currently ignores the faults of this binding.
    pub ignored: bool,
}

/// One cycle input binding, including bindings that have no live source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputBindingRow {
    /// The cycle name.
    pub cycle: String,
    /// The input's declaration-order index.
    pub index: u32,
    /// The topic name, or empty when the input is unwired.
    pub topic: String,
    /// The id of the subscriber row this binding reads, or `None` when unwired.
    pub subscriber: Option<u64>,
    /// The role of this binding in node health.
    pub role: InputRole,
    /// How the binding captures its samples.
    pub mode: InputMode,
    /// The supervision of this binding, or `None` when it has none.
    pub supervision: Option<Supervision>,
}

/// The rows of one node, in registration order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    /// The identity block.
    pub identity: Identity,
    /// The configuration rows.
    pub configuration: Vec<ConfigRow>,
    /// The subscriber rows.
    pub subscribers: Vec<EndpointRow>,
    /// The publisher rows.
    pub publishers: Vec<EndpointRow>,
    /// The cycle rows.
    pub cycles: Vec<CycleRow>,
    /// The input binding rows.
    pub input_bindings: Vec<InputBindingRow>,
}
