//! The service policy of a topic: the limits of its iceoryx2 service.
//!
//! The creator stamps its policy on the service as [`QOS_ATTRIBUTE`]. An opener
//! that declares another policy is refused, so the limits do not depend on which
//! process started first; an undeclared opener adopts the stamp. A framework topic
//! ([`FRAMEWORK_TOPICS`]) always has the framework's fixed limits. A payload bound
//! belongs to each endpoint, not the stamp.

use hypha_core::port::{PayloadCapacity, PortError, QosPolicy, QosRequest, TopicSpec};
use iceoryx2::prelude::SemanticString as _;
use iceoryx2::service::attribute::{
    AttributeKey, AttributeSet, AttributeSpecifier, AttributeValue,
};

use crate::domain::MAX_BORROWED_SAMPLES;

/// The service attribute that names a data service's policy:
/// `name/publishers/subscribers/depth`.
pub const QOS_ATTRIBUTE: &str = "hypha.qos";

/// Publishers the `default` policy admits.
pub const DEFAULT_MAX_PUBLISHERS: usize = 8;

/// Subscribers the `default` policy admits.
pub const DEFAULT_MAX_SUBSCRIBERS: usize = 16;

/// The deepest queue the `default` policy admits, and the depth of a subscriber
/// that asks for none.
pub const DEFAULT_MAX_DEPTH: usize = 32;

/// Publishers the framework's limits admit: one per node of a domain.
pub const FRAMEWORK_MAX_PUBLISHERS: usize = 32;

/// Subscribers the framework's limits admit.
pub const FRAMEWORK_MAX_SUBSCRIBERS: usize = 32;

/// The deepest queue of the framework's limits.
pub const FRAMEWORK_MAX_DEPTH: usize = 32;

/// The topics the framework creates itself, always with the framework's limits.
pub const FRAMEWORK_TOPICS: [&str; 2] = ["/clock", "/clock_progress"];

/// The samples a publisher keeps for a late subscriber (the latch).
pub(crate) const HISTORY_SIZE: usize = 1;

/// Subscriber slots a service keeps for undeclared subscribers (monitoring tools,
/// bridges) on top of the policy's `max_subscribers`, so an observer cannot take a
/// declared subscriber's slot. Framework topics reserve nothing. Each slot costs
/// `max_depth + 2` pool samples on every publisher of the topic.
pub const RESERVED_OBSERVER_SUBSCRIBERS: usize = 3;

/// The start of an undeclared subscriber's port name: `hypha:observer:<node>`. A node
/// name cannot hold `:`, so no declared subscriber's port carries this prefix.
pub(crate) const OBSERVER_PORT_PREFIX: &str = "hypha:observer:";

/// iceoryx2 0.10's port name length limit, in bytes.
pub(crate) const MAX_PORT_NAME_BYTES: usize = 64;

/// The `default` policy: hypha's one built-in policy.
#[must_use]
pub fn default_policy() -> QosPolicy {
    QosPolicy::new("default", DEFAULT_MAX_PUBLISHERS, DEFAULT_MAX_SUBSCRIBERS, DEFAULT_MAX_DEPTH)
}

/// The limits of the framework's own topics.
#[must_use]
pub fn framework_policy() -> QosPolicy {
    QosPolicy::new(
        "framework",
        FRAMEWORK_MAX_PUBLISHERS,
        FRAMEWORK_MAX_SUBSCRIBERS,
        FRAMEWORK_MAX_DEPTH,
    )
}

/// True when `topic` is one of [`FRAMEWORK_TOPICS`].
#[must_use]
pub fn is_framework_topic(topic: &str) -> bool {
    FRAMEWORK_TOPICS.contains(&topic)
}

/// The policy an undeclared endpoint creates `topic`'s service with.
#[must_use]
pub fn undeclared_creation_policy(topic: &str) -> QosPolicy {
    let framework = is_framework_topic(topic);
    if framework { framework_policy() } else { default_policy() }
}

/// The subscriber ports a service of `topic` admits under `policy`, observer slots
/// included.
#[must_use]
pub fn subscriber_capacity(topic: &str, policy: &QosPolicy) -> usize {
    if is_framework_topic(topic) {
        return policy.max_subscribers;
    }
    policy.max_subscribers.saturating_add(RESERVED_OBSERVER_SUBSCRIBERS)
}

