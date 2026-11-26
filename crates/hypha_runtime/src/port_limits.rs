//! The supervision and queued-work keys of an input port table.
//!
//!
//! [`supervise`] reads `ports.inputs.<name>`. Allowed keys are `topic`, `ignored`,
//! `silence_ms`, and `stagnation_ms` when a stagnation check is declared, and any other
//! key that the application has read before the call; nested keys count as one dotted
//! name. The check never marks an application key read.
//!
//! An enabled input that counts for health must set `silence_ms` unless tagged
//! `unsupervised`, which must not set it. `topic` is refused on a port that takes a
//! Source. Limits are integer milliseconds inside their range. `ignored` defaults to
//! false, and `true` on a port with no limit is refused.

use hypha_core::introspection::{ConfigRow, ConfigSource, IntrospectionRegistry};
use hypha_core::parameters::{ParamValue, Parameters};

/// The port entry being read, the registry that records its keys, and its table name.
#[derive(Clone, Copy)]
pub struct Port<'a> {
    /// The parameters that hold the table.
    pub parameters: &'a Parameters,
    /// The introspection registry that records every key read.
    pub introspection: &'a IntrospectionRegistry,
    /// The dotted name of the port table.
    pub entry: &'a str,
}

/// Silence and stagnation ranges of one supervised input, in nanoseconds.
///
/// A limit outside its range is refused, never clamped.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SupervisionRanges {
    /// Smallest accepted `silence_ms`, in nanoseconds.
    pub silence_min_nanos: i64,
    /// Largest accepted `silence_ms`, in nanoseconds.
    pub silence_max_nanos: i64,
    /// Smallest accepted `stagnation_ms`, in nanoseconds.
    pub stagnation_min_nanos: i64,
    /// Largest accepted `stagnation_ms`, in nanoseconds.
    pub stagnation_max_nanos: i64,
}

/// The silence and stagnation limits of one supervised input, in nanoseconds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SupervisionLimits {
    /// The most a sample may age after it was sent before the input is stale.
    pub silence_nanos: i64,
    /// Time without an alive change before the input is stagnant.
    pub stagnation_nanos: i64,
}

/// What the code declared about a port.
#[derive(Debug, Clone, Copy)]
pub struct Declared {
    /// The port is advisory: it does not count for health, so `silence_ms` is optional.
    pub advisory: bool,
    /// The port is tagged `unsupervised`: it runs without a silence limit.
    pub unsupervised: bool,
    /// Whether the port declares a stagnation check.
    pub stagnation: bool,
}

/// The detail of a required input that has no limit.
pub const MISSING_SILENCE: &str =
    "missing key 'silence_ms': a required input needs a limit, or the tag hypha::unsupervised";

/// The limits and the ignore flag that a table sets.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Supervision {
    /// The limits, zero when the check is not declared or not read.
    pub limits: SupervisionLimits,
    /// The ignore flag, false when absent.
    pub ignored: bool,
}

/// A refused port table. Each variant carries the detail text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortError {
    /// A key that the table must hold is absent.
    Missing(String),
    /// A key or the table is invalid: unknown, of another type, or out of range.
    Invalid(String),
}

impl PortError {
    /// The detail text of the refusal.
    #[must_use]
    pub fn message(&self) -> &str {
        match self {
            Self::Missing(text) | Self::Invalid(text) => text,
        }
    }
}

const NANOS_PER_MILLI: i64 = 1_000_000;

