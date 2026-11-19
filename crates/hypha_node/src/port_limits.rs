//! The ABI adapter of the supervision and queued-work keys of an input port table.
//!
//! The rules live in `hypha_runtime::port_limits`.

use hypha_runtime::port_limits::{self as runtime, Port, PortError};

use crate::cycle::HyphaSupervisionLimits;
use crate::node::HyphaNode;
use crate::status::HyphaStatus;

pub(crate) use hypha_runtime::port_limits::Declared;

/// Silence and stagnation ranges of one supervised input, in nanoseconds.
///
/// A limit outside its range is refused, never clamped.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HyphaSupervisionRanges {
    /// Smallest accepted `silence_ms`, in nanoseconds.
    pub silence_min_nanos: i64,
    /// Largest accepted `silence_ms`, in nanoseconds.
    pub silence_max_nanos: i64,
    /// Smallest accepted `stagnation_ms`, in nanoseconds.
    pub stagnation_min_nanos: i64,
    /// Largest accepted `stagnation_ms`, in nanoseconds.
    pub stagnation_max_nanos: i64,
}

/// The limits and the ignore flag of one supervised input.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Supervision {
    pub(crate) limits: HyphaSupervisionLimits,
    pub(crate) ignored: bool,
}

/// The status and the detail text of a refused port table.
pub(crate) type Refusal = (HyphaStatus, String);

impl From<HyphaSupervisionRanges> for runtime::SupervisionRanges {
    fn from(ranges: HyphaSupervisionRanges) -> Self {
        Self {
            silence_min_nanos: ranges.silence_min_nanos,
            silence_max_nanos: ranges.silence_max_nanos,
            stagnation_min_nanos: ranges.stagnation_min_nanos,
            stagnation_max_nanos: ranges.stagnation_max_nanos,
        }
    }
}

impl From<HyphaSupervisionLimits> for runtime::SupervisionLimits {
    fn from(limits: HyphaSupervisionLimits) -> Self {
        Self { silence_nanos: limits.silence_nanos, stagnation_nanos: limits.stagnation_nanos }
    }
}

impl From<runtime::SupervisionLimits> for HyphaSupervisionLimits {
    fn from(limits: runtime::SupervisionLimits) -> Self {
        Self { silence_nanos: limits.silence_nanos, stagnation_nanos: limits.stagnation_nanos }
    }
}

/// The status that a refused port table reports.
fn refusal(error: PortError) -> Refusal {
    let status = match error {
        PortError::Missing(_) => HyphaStatus::NotAvailable,
        PortError::Invalid(_) => HyphaStatus::InvalidArgument,
    };
    (status, error.message().to_owned())
}

/// Check the keys of the port entry `entry`, then read its limits and flag when asked to.
pub(crate) fn supervise(
    node: &HyphaNode,
    entry: &str,
    declared: Declared,
    takes_topic: bool,
    read_limits: bool,
    ranges: &HyphaSupervisionRanges,
) -> Result<Supervision, Refusal> {
    let port = Port { parameters: &node.parameters, introspection: &node.introspection, entry };
    let found = runtime::supervise(
        port,
        declared,
        takes_topic,
        read_limits,
        &runtime::SupervisionRanges::from(*ranges),
    )
    .map_err(refusal)?;
    Ok(Supervision { limits: found.limits.into(), ignored: found.ignored })
}
