//! The supervision and queued-work keys of an input port table.
//!
//!
//! [`supervise`] reads `ports.inputs.<name>`. Allowed keys are `topic`, `ignored`,
//! `silence_ms`, and `stagnation_ms` when a stagnation check is declared, and any other
//! key that the application has read before the call; nested keys count as one dotted
//! name. The check never marks an application key read.
//!
//! An enabled input, advisory or not, must set `silence_ms` unless tagged `unsupervised`,
//! which must not set it: an input has no time limit only when it says so (#238).
//! `silence_ms = 0` is an input with no time limit (a ROS timeout of 0.0), as the tag is.
//! `topic` is refused on a port that takes a Source. Limits are integer milliseconds
//! inside their range. `ignored` defaults to false, and `true` on a port with no limit is
//! refused.

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

/// What the code declared about a port. Whether the port is advisory plays no part: the
/// limit is one question, health another.
#[derive(Debug, Clone, Copy)]
pub struct Declared {
    /// The port is tagged `unsupervised`: it runs without a silence limit.
    pub unsupervised: bool,
    /// Whether the port declares a stagnation check.
    pub stagnation: bool,
}

/// The detail of an enabled input whose table states no limit.
pub const MISSING_SILENCE: &str = "missing key 'silence_ms': an input needs a limit, 0 for none, \
     or the tag hypha::unsupervised";

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
    let unsupervised_with_limit = declared.unsupervised && silence_key;
    if unsupervised_with_limit {
        return Err(PortError::Invalid(
            "key 'silence_ms' on a port tagged unsupervised".to_owned(),
        ));
    }
    if !read_limits {
        return Ok(Supervision::default());
    }
    let mut limits = SupervisionLimits::default();
    if !declared.unsupervised {
        limits.silence_nanos = read_silence(port, ranges).map_err(|error| match error {
            PortError::Missing(_) => PortError::Missing(MISSING_SILENCE.to_owned()),
            other @ PortError::Invalid(_) => other,
        })?;
    }
    if limits.silence_nanos == 0 && !declared.stagnation {
        if read_ignored(port)? {
            return Err(PortError::Invalid(
                "key 'ignored' needs a silence_ms or stagnation_ms limit".to_owned(),
            ));
        }
        return Ok(Supervision::default());
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
fn read_limit(port: Port<'_>, key: &str, min: i64, max: i64) -> Result<i64, PortError> {
    let millis = read_millis(port, key)?;
    within(key, millis, min, max)
}

/// Read `silence_ms` of the port entry: zero is an input with no time limit, any other
/// value nanoseconds inside the silence range.
fn read_silence(port: Port<'_>, ranges: &SupervisionRanges) -> Result<i64, PortError> {
    let millis = read_millis(port, "silence_ms")?;
    if millis == 0 {
        return Ok(0);
    }
    within("silence_ms", millis, ranges.silence_min_nanos, ranges.silence_max_nanos)
}

/// The integer millisecond key `key` of the port entry, recorded as it was read.
fn read_millis(port: Port<'_>, key: &str) -> Result<i64, PortError> {
    let full_name = format!("{}.{key}", port.entry);
    let millis = match port.parameters.value(&full_name) {
        None => return Err(PortError::Missing(format!("missing key '{key}'"))),
        Some(ParamValue::Int(millis)) => *millis,
        Some(_) => {
            return Err(PortError::Invalid(format!("key '{key}' has another type")));
        }
    };
    record(port, full_name, millis.to_string(), None, ConfigSource::Parameter);
    Ok(millis)
}

/// `millis` of key `key` as nanoseconds inside `min..=max`, refused, never clamped, outside.
fn within(key: &str, millis: i64, min: i64, max: i64) -> Result<i64, PortError> {
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