/// Check the keys of the port entry, then read its limits and flag when asked to.
///
/// A port that does not read limits returns zero limits with the flag false.
///
/// # Errors
/// Returns [`PortError::Invalid`] for an unknown key, a `topic` on a port that
/// takes no topic, or a key of another type or outside its range. Returns
/// [`PortError::Missing`] for a declared limit key that is absent.
pub fn supervise(
    port: Port<'_>,
    declared: Declared,
    takes_topic: bool,
    read_limits: bool,
    ranges: &SupervisionRanges,
) -> Result<Supervision, PortError> {
    let prefix = format!("{}.", port.entry);
    if let Some(key) = unknown_key(port, &prefix, declared.stagnation) {
        return Err(PortError::Invalid(format!("unknown key '{key}'")));
    }
    let topic_found = port.parameters.has(&format!("{}.topic", port.entry));
    if topic_found && !takes_topic {
        return Err(PortError::Invalid("a port that reads a Source takes no topic".to_owned()));
    }
    let silence_key = port.parameters.has(&format!("{}.silence_ms", port.entry));
    let both_tags = declared.unsupervised && declared.advisory;
    if both_tags {
        return Err(PortError::Invalid(
            "the tags unsupervised and advisory exclude each other".to_owned(),
        ));
    }
    let unsupervised_with_limit = declared.unsupervised && silence_key;
    if unsupervised_with_limit {
        return Err(PortError::Invalid(
            "key 'silence_ms' on a port tagged unsupervised".to_owned(),
        ));
    }
    let required = !declared.advisory && !declared.unsupervised;
    let wants_silence = required || silence_key;
    if !read_limits {
        return Ok(Supervision::default());
    }
    if !(wants_silence || declared.stagnation) {
        if read_ignored(port)? {
            return Err(PortError::Invalid(
                "key 'ignored' needs a silence_ms or stagnation_ms limit".to_owned(),
            ));
        }
        return Ok(Supervision::default());
    }
    let mut limits = SupervisionLimits::default();
    if wants_silence {
        limits.silence_nanos =
            read_limit(port, "silence_ms", ranges.silence_min_nanos, ranges.silence_max_nanos)
                .map_err(|error| match error {
                    PortError::Missing(_) => PortError::Missing(MISSING_SILENCE.to_owned()),
                    other @ PortError::Invalid(_) => other,
                })?;
    }
    if declared.stagnation {
        limits.stagnation_nanos = read_limit(
            port,
            "stagnation_ms",
            ranges.stagnation_min_nanos,
            ranges.stagnation_max_nanos,
        )?;
    }
    let ignored = read_ignored(port)?;
    Ok(Supervision { limits, ignored })
}

/// True when a port table may hold `key`: a framework key (`stagnation_ms` only with a
/// stagnation check), or any other key that the application has read already.
fn is_allowed_port_key(key: &str, check_stagnation: bool, read_by_application: bool) -> bool {
    match key {
        "topic" | "ignored" | "silence_ms" => true,
        "stagnation_ms" => check_stagnation,
        _ => read_by_application,
    }
}

/// Record a row of the port's table in the introspection registry.
fn record(
    port: Port<'_>,
    key: String,
    value: String,
    default: Option<String>,
    source: ConfigSource,
) {
    port.introspection.record_config(ConfigRow { key, value, default, source });
}

/// The first key of the port entry `prefix` that the checks do not allow, in sorted order.
/// Marks every key it passes as read, and leaves the key it refuses unread.
fn unknown_key(port: Port<'_>, prefix: &str, check_stagnation: bool) -> Option<String> {
    for key in port.parameters.names() {
        let Some(suffix) = key.strip_prefix(prefix) else {
            continue;
        };
        let read_by_application = port.parameters.is_read(key);
        if !is_allowed_port_key(suffix, check_stagnation, read_by_application) {
            return Some(suffix.to_owned());
        }
        let _ = port.parameters.value(key);
    }
    None
}

/// Read the millisecond key `key` of the port entry, as nanoseconds inside `min..=max`.
/// The value is recorded before its range is checked.
fn read_limit(port: Port<'_>, key: &str, min: i64, max: i64) -> Result<i64, PortError> {
    let full_name = format!("{}.{key}", port.entry);
    let millis = match port.parameters.value(&full_name) {
        None => return Err(PortError::Missing(format!("missing key '{key}'"))),
        Some(ParamValue::Int(millis)) => *millis,
        Some(_) => {
            return Err(PortError::Invalid(format!("key '{key}' has another type")));
        }
    };
    record(port, full_name, millis.to_string(), None, ConfigSource::Parameter);
    let nanos = millis.checked_mul(NANOS_PER_MILLI).filter(|nanos| (min..=max).contains(nanos));
    nanos.ok_or_else(|| PortError::Invalid(format!("key '{key}' is outside its range")))
}

/// Read the `ignored` flag of the port entry. Absent means false.
fn read_ignored(port: Port<'_>) -> Result<bool, PortError> {
    let full_name = format!("{}.ignored", port.entry);
    match port.parameters.value(&full_name) {
        None => {
            record(
                port,
                full_name,
                "false".to_owned(),
                Some("false".to_owned()),
                ConfigSource::Default,
            );
            Ok(false)
        }
        Some(ParamValue::Bool(flag)) => {
            let value = flag.to_string();
            record(port, full_name, value, Some("false".to_owned()), ConfigSource::Parameter);
            Ok(*flag)
        }
        Some(_) => Err(PortError::Invalid("key 'ignored' has another type".to_owned())),
    }
}
