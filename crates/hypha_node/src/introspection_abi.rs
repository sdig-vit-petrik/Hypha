//! The introspection C ABI: configuration rows and the snapshot of one node.

use hypha_core::introspection::{
    ConfigRow, ConfigSource, CycleRow, EndpointRow, InputBindingRow, InputMode, InputRole,
    OutputFailure, Snapshot,
};
use hypha_msgs::hypha_msgs::{
    IntrospectedConfig, IntrospectedCycle, IntrospectedEndpoint, IntrospectedInput,
    NodeIntrospection,
};

use crate::config::HyphaConfig;
use crate::introspection::config_source;
use crate::node::HyphaNode;
use crate::status::Error;

/// Records the configuration row `key` of `node`, in its configuration and in its
/// introspection snapshot. `source` is a `HyphaConfigSource` value.
///
/// # Errors
/// `InvalidArgument` for an empty key or an unknown source.
pub fn node_record_config(
    node: &HyphaNode,
    key: &str,
    value: &str,
    default: Option<&str>,
    source: u32,
) -> Result<(), Error> {
    let row = config_row(key, value, default, source)?;
    node.parameters.record_introspection_row(row.clone());
    node.introspection.record_config(row);
    Ok(())
}

/// Records the configuration row `key` of a standalone `config`.
///
/// # Errors
/// `InvalidArgument` for an empty key or an unknown source.
pub fn config_record_config(
    config: &HyphaConfig,
    key: &str,
    value: &str,
    default: Option<&str>,
    source: u32,
) -> Result<(), Error> {
    let row = config_row(key, value, default, source)?;
    config.record_introspection_row(row);
    Ok(())
}

/// The introspection snapshot of `node` as the message `~/introspection` carries.
#[must_use]
pub fn node_introspect(node: &HyphaNode) -> NodeIntrospection {
    to_message(&node.introspection.snapshot())
}

fn config_row(
    key: &str,
    value: &str,
    default: Option<&str>,
    source: u32,
) -> Result<ConfigRow, Error> {
    if key.is_empty() {
        return Err(Error::InvalidArgument);
    }
    let source = config_source(source).ok_or(Error::InvalidArgument)?;
    Ok(ConfigRow {
        key: key.to_owned(),
        value: value.to_owned(),
        default: default.map(str::to_owned),
        source,
    })
}

fn to_message(snapshot: &Snapshot) -> NodeIntrospection {
    NodeIntrospection {
        node: snapshot.identity.node.clone(),
        domain: snapshot.identity.domain.clone(),
        pid: snapshot.identity.pid,
        created: snapshot.identity.created.clone(),
        hypha: snapshot.identity.hypha.clone(),
        configuration: snapshot.configuration.iter().map(config_message).collect(),
        subscribers: snapshot.subscribers.iter().map(endpoint_message).collect(),
        publishers: snapshot.publishers.iter().map(endpoint_message).collect(),
        cycles: snapshot.cycles.iter().map(cycle_message).collect(),
        input_bindings: snapshot.input_bindings.iter().map(input_message).collect(),
    }
}

fn config_message(row: &ConfigRow) -> IntrospectedConfig {
    let source = match row.source {
        ConfigSource::Parameter => IntrospectedConfig::SOURCE_PARAMETER,
        ConfigSource::Application => IntrospectedConfig::SOURCE_APPLICATION,
        ConfigSource::CommandLine => IntrospectedConfig::SOURCE_COMMAND_LINE,
        ConfigSource::Default => IntrospectedConfig::SOURCE_DEFAULT,
    };
    IntrospectedConfig {
        key: row.key.clone(),
        value: row.value.clone(),
        has_default: row.default.is_some(),
        default_value: row.default.clone().unwrap_or_default(),
        source,
    }
}

fn endpoint_message(row: &EndpointRow) -> IntrospectedEndpoint {
    let (output_cycle, output_failure) = match &row.output {
        None => (String::new(), IntrospectedEndpoint::OUTPUT_NONE),
        Some(output) => {
            let failure = match output.failure {
                OutputFailure::FailRun => IntrospectedEndpoint::OUTPUT_FAIL_RUN,
                OutputFailure::DropPoolExhausted => {
                    IntrospectedEndpoint::OUTPUT_DROP_POOL_EXHAUSTED
                }
            };
            (output.cycle.clone(), failure)
        }
    };
    IntrospectedEndpoint {
        id: row.id,
        topic: row.topic.clone(),
        data_type: row.data_type.clone(),
        queue_depth: u32::try_from(row.queue_depth).unwrap_or(u32::MAX),
        latched: row.latched,
        output_cycle,
        output_failure,
    }
}

fn cycle_message(row: &CycleRow) -> IntrospectedCycle {
    IntrospectedCycle {
        name: row.name.clone(),
        has_period: row.period_nanos.is_some(),
        period_nanos: row.period_nanos.unwrap_or(0),
        inputs: u32::try_from(row.inputs).unwrap_or(u32::MAX),
        outputs: u32::try_from(row.outputs).unwrap_or(u32::MAX),
        trigger: row.trigger.clone().unwrap_or_default(),
    }
}

fn input_message(row: &InputBindingRow) -> IntrospectedInput {
    let (mode, max_samples_per_iteration) = match row.mode {
        InputMode::Latest => (IntrospectedInput::MODE_LATEST, 0),
        InputMode::Queued { max_samples_per_iteration } => {
            let samples = u32::try_from(max_samples_per_iteration).unwrap_or(u32::MAX);
            (IntrospectedInput::MODE_QUEUED, samples)
        }
    };
    let supervision = row.supervision.unwrap_or(hypha_core::introspection::Supervision {
        silence_nanos: 0,
        stagnation_nanos: 0,
        ignored: false,
    });
    IntrospectedInput {
        cycle: row.cycle.clone(),
        index: row.index,
        topic: row.topic.clone(),
        subscriber_id: row.subscriber.unwrap_or(0),
        required: matches!(row.role, InputRole::Required),
        mode,
        max_samples_per_iteration,
        supervised: row.supervision.is_some(),
        silence_nanos: supervision.silence_nanos,
        stagnation_nanos: supervision.stagnation_nanos,
        ignored: supervision.ignored,
    }
}