/// Samples in the pool of one publisher holding `loaned_samples` loans: every
/// subscriber's queue full, their borrowed samples, the latch and the loans.
#[must_use]
pub fn pool_samples(policy: &QosPolicy, subscribers: usize, loaned_samples: usize) -> usize {
    let queued_per_subscriber = policy.max_depth.saturating_add(MAX_BORROWED_SAMPLES);
    subscribers
        .saturating_mul(queued_per_subscriber)
        .saturating_add(HISTORY_SIZE)
        .saturating_add(loaned_samples)
}

/// The nodes one service admits: each endpoint may be a node of its own.
#[must_use]
pub(crate) fn max_nodes(policy: &QosPolicy, subscribers: usize) -> usize {
    policy.max_publishers.saturating_add(subscribers)
}

/// The subscriber slots one subscriber takes from its service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SubscriberSlots {
    /// A declared subscriber: up to `limit` declared subscribers, observers not counted.
    Declared { limit: usize },
    /// An undeclared subscriber: up to `limit` observers.
    Observer { limit: usize },
    /// A framework topic: its limits are fixed and not split between the two kinds.
    Unsplit,
}

/// The slots a declared subscriber of `topic` takes under the stamped `policy`.
#[must_use]
pub(crate) fn declared_slots(topic: &str, policy: &QosPolicy) -> SubscriberSlots {
    if is_framework_topic(topic) {
        return SubscriberSlots::Unsplit;
    }
    SubscriberSlots::Declared { limit: policy.max_subscribers }
}

/// The slots an undeclared subscriber of `topic` takes.
#[must_use]
pub(crate) fn observer_slots(topic: &str) -> SubscriberSlots {
    if is_framework_topic(topic) {
        return SubscriberSlots::Unsplit;
    }
    SubscriberSlots::Observer { limit: RESERVED_OBSERVER_SUBSCRIBERS }
}

/// True when a subscriber may create `topic`'s service: one that declares a policy,
/// or any on a framework topic. A monitoring tool started first must not lock a
/// topic's policy to `default`.
#[must_use]
pub(crate) fn subscriber_may_create(topic: &str, request: &QosRequest) -> bool {
    match request {
        QosRequest::Declared(_) => true,
        QosRequest::Undeclared => is_framework_topic(topic),
    }
}

/// The policy an endpoint creates a service with.
#[must_use]
pub(crate) fn creation_policy(topic: &str, request: &QosRequest) -> QosPolicy {
    if is_framework_topic(topic) {
        return framework_policy();
    }
    match request {
        QosRequest::Declared(policy) => policy.clone(),
        QosRequest::Undeclared => undeclared_creation_policy(topic),
    }
}

/// Refuses an endpoint whose declared policy is not the `stamped` one of its service.
///
/// # Errors
/// [`PortError::QosMismatch`] when the declared policy differs in its name or any limit.
pub(crate) fn check_stamped(
    topic: &str,
    request: &QosRequest,
    stamped: &QosPolicy,
) -> Result<(), PortError> {
    if is_framework_topic(topic) {
        return Ok(());
    }
    let QosRequest::Declared(declared) = request else {
        return Ok(());
    };
    if declared == stamped {
        return Ok(());
    }
    log::error!(target: "hypha/transport", "hypha_iceoryx2: topic '{topic}': the declared QoS policy {declared} differs from \
             the policy {stamped} its service was created with; this endpoint is refused");
    Err(PortError::QosMismatch)
}

/// Refuses a wildcard endpoint that declares a policy a matching service does not carry.
///
/// # Errors
/// [`PortError::QosMismatch`] when the stamp is missing, unreadable or another policy.
pub(crate) fn check_declared_service(
    topic: &str,
    request: &QosRequest,
    attributes: &AttributeSet,
) -> Result<(), PortError> {
    let QosRequest::Declared(_) = request else {
        return Ok(());
    };
    let stamped = policy_of_service(attributes).inspect_err(|_| {
        log::error!(target: "hypha/transport", "hypha_iceoryx2: topic '{topic}' names no QoS policy of this hypha build \
                 in its attribute {QOS_ATTRIBUTE}");
    })?;
    check_stamped(topic, request, &stamped)
}

/// Checks the policy an endpoint declares.
///
/// # Errors
/// [`PortError::InvalidArgument`] when the declared policy is outside the hard bounds.
pub(crate) fn check_request(topic: &str, request: &QosRequest) -> Result<(), PortError> {
    let QosRequest::Declared(policy) = request else {
        return Ok(());
    };
    policy.validate().map_err(|cause| {
        log::error!(target: "hypha/transport", "hypha_iceoryx2: topic '{topic}': {cause}");
        PortError::InvalidArgument
    })
}

