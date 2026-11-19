//! What a caller asks a topic to promise.

use alloc::string::{String, ToString as _};

use crate::port::error::PortError;

/// Samples one publisher may hold on loan at a time.
///
/// The transport sets it on every publisher it creates.
pub const PUBLISHER_LOANED_SAMPLES: usize = 2;

/// The largest publisher count a declared policy may name.
pub const MAX_POLICY_PUBLISHERS: usize = 256;

/// The largest subscriber count a declared policy may name.
pub const MAX_POLICY_SUBSCRIBERS: usize = 256;

/// The deepest subscriber queue a declared policy may allow.
pub const MAX_POLICY_DEPTH: usize = 1024;

/// The longest name of a declared policy.
pub const MAX_POLICY_NAME_LENGTH: usize = 32;

/// How large one message of a publisher may be.
///
/// A fixed-size type is always bounded to its exact encoded size. Any other type is
/// growable unless it names a byte limit.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum PayloadCapacity {
    /// At most this many payload bytes; a larger loan is refused with
    /// [`PortError::TooLarge`]. The publisher's memory is fixed at creation.
    Bounded(usize),
    /// No limit: the publisher grows its memory when a message needs more. Meant for
    /// relays that forward messages of unknown size.
    #[default]
    Growable,
}

/// The service limits of a declared `QoS` policy: how many endpoints a topic's
/// service admits and how deep the queue of its subscribers may be. The payload bound
/// belongs to each endpoint ([`TopicSpec::payload_capacity`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QosPolicy {
    /// The policy's name, as the service stamp carries it.
    pub name: String,
    /// Publishers the service admits.
    pub max_publishers: usize,
    /// Subscribers the service admits.
    pub max_subscribers: usize,
    /// The deepest queue a subscriber may ask for.
    pub max_depth: usize,
}

impl QosPolicy {
    /// A policy with these limits. Nothing is checked until [`Self::validate`].
    #[must_use]
    pub fn new(
        name: &str,
        max_publishers: usize,
        max_subscribers: usize,
        max_depth: usize,
    ) -> Self {
        Self { name: name.to_string(), max_publishers, max_subscribers, max_depth }
    }

    /// Checks the name and every limit against the hard bounds of a declared policy.
    ///
    /// # Errors
    /// A message that names the first field out of bounds.
    pub fn validate(&self) -> Result<(), String> {
        if !is_policy_name(&self.name) {
            return Err(format!(
                "QoS policy name '{}' is not 1 to {MAX_POLICY_NAME_LENGTH} characters of letters, \
                 digits, '_', '-' or '.', starting with a letter or digit",
                self.name
            ));
        }
        check_policy_count("max_publishers", self.max_publishers, MAX_POLICY_PUBLISHERS)?;
        check_policy_count("max_subscribers", self.max_subscribers, MAX_POLICY_SUBSCRIBERS)?;
        check_policy_count("max_depth", self.max_depth, MAX_POLICY_DEPTH)
    }
}

impl core::fmt::Display for QosPolicy {
    fn fmt(&self, out: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            out,
            "'{}' (max_publishers {}, max_subscribers {}, max_depth {})",
            self.name, self.max_publishers, self.max_subscribers, self.max_depth
        )
    }
}

/// True for a policy name: 1 to [`MAX_POLICY_NAME_LENGTH`] characters, starting with
/// an ASCII letter or digit, then ASCII letters, digits, `_`, `-` or `.`.
fn is_policy_name(name: &str) -> bool {
    let starts_with_alphanumeric = name.chars().next().is_some_and(|c| c.is_ascii_alphanumeric());
    let within_length = !name.is_empty() && name.len() <= MAX_POLICY_NAME_LENGTH;
    let only_name_characters =
        name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    starts_with_alphanumeric && within_length && only_name_characters
}

/// Checks one count of a declared policy: at least one, and at most `max`.
fn check_policy_count(field: &str, value: usize, max: usize) -> Result<(), String> {
    if value == 0 {
        return Err(format!("QoS policy {field} must be at least 1"));
    }
    if value > max {
        return Err(format!("QoS policy {field} {value} exceeds the hard bound {max}"));
    }
    Ok(())
}

/// Which service policy an endpoint asks for.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum QosRequest {
    /// The endpoint declares no policy and adopts the topic's service. A publisher that
    /// finds none creates it with the framework's limits for a framework topic and with
    /// hypha's `default` policy otherwise. A subscriber creates a service only for a
    /// framework topic; elsewhere it is `NotAvailable` until the topic is opened.
    #[default]
    Undeclared,
    /// The endpoint declares this policy. The service must carry the same name and
    /// limits, or it is refused; a missing service is created with them. On a framework
    /// topic the policy does not apply and the endpoint is not refused.
    Declared(QosPolicy),
}

