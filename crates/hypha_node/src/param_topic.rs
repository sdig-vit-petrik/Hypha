//! An endpoint's topic from its `[ports.inputs.<name>]` or `[ports.outputs.<name>]` table.

//!
//! `hypha_param_topic` reads `ports.inputs.<name>.topic` (`HYPHA_PORT_INPUT`) or
//! `ports.outputs.<name>.topic` (`HYPHA_PORT_OUTPUT`). `rule` decides what an unset or
//! empty topic means:
//!
//! | `rule` | No topic | `topic = ""` |
//! |---|---|---|
//! | `HYPHA_PORT_TOPIC_REQUIRED` | `NotAvailable` | `InvalidArgument`, logged |
//! | `HYPHA_PORT_TOPIC_OPTIONAL` | `Ok`, length 0 | `Ok`, length 0 |
//!
//! Length 0 tells the SDK to open a disabled endpoint. A `~/` name is returned as written.
//! Returns `TooLarge` when `out_capacity` is too small; `out_len` then holds the length.
//!

use hypha_core::introspection::{ConfigRow, ConfigSource};
use hypha_core::parameters::ParamValue;

use crate::node::HyphaNode;
use crate::status::Error;

/// The port is a subscribing endpoint: its topic is in the `[ports.inputs.<name>]` table.
pub const HYPHA_PORT_INPUT: u32 = 0;
/// The port is a publishing endpoint: its topic is in the `[ports.outputs.<name>]` table.
pub const HYPHA_PORT_OUTPUT: u32 = 1;

/// The topic must be set and non-empty: the endpoint cannot be switched off.
pub const HYPHA_PORT_TOPIC_REQUIRED: u32 = 0;
/// An unset topic or `topic = ""` gives a disabled endpoint.
pub const HYPHA_PORT_TOPIC_OPTIONAL: u32 = 1;

/// What an unset or empty topic means for one endpoint.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TopicRule {
    Required,
    Optional,
}

impl TopicRule {
    fn from_code(code: u32) -> Option<Self> {
        match code {
            HYPHA_PORT_TOPIC_REQUIRED => Some(Self::Required),
            HYPHA_PORT_TOPIC_OPTIONAL => Some(Self::Optional),
            _ => None,
        }
    }
}

/// The table of the parameter file that holds the topics of one port direction.
pub(crate) fn table_of(direction: u32) -> Option<&'static str> {
    match direction {
        HYPHA_PORT_INPUT => Some("inputs"),
        HYPHA_PORT_OUTPUT => Some("outputs"),
        _ => None,
    }
}

/// The topic and where it came from. `from_file` is false when no topic is set.
struct Resolution {
    key: String,
    topic: Option<String>,
    from_file: bool,
}

fn resolve(node: &HyphaNode, entry: &str, rule: TopicRule) -> Result<Resolution, Error> {
    let key = format!("{entry}.topic");
    if node.parameters.has(entry) {
        log::error!(target: "hypha", "hypha: the entry {entry} must be a table with a `topic` key");
        return Err(Error::InvalidArgument);
    }
    let configured = match node.parameters.value(&key) {
        Some(ParamValue::Text(text)) => Some(text.clone()),
        Some(_) => {
            log::error!(target: "hypha", "hypha: the parameter {key} must be a topic name in text");
            return Err(Error::InvalidArgument);
        }
        None => None,
    };
    let from_file = configured.is_some();
    let switched_off = configured.as_deref() == Some("");
    if switched_off {
        if rule == TopicRule::Required {
            log::error!(target: "hypha", "hypha: the port {entry} is required; {key} must not be empty");
            return Err(Error::InvalidArgument);
        }
        return Ok(Resolution { key, topic: None, from_file });
    }
    let Some(topic) = configured else {
        return if rule == TopicRule::Optional {
            Ok(Resolution { key, topic: None, from_file })
        } else {
            Err(Error::NotAvailable)
        };
    };
    if !hypha_core::port::is_valid_topic_name(&topic) {
        let too_long = topic.trim_start_matches('/').len() > hypha_core::port::MAX_TOPIC_NAME_BYTES;
        let reason = if too_long {
            format!(": longer than {} bytes after its '/'", hypha_core::port::MAX_TOPIC_NAME_BYTES)
        } else {
            String::new()
        };
        log::error!(target: "hypha", "hypha: the topic '{topic}' of {key} is not a valid topic name{reason}");
        return Err(Error::InvalidTopic);
    }
    Ok(Resolution { key, topic: Some(topic), from_file })
}

/// The topic of the port `name` of `node`, checked against `rule` and recorded as an
/// introspection row. An empty result is a disabled optional port.
///
/// # Errors
/// `InvalidArgument` for an empty name, an unknown direction or rule, or a malformed
/// entry; `NotAvailable` for a required port with no topic; `InvalidTopic` for an
/// invalid topic name.
pub fn param_topic(
    node: &HyphaNode,
    direction: u32,
    name: &str,
    rule: u32,
) -> Result<String, Error> {
    if name.is_empty() {
        return Err(Error::InvalidArgument);
    }
    let table = table_of(direction).ok_or(Error::InvalidArgument)?;
    let entry = format!("ports.{table}.{name}");
    let rule = TopicRule::from_code(rule).ok_or(Error::InvalidArgument)?;
    let resolution = resolve(node, &entry, rule)?;
    record_topic_row(node, &resolution);
    Ok(resolution.topic.unwrap_or_default())
}

/// Record the introspection row `<key>`: the topic, and source `parameter` when the
/// file set the topic, else `default`.
fn record_topic_row(node: &HyphaNode, resolution: &Resolution) {
    let source = if resolution.from_file { ConfigSource::Parameter } else { ConfigSource::Default };
    let value = resolution.topic.clone().unwrap_or_default();
    record_key_row(node, resolution.key.clone(), value, None, source);
}

/// Record an introspection row of a port key: its value, its default and its source.
pub(crate) fn record_key_row(
    node: &HyphaNode,
    key: String,
    value: String,
    default: Option<String>,
    source: ConfigSource,
) {
    node.introspection.record_config(ConfigRow { key, value, default, source });
}
