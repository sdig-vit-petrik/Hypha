//! Owning raw receive support.

use std::sync::Arc;

use crate::sdk::ffi;
use crate::sdk::options::SubscriberOptions;
use crate::sdk::status::{named_error, numbered_error};

/// A raw receive operation the runtime refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReceiveError {
    status: i32,
}

impl ReceiveError {
    /// The status code (the C ABI number) that refused the receive operation.
    #[must_use]
    pub const fn status(self) -> i32 {
        self.status
    }
}

impl std::fmt::Display for ReceiveError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "receive failed (status {})", self.status)
    }
}

impl std::error::Error for ReceiveError {}

/// Identity of a received frame. It borrows the topic from the sample.
pub type SampleIdentity<'sample> = ffi::FrameIdentity<'sample>;

/// An owned copy of a received raw payload and its identity.
#[derive(Debug, Clone)]
pub struct RawSample {
    /// Bus topic name.
    pub topic: String,
    /// Host monotonic send time, in nanoseconds.
    pub transport_sent_nanos: u64,
    /// Framework send time, in nanoseconds (see [`SampleIdentity`]).
    pub framework_sent_nanos: u64,
    /// 64-bit schema hash derived from the MD5.
    pub schema_hash: u64,
    /// The message payload.
    pub bytes: Vec<u8>,
}

/// One owning received sample, released when dropped. It may move to another thread.
pub struct ReceivedSample {
    sample: ffi::HyphaReceivedSample,
}

impl ReceivedSample {
    /// The message payload, or `None` when the sample is unavailable.
    #[must_use]
    pub fn bytes(&self) -> Option<&[u8]> {
        ffi::sample_payload(&self.sample).ok()
    }

    /// Identity from the frame and subscription, or `None` for a malformed frame.
    #[must_use]
    pub fn identity(&self) -> Option<SampleIdentity<'_>> {
        ffi::sample_identity(&self.sample).ok()
    }

    /// Copies this sample into an owned raw value.
    #[must_use]
    pub fn to_raw_sample(&self) -> Option<RawSample> {
        let identity = self.identity()?;
        let bytes = self.bytes()?.to_vec();
        Some(RawSample {
            topic: identity.topic.to_owned(),
            transport_sent_nanos: identity.transport_sent_nanos,
            framework_sent_nanos: identity.framework_sent_nanos,
            schema_hash: identity.schema_hash,
            bytes,
        })
    }
}

/// Raw subscriber that owns each received transport sample until it is dropped.
pub struct RawSubscriber {
    pub(crate) subscriber: Arc<ffi::HyphaSubscriber>,
}

impl RawSubscriber {
    /// Takes one sample, or `None` when the queue is empty.
    ///
    /// # Errors
    /// Returns an error when the ABI refuses the operation, including a full
    /// owning-sample budget or a mixed receive mode.
    pub fn try_receive(&mut self) -> Result<Option<ReceivedSample>, ReceiveError> {
        match ffi::subscriber_receive(&self.subscriber) {
            Ok(Some(sample)) => Ok(Some(ReceivedSample { sample })),
            Ok(None) => Ok(None),
            Err(error) => Err(ReceiveError { status: ffi::status_code(error) }),
        }
    }

    /// The topic this subscriber resolved at creation, such as `/<node>/x` for `~/x`.
    ///
    /// # Errors
    /// Never fails: the topic is fixed at creation.
    pub fn topic(&self) -> Result<String, String> {
        Ok(ffi::subscriber_topic(&self.subscriber).to_owned())
    }

    /// Every metric of this subscriber, in the order the runtime emits them.
    ///
    /// # Errors
    /// Never fails: a subscriber always has its metrics.
    pub fn metrics(&self) -> Result<Vec<(String, u64)>, String> {
        let subscriber = &self.subscriber;
        Ok(crate::sdk::metrics::collect(|emit| ffi::subscriber_metrics(subscriber, emit)))
    }

    /// The other schemas (MD5s) of this subscriber's topic that a live publisher sends, as
    /// its node's housekeeping last found them: `None` before the first check, which comes
    /// only while a run of the node's `WaitSet` goes on, and for a subscriber of no single
    /// stream. A fact about the topic, not a fault.
    #[must_use]
    pub fn other_schemas(&self) -> Option<u64> {
        other_schemas_of(&self.subscriber)
    }