pub(crate) fn corrupted() -> PortError {
    PortError::Backend(crate::service_open::BACKEND_SERVICE_CORRUPTED)
}

fn qos_attribute_key() -> Result<AttributeKey, PortError> {
    AttributeKey::try_from(QOS_ATTRIBUTE).map_err(|_| corrupted())
}

fn stamp_value(policy: &QosPolicy) -> Result<AttributeValue, PortError> {
    let text = format!(
        "{}/{}/{}/{}",
        policy.name, policy.max_publishers, policy.max_subscribers, policy.max_depth
    );
    AttributeValue::try_from(text.as_str()).map_err(|_| corrupted())
}

/// The attributes the endpoint that creates a data service stamps on it.
pub(crate) fn stamp_specifier(policy: &QosPolicy) -> Result<AttributeSpecifier, PortError> {
    let key = qos_attribute_key()?;
    let value = stamp_value(policy)?;
    AttributeSpecifier::new().define(&key, &value).map_err(|_| corrupted())
}

/// Reads the policy a stamp names, or `None` when this build does not write it.
fn parse_stamp(text: &str) -> Option<QosPolicy> {
    let mut fields = text.split('/');
    let name = fields.next()?;
    let max_publishers = fields.next()?.parse::<usize>().ok()?;
    let max_subscribers = fields.next()?.parse::<usize>().ok()?;
    let max_depth = fields.next()?.parse::<usize>().ok()?;
    let has_extra_field = fields.next().is_some();
    if has_extra_field {
        return None;
    }
    let policy = QosPolicy::new(name, max_publishers, max_subscribers, max_depth);
    policy.validate().ok()?;
    Some(policy)
}

/// The policy a data service was created with, read from its [`QOS_ATTRIBUTE`].
///
/// # Errors
/// [`PortError::QosMismatch`] when the attribute is missing, repeated, or names no
/// policy this build writes.
pub(crate) fn policy_of_service(attributes: &AttributeSet) -> Result<QosPolicy, PortError> {
    let text = attribute_text(attributes, QOS_ATTRIBUTE).ok_or(PortError::QosMismatch)?;
    parse_stamp(&text).ok_or(PortError::QosMismatch)
}

/// The one value of attribute `key`, as text; `None` when it is missing, repeated or not
/// UTF-8. Topic and service stamps are read through it.
pub(crate) fn attribute_text(attributes: &AttributeSet, key: &str) -> Option<String> {
    let key = AttributeKey::try_from(key).ok()?;
    if attributes.number_of_key_values(&key) != 1 {
        return None;
    }
    let value = attributes.key_value(&key, 0)?;
    core::str::from_utf8(value.as_bytes()).ok().map(str::to_owned)
}

/// The payload limit of a publisher of `spec`.
///
/// A fixed-size type is bounded to its exact encoded size, whatever was asked.
///
/// # Errors
/// [`PortError::InvalidTopic`] for a definition that does not match its MD5,
/// [`PortError::InvalidArgument`] for a limit of zero on a variable-size type.
pub(crate) fn resolve_payload(spec: &TopicSpec) -> Result<PayloadCapacity, PortError> {
    use hypha_msgdef::size::{EncodedSize, SizeError, verified_encoded_size};

    let fixed = if spec.definition.is_empty() {
        // Only an empty message has empty text, and its MD5 says so.
        match verified_encoded_size(&spec.data_type, &spec.md5, "") {
            Ok(EncodedSize::Fixed(size)) => Some(size),
            _ => None,
        }
    } else {
        match verified_encoded_size(&spec.data_type, &spec.md5, &spec.definition) {
            Ok(EncodedSize::Fixed(size)) => Some(size),
            Ok(EncodedSize::Variable) => None,
            Err(SizeError::Unreadable) => {
                log::error!(target: "hypha/transport", "hypha_iceoryx2: topic '{}': the definition of {} does not fully \
                         describe it", spec.name, spec.data_type);
                return Err(PortError::InvalidTopic);
            }
            Err(SizeError::Md5Mismatch { computed }) => {
                log::error!(target: "hypha/transport", "hypha_iceoryx2: topic '{}': the definition of {} hashes to {computed}, \
                         not to its MD5 {}", spec.name, spec.data_type, spec.md5);
                return Err(PortError::InvalidTopic);
            }
            Err(SizeError::NestingTooDeep { limit }) => {
                log::error!(target: "hypha/transport", "hypha_iceoryx2: topic '{}': the definition of {} nests deeper than {limit} levels",
                         spec.name, spec.data_type);
                return Err(PortError::InvalidTopic);
            }
            Err(SizeError::TooLarge) => {
                log::error!(target: "hypha/transport", "hypha_iceoryx2: topic '{}': the definition of {} expands to more than {} \
                         nested messages or {} bytes", spec.name, spec.data_type,
                         hypha_msgdef::spec::MAX_EXPANDED_MESSAGES, usize::MAX);
                return Err(PortError::InvalidTopic);
            }
        }
    };
    match (spec.payload_capacity, fixed) {
        // Zero is not a size iceoryx2 takes.
        (_, Some(size)) => Ok(PayloadCapacity::Bounded(size.max(1))),
        (PayloadCapacity::Bounded(0), None) => Err(PortError::InvalidArgument),
        (capacity, None) => Ok(capacity),
    }
}