/// One topic, and what it promises.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopicSpec {
    /// A flat topic name, or a wildcard pattern for a subscription.
    pub name: String,
    /// Explicit subscriber buffer size; `None` adopts the service default.
    pub buffer_size: Option<usize>,
    /// Data type.
    pub data_type: String,
    /// Message MD5 hash.
    pub md5: String,
    /// Message definition.
    pub definition: String,
    /// Whether a subscriber that attaches after the last publish still receives the
    /// most recent samples (ROS latched topic, as `/tf_static`).
    pub latch: bool,
    /// Samples the publisher may hold on loan at once, reserved beyond the subscriber queues.
    pub loaned_samples: usize,
    /// The largest message a publisher of this topic sends.
    pub payload_capacity: PayloadCapacity,
    /// The first slot size, in bytes, of a growable publisher: a hint, not a bound.
    /// Zero takes the transport's default. A bounded publisher ignores it.
    pub initial_payload: usize,
    /// The service policy this endpoint declares, or none.
    pub qos: QosRequest,
}

impl TopicSpec {
    /// A topic with the defaults a periodic sensor or control value wants.
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            buffer_size: None,
            data_type: String::new(),
            md5: String::new(),
            definition: String::new(),
            latch: false,
            loaned_samples: PUBLISHER_LOANED_SAMPLES,
            payload_capacity: PayloadCapacity::default(),
            initial_payload: 0,
            qos: QosRequest::Undeclared,
        }
    }

    /// The same topic, with the service policy this endpoint declares.
    #[must_use]
    pub fn with_qos(mut self, qos: QosRequest) -> Self {
        self.qos = qos;
        self
    }

    /// The same topic, letting the publisher hold `loaned_samples` loans at once.
    #[must_use]
    pub fn with_loaned_samples(mut self, loaned_samples: usize) -> Self {
        self.loaned_samples = loaned_samples;
        self
    }

    /// The same topic, with the publisher's largest message set to `capacity`.
    #[must_use]
    pub fn with_payload_capacity(mut self, capacity: PayloadCapacity) -> Self {
        self.payload_capacity = capacity;
        self
    }

    /// The same topic, with `bytes` as the first slot size of a growable publisher.
    /// Zero takes the transport's default.
    #[must_use]
    pub fn with_initial_payload(mut self, bytes: usize) -> Self {
        self.initial_payload = bytes;
        self
    }

    /// The same topic, latched.
    #[must_use]
    pub fn latched(mut self) -> Self {
        self.latch = true;
        self
    }

    /// The same topic, keeping `depth` samples for a subscriber that is behind.
    #[must_use]
    pub fn keeping(mut self, depth: usize) -> Self {
        self.buffer_size = (depth > 0).then_some(depth);
        self
    }

    /// True when the name is usable.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        !self.name.is_empty()
    }

    /// True when the name is a canonical public topic.
    #[must_use]
    pub fn has_valid_public_name(&self) -> bool {
        let Some(bare) = self.name.strip_prefix('/') else {
            return false;
        };
        // Only the bare product name is reserved; applications publish below it.
        let is_reserved = self.name == "/hypha";
        let has_invalid_content = bare.is_empty()
            || is_reserved
            || bare.contains('*')
            || bare.chars().any(char::is_whitespace)
            || bare.chars().any(char::is_control);
        if has_invalid_content {
            return false;
        }
        bare.split('/').all(|segment| !segment.is_empty() && segment != "." && segment != "..")
    }

    /// True when the type name and MD5 form a valid schema identity.
    #[must_use]
    pub fn has_valid_schema_identity(&self) -> bool {
        !self.data_type.is_empty() && hypha_wire::is_valid_md5(&self.md5)
    }

    /// True when the specification carries a schema definition and a valid identity.
    #[must_use]
    pub fn has_schema_identity(&self) -> bool {
        self.has_valid_schema_identity() && !self.definition.is_empty()
    }

    /// Sets data type.
    #[must_use]
    pub fn with_type(mut self, dt: &str) -> Self {
        self.data_type = dt.to_string();
        self
    }

    /// Sets md5.
    #[must_use]
    pub fn with_md5(mut self, md5: &str) -> Self {
        self.md5 = md5.to_string();
        self
    }

    /// Sets definition.
    #[must_use]
    pub fn with_definition(mut self, def: &str) -> Self {
        self.definition = def.to_string();
        self
    }
}

/// Which schema record a lookup accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaSelector {
    /// Select the exact ROS message MD5.
    Md5(String),
    /// Select a compact frame schema hash: the first eight bytes of the MD5.
    Hash(u64),
}

