//! The node introspection: configuration rows and the rows of every endpoint
//! and cycle the node holds. A row handle removes its row when it is dropped.

use hypha_core::introspection::{ConfigSource, Identity, InputMode, OutputFailure};

use crate::cycle::{HyphaCycleBindingOptions, HyphaCycleInputMode, HyphaCycleOutputFailure};

/// A configuration value that the parameter file holds.
pub const HYPHA_CONFIG_SOURCE_PARAMETER: u32 = 0;
/// A configuration value that application code set.
pub const HYPHA_CONFIG_SOURCE_APPLICATION: u32 = 1;
/// A configuration value that the command line gives.
pub const HYPHA_CONFIG_SOURCE_COMMAND_LINE: u32 = 2;
/// A configuration value taken from its default, because the file did not set it.
pub const HYPHA_CONFIG_SOURCE_DEFAULT: u32 = 3;

pub(crate) use hypha_core::introspection::{IntrospectionRegistry, IntrospectionRow, Row};

/// The capture mode of an input binding, from its ABI options.
pub(crate) fn input_mode(options: &HyphaCycleBindingOptions) -> InputMode {
    if options.mode == HyphaCycleInputMode::Queued as u32 {
        return InputMode::Queued { max_samples_per_iteration: options.max_samples_per_iteration };
    }
    InputMode::Latest
}

/// The failure handling of a cycle output, from its ABI tag.
pub(crate) fn output_failure(policy_tag: HyphaCycleOutputFailure) -> OutputFailure {
    match policy_tag {
        HyphaCycleOutputFailure::FailRun => OutputFailure::FailRun,
        HyphaCycleOutputFailure::DropPoolExhausted => OutputFailure::DropPoolExhausted,
    }
}

/// The identity of a node: its name, domain, process and creation time (ISO 8601 UTC).
pub(crate) fn identity(name: &str, created_wall_nanos: i64) -> Identity {
    Identity {
        node: name.to_owned(),
        domain: hypha_iceoryx2::domain::domain().map_or_else(|_| String::new(), str::to_owned),
        pid: i32::try_from(std::process::id()).unwrap_or(i32::MAX),
        created: hypha_time::Time::from_nanos(created_wall_nanos).utc().to_string(),
        hypha: env!("CARGO_PKG_VERSION").to_owned(),
    }
}

pub(crate) fn config_source(source: u32) -> Option<ConfigSource> {
    match source {
        HYPHA_CONFIG_SOURCE_PARAMETER => Some(ConfigSource::Parameter),
        HYPHA_CONFIG_SOURCE_APPLICATION => Some(ConfigSource::Application),
        HYPHA_CONFIG_SOURCE_COMMAND_LINE => Some(ConfigSource::CommandLine),
        HYPHA_CONFIG_SOURCE_DEFAULT => Some(ConfigSource::Default),
        _ => None,
    }
}