#[cfg(test)]
#[allow(clippy::arithmetic_side_effects, clippy::expect_used)]
mod tests {
    use super::{
        FRAMEWORK_TOPICS, QOS_ATTRIBUTE, RESERVED_OBSERVER_SUBSCRIBERS, SubscriberSlots,
        check_stamped, creation_policy, declared_slots, default_policy, framework_policy,
        observer_slots, parse_stamp, policy_of_service, pool_samples, resolve_payload,
        stamp_specifier, subscriber_capacity, undeclared_creation_policy,
    };
    use hypha_core::port::{PayloadCapacity, PortError, QosPolicy, QosRequest, TopicSpec};
    use hypha_msgdef::md5::md5_from_definition;
    use iceoryx2::service::attribute::{AttributeKey, AttributeSpecifier, AttributeValue};

    #[test]
    fn the_built_in_policies_have_the_numbers_of_the_design() {
        assert_eq!(default_policy(), QosPolicy::new("default", 8, 16, 32));
        assert_eq!(framework_policy(), QosPolicy::new("framework", 32, 32, 32));
    }

    #[test]
    fn the_framework_topics_are_clock_and_clock_progress() {
        assert_eq!(FRAMEWORK_TOPICS, ["/clock", "/clock_progress"]);
    }

    #[test]
    fn a_framework_topic_gets_the_framework_limits_and_others_the_default() {
        for topic in ["/clock", "/clock_progress"] {
            assert_eq!(undeclared_creation_policy(topic), framework_policy(), "{topic}");
        }
        for topic in ["/tf", "/clock_in", "/clock_seek", "/node/state", "/clock/extra"] {
            assert_eq!(undeclared_creation_policy(topic), default_policy(), "{topic}");
        }
    }

    #[test]
    fn a_declared_policy_does_not_decide_or_refuse_a_framework_topic() {
        let camera = QosPolicy::new("camera", 4, 64, 256);
        let declared = QosRequest::Declared(camera.clone());
        for topic in ["/clock", "/clock_progress"] {
            assert_eq!(creation_policy(topic, &declared), framework_policy(), "{topic}");
            assert_eq!(check_stamped(topic, &declared, &framework_policy()), Ok(()), "{topic}");
        }
        assert_eq!(creation_policy("/image", &declared), camera);
        assert_eq!(creation_policy("/image", &QosRequest::Undeclared), default_policy());
        assert_eq!(
            check_stamped("/image", &declared, &default_policy()),
            Err(PortError::QosMismatch)
        );
        assert_eq!(check_stamped("/image", &QosRequest::Undeclared, &camera), Ok(()));
    }

    #[test]
    fn a_stamp_names_its_policy_and_a_foreign_one_is_refused() {
        let policy = QosPolicy::new("camera", 4, 64, 256);
        let specifier = stamp_specifier(&policy).expect("specifier");
        assert_eq!(policy_of_service(specifier.attributes()), Ok(policy));

        // A stamp of an older build, a missing stamp, or a second one is refused.
        let key = AttributeKey::try_from(QOS_ATTRIBUTE).expect("key");
        for old in ["generic/1", "a/b/c/d", "default/8/16/8/0", "/8/16/8", "default/8/x/8"] {
            let value = AttributeValue::try_from(old).expect("value");
            let stamped = AttributeSpecifier::new().define(&key, &value).expect("specifier");
            assert_eq!(
                policy_of_service(stamped.attributes()),
                Err(PortError::QosMismatch),
                "{old}"
            );
        }
        let none = AttributeSpecifier::new();
        assert_eq!(policy_of_service(none.attributes()), Err(PortError::QosMismatch));
    }