/// Segments of a topic or pattern, ignoring empty ones.
fn segments(text: &str) -> impl Iterator<Item = &str> {
    text.split('/').filter(|segment| !segment.is_empty())
}

/// True when `topic` is matched by a subscription `pattern`.
///
/// `*` stands for exactly one segment, a trailing `**` for the remaining segments, and
/// `**` alone matches every topic. Empty segments are ignored.
#[must_use]
pub fn topic_matches_pattern(pattern: &str, topic: &str) -> bool {
    if pattern == topic || pattern == "**" {
        return true;
    }
    let pattern_length = segments(pattern).count();
    let topic_length = segments(topic).count();
    let takes_remaining_segments = segments(pattern).last() == Some("**");
    let prefix_length =
        if takes_remaining_segments { pattern_length.saturating_sub(1) } else { pattern_length };
    let length_fits = if takes_remaining_segments {
        topic_length >= prefix_length
    } else {
        topic_length == pattern_length
    };
    if !length_fits {
        return false;
    }
    segments(pattern)
        .take(prefix_length)
        .zip(segments(topic))
        .all(|(expected, actual)| expected == "*" || expected == actual)
}

/// True when `pattern` is a subscription pattern that [`topic_matches_pattern`]
/// can follow.
///
/// A `*` must be a whole segment, or `**` as the last segment. A pattern with no
/// segment, or any other `*`, is refused.
#[must_use]
pub fn is_valid_topic_pattern(pattern: &str) -> bool {
    let segment_count = segments(pattern).count();
    let has_segment = segment_count > 0;
    let last_index = segment_count.saturating_sub(1);
    let stars_are_well_placed = segments(pattern).enumerate().all(|(index, segment)| {
        let is_last = index == last_index;
        !segment.contains('*') || segment == "*" || (segment == "**" && is_last)
    });
    has_segment && stars_are_well_placed
}

/// True when `name` is a valid public topic name, or a valid `~/` name that
/// resolves to one under any node.
#[must_use]
pub fn is_valid_topic_name(name: &str) -> bool {
    resolve_node_name(name, "n")
        .is_ok_and(|resolved| TopicSpec::new(&resolved).has_valid_public_name())
}

/// The longest node name.
pub const MAX_NODE_NAME_LENGTH: usize = 64;

/// True when `name` is `[a-z][a-z0-9_]*` and at most `max_length` long.
#[must_use]
pub fn is_identifier(name: &str, max_length: usize) -> bool {
    let mut chars = name.chars();
    let starts_with_letter = chars.next().is_some_and(|first| first.is_ascii_lowercase());
    let rest_is_valid =
        chars.all(|next| next.is_ascii_lowercase() || next.is_ascii_digit() || next == '_');
    starts_with_letter && rest_is_valid && name.len() <= max_length
}

/// True when `name` is a valid node name.
#[must_use]
pub fn is_node_name(name: &str) -> bool {
    is_identifier(name, MAX_NODE_NAME_LENGTH)
}

/// The prefix of a CLI tool node name, as in `_hyphacli_top`.
///
/// Any number of processes can hold one CLI tool name at the same time, and any node
/// creation reclaims a dead CLI tool node.
pub const CLI_NODE_PREFIX: &str = "_hyphacli_";

/// True when `name` is a CLI tool node name: [`CLI_NODE_PREFIX`], then a node
/// name, at most [`MAX_NODE_NAME_LENGTH`] characters in all.
#[must_use]
pub fn is_cli_node_name(name: &str) -> bool {
    let Some(tool) = name.strip_prefix(CLI_NODE_PREFIX) else {
        return false;
    };
    let max_tool_length = MAX_NODE_NAME_LENGTH.saturating_sub(CLI_NODE_PREFIX.len());
    is_identifier(tool, max_tool_length)
}

/// Resolves a topic or service name that the node `node` owns.
///
/// `~/x` resolves to `/<node>/x`. Every other name is returned unchanged.
///
/// # Errors
/// Returns [`PortError::InvalidTopic`] for `~`, for `~/` with nothing after
/// it, and for a name such as `~x` that starts with `~` but not with `~/`.
pub fn resolve_node_name(name: &str, node: &str) -> Result<String, PortError> {
    let Some(rest) = name.strip_prefix('~') else {
        return Ok(name.to_string());
    };
    let Some(relative) = rest.strip_prefix('/') else {
        return Err(PortError::InvalidTopic);
    };
    if relative.is_empty() {
        return Err(PortError::InvalidTopic);
    }
    let mut resolved =
        String::with_capacity(node.len().saturating_add(rest.len()).saturating_add(1));
    resolved.push('/');
    resolved.push_str(node);
    resolved.push_str(rest);
    Ok(resolved)
}