    /// Counts the frames this subscriber lost before it read them: the sequence
    /// values skipped between accepted frames of one publisher. The count only grows.
    ///
    /// # Errors
    /// Never fails: the count is the subscriber's own.
    pub fn missing_sequences(&self) -> Result<u64, String> {
        Ok(self.subscriber.missing_sequences())
    }

    /// A handle to this subscriber's counters. It stays readable after the subscriber
    /// moved into a [`crate::sdk::WaitSet`] attachment, and while the subscriber lives
    /// it reads the same counts as [`RawSubscriber::missing_sequences`] and
    /// [`RawSubscriber::metrics`]. It keeps the subscriber's counts alive, not the
    /// subscription: after a detach it reads the final counts.
    #[must_use]
    pub fn counters(&self) -> SubscriberCounters {
        SubscriberCounters { subscriber: Arc::clone(&self.subscriber) }
    }

    /// Counts the publishers currently connected to this subscriber's topic.
    ///
    /// # Errors
    /// Returns an error when the transport cannot read the count.
    pub fn publisher_count(&self) -> Result<usize, String> {
        ffi::subscriber_publisher_count(&self.subscriber)
            .map_err(|error| named_error("cannot read the publisher count", error))
    }
}

/// The counters of one [`RawSubscriber`], readable from any thread while the
/// subscriber itself is owned elsewhere, such as by a [`crate::sdk::WaitSet`].
#[derive(Clone)]
pub struct SubscriberCounters {
    subscriber: Arc<ffi::HyphaSubscriber>,
}

impl SubscriberCounters {
    /// The subscriber's missing-sequence count, as [`RawSubscriber::missing_sequences`].
    #[must_use]
    pub fn missing_sequences(&self) -> u64 {
        self.subscriber.missing_sequences()
    }

    /// The other schemas of the subscriber's topic, as [`RawSubscriber::other_schemas`].
    #[must_use]
    pub fn other_schemas(&self) -> Option<u64> {
        other_schemas_of(&self.subscriber)
    }

    /// Every metric of the subscriber, as [`RawSubscriber::metrics`].
    #[must_use]
    pub fn metrics(&self) -> Vec<(String, u64)> {
        let subscriber = &self.subscriber;
        crate::sdk::metrics::collect(|emit| ffi::subscriber_metrics(subscriber, emit))
    }
}

/// The `other_schemas` counter of `subscriber`, `None` for "not checked".
fn other_schemas_of(subscriber: &ffi::HyphaSubscriber) -> Option<u64> {
    let other_schemas = ffi::subscriber_counters(subscriber).other_schemas;
    (other_schemas != crate::subscriber::HYPHA_NOT_CHECKED).then_some(other_schemas)
}

/// Open a raw subscriber without registering it with an executor.
pub(crate) fn subscribe_on(
    node: &ffi::HyphaNode,
    topic_pattern: &str,
    data_type: &str,
    expected_md5: &str,
    options: SubscriberOptions,
) -> Result<RawSubscriber, String> {
    let expected_md5 = if expected_md5.is_empty() { None } else { Some(expected_md5) };
    let identity = ffi::SubscriberIdentity { data_type, expected_md5, definition: "" };
    let settings = ffi::SubscriberSettings {
        depth: options.depth,
        no_publisher_warning_after_nanos: ffi::HYPHA_DEFAULT_NO_PUBLISHER_WARNING_NANOS,
        request: hypha_core::port::QosRequest::Undeclared,
    };
    ffi::subscribe_typed(node, topic_pattern, &identity, settings)
        .map(|subscriber| RawSubscriber { subscriber })
        .map_err(|error| numbered_error(format!("subscribe failed for '{topic_pattern}'"), error))
}

/// Open a raw subscriber of the one stream of `schema_hash`, without registering it with an
/// executor.
pub(crate) fn subscribe_stream_on(
    node: &ffi::HyphaNode,
    topic: &str,
    schema_hash: u64,
    options: SubscriberOptions,
) -> Result<RawSubscriber, String> {
    let settings = ffi::SubscriberSettings {
        depth: options.depth,
        no_publisher_warning_after_nanos: ffi::HYPHA_DEFAULT_NO_PUBLISHER_WARNING_NANOS,
        request: hypha_core::port::QosRequest::Undeclared,
    };
    ffi::subscribe_stream(node, topic, schema_hash, settings)
        .map(|subscriber| RawSubscriber { subscriber })
        .map_err(|error| numbered_error(format!("subscribe failed for '{topic}'"), error))
}