    #[test]
    fn a_stamp_outside_the_hard_bounds_is_refused() {
        assert_eq!(parse_stamp("camera/0/16/8"), None);
        assert_eq!(parse_stamp("camera/8/16/2000"), None);
        assert_eq!(parse_stamp("cam/era/8/16/8"), None);
        assert_eq!(parse_stamp("camera/8/16/8"), Some(QosPolicy::new("camera", 8, 16, 8)));
    }

    #[test]
    fn pools_follow_the_policy() {
        // Two loans held by the publisher, two samples borrowed by each subscriber.
        let default_subscribers = subscriber_capacity("/camera", &default_policy());
        assert_eq!(default_subscribers, 16 + RESERVED_OBSERVER_SUBSCRIBERS);
        assert_eq!(
            pool_samples(&default_policy(), default_subscribers, 2),
            (16 + RESERVED_OBSERVER_SUBSCRIBERS) * (32 + 2) + 1 + 2
        );
        let framework = subscriber_capacity("/clock", &framework_policy());
        assert_eq!(framework, 32, "a framework topic reserves no observer slots");
        assert_eq!(pool_samples(&framework_policy(), framework, 2), 32 * (32 + 2) + 1 + 2);
    }

    #[test]
    fn observers_and_declared_subscribers_split_the_slots_of_a_topic() {
        assert_eq!(
            observer_slots("/camera"),
            SubscriberSlots::Observer { limit: RESERVED_OBSERVER_SUBSCRIBERS }
        );
        assert_eq!(
            declared_slots("/camera", &default_policy()),
            SubscriberSlots::Declared { limit: 16 }
        );
        assert_eq!(observer_slots("/clock"), SubscriberSlots::Unsplit);
        assert_eq!(declared_slots("/clock", &framework_policy()), SubscriberSlots::Unsplit);
    }

    fn spec(data_type: &str, definition: &str, capacity: PayloadCapacity) -> TopicSpec {
        let md5 = md5_from_definition(data_type, definition).unwrap_or_default();
        TopicSpec::new("/t")
            .with_type(data_type)
            .with_md5(&md5)
            .with_definition(definition)
            .with_payload_capacity(capacity)
    }

    #[test]
    fn a_fixed_type_is_bounded_to_its_size_whatever_the_policy_says() {
        let clock =
            |capacity| resolve_payload(&spec("rosgraph_msgs/Clock", "time clock", capacity));
        assert_eq!(clock(PayloadCapacity::Growable), Ok(PayloadCapacity::Bounded(8)));
        assert_eq!(clock(PayloadCapacity::Bounded(4096)), Ok(PayloadCapacity::Bounded(8)));
        // A bound has no effect on a fixed-size type, also one below its size.
        assert_eq!(clock(PayloadCapacity::Bounded(7)), Ok(PayloadCapacity::Bounded(8)));
        assert_eq!(clock(PayloadCapacity::Bounded(0)), Ok(PayloadCapacity::Bounded(8)));
        // An empty message has empty text: it gets the smallest slot.
        let empty = spec("std_msgs/Empty", "", PayloadCapacity::Growable);
        assert_eq!(resolve_payload(&empty), Ok(PayloadCapacity::Bounded(1)));
        // A publisher that sends no definition gets no size from it: it stays growable.
        let mut bare = spec("rosgraph_msgs/Clock", "time clock", PayloadCapacity::Growable);
        bare.definition.clear();
        assert_eq!(resolve_payload(&bare), Ok(PayloadCapacity::Growable));
    }

    #[test]
    fn a_variable_type_is_growable_unless_it_names_a_bound() {
        let text = |capacity| resolve_payload(&spec("p/Text", "string data", capacity));
        assert_eq!(text(PayloadCapacity::Growable), Ok(PayloadCapacity::Growable));
        assert_eq!(text(PayloadCapacity::default()), Ok(PayloadCapacity::Growable));
        assert_eq!(text(PayloadCapacity::Bounded(300)), Ok(PayloadCapacity::Bounded(300)));
        assert_eq!(text(PayloadCapacity::Bounded(0)), Err(PortError::InvalidArgument));
    }

    #[test]
    fn a_definition_of_another_type_is_refused() {
        let mut wrong = spec("p/A", "int32 a", PayloadCapacity::Growable);
        wrong.definition = "int64 a".to_owned();
        assert_eq!(resolve_payload(&wrong), Err(PortError::InvalidTopic));
    }
}
