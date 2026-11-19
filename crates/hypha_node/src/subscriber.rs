//! Receiving: callback delivery remains ephemeral; owning receive keeps one
//! transport sample until its explicit release.

//!
//! `hypha_subscribe_typed` is the one way to create a subscriber; the caller releases it
//! with `hypha_subscriber_destroy`. A null `data_type` means no type, otherwise it must be
//! UTF-8 or the call returns `InvalidArgument`. `definition_len` 0 means no definition and
//! `definition` is then not read. A definition needs `data_type` and a valid
//! `expected_md5`, or the call returns `InvalidSchema`.

pub mod read;
pub use read::{subscriber_receive, subscriber_topic};

use std::os::raw::c_char;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

pub mod framework;
pub(crate) mod loan_budget;
pub(crate) use framework::{create_progress_subscriber, create_sim_clock_subscriber};

use crate::node::HyphaNode;
use hypha_core::frame_validation::{FrameRejection, FrameValidator};
use hypha_core::port::{QosRequest, TopicSpec};
use loan_budget::LoanBudget;

use crate::qos::HyphaQos;
use crate::status::Error;

type BackendSubscriber = hypha_iceoryx2::Iox2Subscriber;
pub(crate) type BackendSample = hypha_iceoryx2::Iox2Sample;

const MAX_BORROWED_SAMPLES: usize = hypha_iceoryx2::MAX_BORROWED_SAMPLES;
pub(crate) const MODE_UNSET: u8 = 0;
pub(crate) const MODE_RECEIVE: u8 = 1;
/// A `WaitSet` attachment dispatches this subscriber's samples.
pub(crate) const MODE_DISPATCH: u8 = 2;
/// A managed input source drains this subscriber into its history.
pub(crate) const MODE_SOURCE: u8 = 3;
/// The node drains this framework input (`/clock`, `/clock_progress`) itself.
pub(crate) const MODE_FRAMEWORK: u8 = 4;

/// The one no-publisher warning of a subscriber: due once no publisher has connected
/// for `after` since the subscriber was created. A publisher that connects ends the watch.
#[derive(Debug)]
struct NoPublisherWatch {
    after: Duration,
    since: std::time::Instant,
    /// Set once the watch can no longer warn: it warned, a publisher connected, or it
    /// is disabled.
    done: AtomicBool,
    /// Host time of the last publisher count; zero before the first.
    checked_nanos: AtomicU64,
}

impl NoPublisherWatch {
    /// A watch that warns after `after`, counted from `since`. Zero disables it.
    fn new(after: Duration, since: std::time::Instant) -> Self {
        Self {
            after,
            since,
            done: AtomicBool::new(after.is_zero()),
            checked_nanos: AtomicU64::new(0),
        }
    }

    /// How often the publisher count is read: once a second, or once per `after` when
    /// that is shorter.
    fn check_nanos(&self) -> u64 {
        u64::try_from(self.after.as_nanos()).unwrap_or(u64::MAX).clamp(1, 1_000_000_000)
    }

    /// True when the publisher count is due for a read at host time `now_nanos`. At most
    /// one caller gets `true` per interval.
    fn due(&self, now_nanos: impl FnOnce() -> u64) -> bool {
        if self.done.load(Ordering::Relaxed) {
            return false;
        }
        let now_nanos = now_nanos();
        let last = self.checked_nanos.load(Ordering::Acquire);
        if last != 0 && now_nanos.saturating_sub(last) < self.check_nanos() {
            return false;
        }
        self.checked_nanos
            .compare_exchange(last, now_nanos.max(1), Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// True when the warning is due now. A count above zero ends the watch without a warning.
    fn observe(&self, publisher_count: usize, now: std::time::Instant) -> bool {
        if self.done.load(Ordering::Relaxed) {
            return false;
        }
        if publisher_count > 0 {
            self.done.store(true, Ordering::Relaxed);
            return false;
        }
        if now.duration_since(self.since) < self.after {
            return false;
        }
        self.done.store(true, Ordering::Relaxed);
        true
    }
}

/// Rate limit of the refused-frame warning: one line per subscriber per second.
#[derive(Debug, Default)]
struct RejectionReport {
    /// Host time before which no line is written.
    next_nanos: AtomicU64,
    /// Refusals counted but not written since the last line.
    suppressed: AtomicU64,
}

impl RejectionReport {
    const INTERVAL_NANOS: u64 = 1_000_000_000;

    /// The count suppressed since the last line when a line is due at `now`.
    fn due(&self, now: u64) -> Option<u64> {
        if now < self.next_nanos.load(Ordering::Relaxed) {
            self.suppressed.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        self.next_nanos.store(now.saturating_add(Self::INTERVAL_NANOS), Ordering::Relaxed);
        Some(self.suppressed.swap(0, Ordering::Relaxed))
    }
}

/// Identity joined from the subscription topic and the frame header.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct HyphaSampleIdentity {
    /// Subscription topic bytes.
    pub topic: *const c_char,
    /// Topic byte count.
    pub topic_len: usize,
    /// Host monotonic time, in nanoseconds, at which the publisher sent the frame.
    pub transport_sent_nanos: u64,
    /// Framework time, in nanoseconds, at which the publisher sent the frame: simulated
    /// time on a sim-time node, and wall time otherwise.
    pub framework_sent_nanos: u64,
    /// 64-bit schema hash prefix from MD5.
    pub schema_hash: u64,
}

/// The identity of one received frame, borrowed from the sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameIdentity<'a> {
    /// The subscription topic.
    pub topic: &'a str,
    /// Host monotonic time, in nanoseconds, at which the publisher sent the frame.
    pub transport_sent_nanos: u64,
    /// Framework time, in nanoseconds, at which the publisher sent the frame.
    pub framework_sent_nanos: u64,
    /// 64-bit schema hash prefix from MD5.
    pub schema_hash: u64,
}

impl HyphaSampleIdentity {
    /// The C view of `identity`, which borrows its topic.
    #[must_use]
    pub fn from_view(identity: &FrameIdentity<'_>) -> Self {
        let topic = identity.topic.as_bytes();
        Self {
            topic: topic.as_ptr().cast::<c_char>(),
            topic_len: topic.len(),
            transport_sent_nanos: identity.transport_sent_nanos,
            framework_sent_nanos: identity.framework_sent_nanos,
            schema_hash: identity.schema_hash,
        }
    }
}

/// A receiving endpoint. Opaque to the caller.
///
/// It has one delivery style: owning receive (Rust SDK only) or `WaitSet` dispatch. The
/// first claim fixes the style and a detach does not release it; the other style then
/// returns `InvalidOperation` and consumes no sample. A notification is a wake hint, never data.
pub struct HyphaSubscriber {
    /// The address of the node that created this subscriber, compared and never
    /// dereferenced. Zero for a framework subscriber.
    pub(crate) owner_node: usize,
    pub(crate) topic: String,
    /// The message type it was opened with, empty for none.
    pub(crate) data_type: String,
    pub(crate) port: BackendSubscriber,
    budget: Arc<LoanBudget>,
    pub(crate) mode: AtomicU8,
    /// True while one `WaitSet` registration dispatches this subscriber.
    attached: AtomicBool,
    validator: FrameValidator,
    /// The one warning when this subscriber has no connected publisher.
    no_publisher_watch: NoPublisherWatch,
    /// Rate limit of the refused-frame warning.
    rejection_report: RejectionReport,
    /// The subscriber row of the node introspection, removed with this subscriber.
    pub(crate) introspection: crate::introspection::IntrospectionRow,
}

impl HyphaSubscriber {
    /// Warn once when no publisher has connected for the warning delay. Reads the
    /// publisher count at most once per second.
    pub(crate) fn check_no_publisher_warning(&self) {
        let watch = &self.no_publisher_watch;
        if !watch.due(host_now_nanos) {
            return;
        }
        let count = match self.port.publisher_count() {
            Ok(count) => count,
            Err(cause) => {
                // Read again at the next due check; the warning is not decided on a failed read.
                log::warn!(target: "hypha/subscriber", "hypha: cannot count the publishers of topic '{}': {cause:?}", self.topic);
                return;
            }
        };
        if watch.observe(count, std::time::Instant::now()) {
            let domain = match hypha_iceoryx2::domain::domain() {
                Ok("") => "<default>".to_owned(),
                Ok(domain) => domain.to_owned(),
                Err(cause) => format!("<unavailable: {cause:?}>"),
            };
            log::warn!(target: "hypha/subscriber", "hypha: no publisher connected to topic '{}' in domain '{}' after {} ms; if the publisher runs in another container, check that both /dev/shm and /tmp/iceoryx2 are shared",
                    self.topic,
                    domain,
                    watch.after.as_millis());
        }
    }

    /// The cumulative count of frames this subscriber lost before it read them. It only grows.
    #[must_use]
    pub fn missing_sequences(&self) -> u64 {
        self.validator.missing_sequences()
    }

    /// Take one sample and validate its header, payload, send time, age and sequence.
    ///
    /// # Errors
    /// [`ReceiveFailure::Transport`] when no sample could be taken, and
    /// [`ReceiveFailure::Refused`] when validation rejected one. A caller that drains in a
    /// loop must stop on the first and may continue on the second.
    pub(crate) fn receive_validated(&self) -> Result<Option<BackendSample>, ReceiveFailure> {
        // A zero cache reads the clock only after a frame with a nonzero send time arrives.
        self.receive_validated_with(&mut 0, |_| ())
            .map(|received| received.map(|(sample, ())| sample))
    }

    /// Process the authenticated frame while its sample is borrowed.
    pub(crate) fn receive_validated_with<T>(
        &self,
        now_nanos: &mut u64,
        consume: impl FnOnce(hypha_wire::frame::ParsedFrame<'_>) -> T,
    ) -> Result<Option<(BackendSample, T)>, ReceiveFailure> {
        let received = self.port.try_receive();
        match received {
            Ok(Some(sample)) => match validate_integrity(self, &sample, now_nanos) {
                Ok(frame) => {
                    let value = consume(frame);
                    Ok(Some((sample, value)))
                }
                Err(status) => Err(ReceiveFailure::Refused(status)),
            },
            Ok(None) => Ok(None),
            Err(cause) => Err(ReceiveFailure::Transport(Error::from(cause))),
        }
    }

    /// Fix the one style that drains this subscriber's queue.
    ///
    /// The first claim decides; a claim of another style returns `InvalidOperation`.
    /// `Ok` carries the style held before the claim.
    pub(crate) fn claim(&self, mode: u8) -> Result<u8, Error> {
        let claimed =
            self.mode.compare_exchange(MODE_UNSET, mode, Ordering::AcqRel, Ordering::Acquire);
        match claimed {
            Ok(previous) => Ok(previous),
            Err(held) if held == mode => Ok(held),
            Err(_) => Err(Error::InvalidOperation),
        }
    }

    /// Claim this subscriber for dispatch by one `WaitSet` registration.
    ///
    /// Another style returns `InvalidOperation`, a second registration `InvalidState`.
    /// `Ok` carries the style held before.
    pub(crate) fn attach_for_dispatch(&self) -> Result<u8, Error> {
        let previous = self.claim(MODE_DISPATCH)?;
        if self.attached.swap(true, Ordering::AcqRel) {
            return Err(Error::InvalidState);
        }
        Ok(previous)
    }

    /// Undo [`HyphaSubscriber::attach_for_dispatch`] for an attach that failed.
    pub(crate) fn undo_attach(&self, previous: u8) {
        self.attached.store(false, Ordering::Release);
        self.mode.store(previous, Ordering::Release);
    }

    /// Record that the registration that dispatched this subscriber is gone.
    /// The dispatch style stays claimed; another `WaitSet` may now attach the subscriber.
    pub(crate) fn end_dispatch(&self) {
        self.attached.store(false, Ordering::Release);
    }
}

/// Why [`HyphaSubscriber::receive_validated`] produced no usable sample.
///
/// A refused frame was taken from the queue; a transport failure took nothing, so
/// repeating it makes no progress and must end the drain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReceiveFailure {
    /// The transport could not produce a sample.
    Transport(Error),
    /// A sample arrived and validation rejected it.
    Refused(Error),
}

impl ReceiveFailure {
    /// The error a caller reports for this failure.
    pub(crate) fn error(self) -> Error {
        match self {
            Self::Transport(status) | Self::Refused(status) => status,
        }
    }
}

/// The SDK default of `HyphaSubscriberOptions::no_publisher_warning_after_nanos`: 5 s.
/// The ABI value zero disables the warning.
pub const HYPHA_DEFAULT_NO_PUBLISHER_WARNING_NANOS: u64 = 5_000_000_000;

/// Endpoint and resource options of a subscriber, passed to `hypha_subscribe_typed`.
///
/// `qos` declares the service policy the topic must carry, or none. Its payload bound
/// does not apply to a subscriber.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct HyphaSubscriberOptions {
    /// Number of pending samples retained for this subscriber. Zero selects the
    /// service's maximum depth. A depth above the service's `max_depth` is refused
    /// with `HYPHA_STATUS_RESOURCE_LIMIT`.
    pub depth: usize,
    /// Warn once after this many nanoseconds without a connected publisher. Zero disables it.
    /// A wildcard pattern never warns.
    pub no_publisher_warning_after_nanos: u64,
    /// The service policy this subscriber declares, or none.
    pub qos: HyphaQos,
}

impl Default for HyphaSubscriberOptions {
    fn default() -> Self {
        Self { depth: 1, no_publisher_warning_after_nanos: 0, qos: HyphaQos::default() }
    }
}

/// The message identity a subscriber names: type, accepted MD5 and definition.
#[derive(Debug, Clone, Copy)]
pub struct SubscriberIdentity<'a> {
    /// The message type, empty for none: the introspection row then shows `-`.
    pub data_type: &'a str,
    /// The MD5 the subscriber accepts, or `None` for any.
    pub expected_md5: Option<&'a str>,
    /// The message definition of `expected_md5`, empty for none.
    pub definition: &'a str,
}

/// The endpoint and resource options of a subscriber, as `HyphaSubscriberOptions` reads.
#[derive(Debug, Clone)]
pub struct SubscriberSettings {
    /// Pending samples retained. Zero selects the service's maximum depth.
    pub depth: usize,
    /// Warn once after this many nanoseconds without a publisher. Zero disables it.
    pub no_publisher_warning_after_nanos: u64,
    /// The service policy the subscriber declares, or none.
    pub request: QosRequest,
}

/// Subscribe on `node` to `topic` (a literal topic or a wildcard pattern). See
/// `hypha_subscribe_typed`.
///
/// # Errors
/// `InvalidSchema` for an invalid MD5 or a definition without type and MD5,
/// `InvalidTopic` for a refused name, and the transport's error otherwise.
pub fn subscribe_typed(
    node: &HyphaNode,
    topic: &str,
    identity: &SubscriberIdentity<'_>,
    settings: SubscriberSettings,
) -> Result<Arc<HyphaSubscriber>, Error> {
    let SubscriberIdentity { data_type, expected_md5, definition } = *identity;
    let expected_schema_hash = match expected_md5 {
        None => None,
        Some(md5) => Some(hypha_wire::schema_hash_from_md5(md5).ok_or(Error::InvalidSchema)?),
    };
    let resolved = node.resolve_name(topic)?;
    let name = resolved.as_str();
    let mut spec = TopicSpec::new(name).with_qos(settings.request);
    if settings.depth > 0 {
        spec = spec.keeping(settings.depth);
    }
    if !data_type.is_empty() {
        spec = spec.with_type(data_type);
    }
    if !definition.is_empty() {
        let Some(md5) = expected_md5.filter(|_| !data_type.is_empty()) else {
            return Err(Error::InvalidSchema);
        };
        spec = spec.with_type(data_type).with_md5(md5).with_definition(definition);
    }
    let port = node.transport.subscribe(&spec).map_err(Error::from)?;
    let depth = port.buffer_size();
    let budget = Arc::new(LoanBudget {
        budget: depth.min(MAX_BORROWED_SAMPLES),
        held: AtomicUsize::new(0),
        refused: AtomicU64::new(0),
    });
    Ok(Arc::new(HyphaSubscriber {
        owner_node: std::ptr::from_ref(node) as usize,
        topic: name.to_owned(),
        data_type: data_type.to_owned(),
        port,
        budget,
        mode: AtomicU8::new(MODE_UNSET),
        attached: AtomicBool::new(false),
        validator: FrameValidator::new(expected_schema_hash),
        no_publisher_watch: NoPublisherWatch::new(
            no_publisher_delay(name, settings.no_publisher_warning_after_nanos),
            std::time::Instant::now(),
        ),
        rejection_report: RejectionReport::default(),
        introspection: crate::introspection::IntrospectionRow::new(
            &node.introspection,
            crate::introspection::Row::Subscriber(hypha_core::introspection::EndpointRow {
                queue_depth: depth,
                ..hypha_core::introspection::EndpointRow::new(name, data_type)
            }),
        ),
    }))
}

/// The delay before the no-publisher warning of a subscription on `topic`.
///
/// A wildcard pattern with no match at creation never warns.
fn no_publisher_delay(topic: &str, warning_after_nanos: u64) -> Duration {
    let is_pattern = topic.contains('*');
    if is_pattern {
        return Duration::ZERO;
    }
    Duration::from_nanos(warning_after_nanos)
}

impl HyphaSubscriber {
    /// The frames this subscriber refused. A sequence gap is no refusal.
    pub(crate) fn rejected_frames(&self) -> u64 {
        let validation = self.validator.stats();
        validation
            .invalid_frame
            .wrapping_add(validation.corrupt_payload)
            .wrapping_add(validation.schema_mismatch)
            .wrapping_add(validation.invalid_timestamp)
    }
}

/// The counters of one subscriber, as `hypha_subscriber_counters` writes them: eight
/// `uint64_t`, 64 bytes, owned by the caller, with nothing to destroy. Every counter but
/// `loan_budget` and `loan_held` only grows, and wraps.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HyphaSubscriberCounters {
    /// `integrity.sequence_gap`: frames refused because their sequence was not newer
    /// than the last accepted one of their publisher. A lost frame is not one.
    pub sequence_gap: u64,
    /// `integrity.invalid_frame`: frames whose layout is not an hypha frame.
    pub invalid_frame: u64,
    /// `integrity.corrupt_payload`: frames whose CRC-32C does not match.
    pub corrupt_payload: u64,
    /// `integrity.schema_mismatch`: frames of another schema than the subscriber expects.
    pub schema_mismatch: u64,
    /// `integrity.invalid_timestamp`: frames sent later than the host clock reads.
    pub invalid_timestamp: u64,
    /// `loan.budget`: the owning samples the subscriber may hold at once.
    pub loan_budget: u64,
    /// `loan.held`: the owning samples it holds now.
    pub loan_held: u64,
    /// `loan.refused`: owning receives refused because the budget was full.
    pub loan_refused: u64,
}

/// The counters of `endpoint`. Each counter is read atomically, the eight not at one instant.
#[must_use]
pub fn subscriber_counters(endpoint: &HyphaSubscriber) -> HyphaSubscriberCounters {
    let validation = endpoint.validator.stats();
    HyphaSubscriberCounters {
        sequence_gap: validation.sequence_gap,
        invalid_frame: validation.invalid_frame,
        corrupt_payload: validation.corrupt_payload,
        schema_mismatch: validation.schema_mismatch,
        invalid_timestamp: validation.invalid_timestamp,
        // `usize` is at most 64 bits on every supported target.
        loan_budget: u64::try_from(endpoint.budget.budget).unwrap_or(0),
        loan_held: u64::try_from(endpoint.budget.held.load(Ordering::Acquire)).unwrap_or(0),
        loan_refused: endpoint.budget.refused.load(Ordering::Acquire),
    }
}

/// Emit the `integrity.*` and `loan.*` metrics of one subscriber, in that order.
pub(crate) fn emit_subscriber_metrics(endpoint: &HyphaSubscriber, emit: &mut dyn FnMut(&str, u64)) {
    let counters = subscriber_counters(endpoint);
    emit("integrity.invalid_frame", counters.invalid_frame);
    emit("integrity.corrupt_payload", counters.corrupt_payload);
    emit("integrity.schema_mismatch", counters.schema_mismatch);
    emit("integrity.sequence_gap", counters.sequence_gap);
    emit("integrity.invalid_timestamp", counters.invalid_timestamp);
    emit("loan.budget", counters.loan_budget);
    emit("loan.held", counters.loan_held);
    emit("loan.refused", counters.loan_refused);
}

pub(crate) fn host_now_nanos() -> u64 {
    u64::try_from(hypha_time::steady_now().nanos()).unwrap_or(0)
}

/// Validate one received sample. Every rejection is counted before it is returned and
/// logged at most once per second per subscriber.
fn validate_integrity<'a>(
    endpoint: &HyphaSubscriber,
    sample: &'a BackendSample,
    now_nanos: &mut u64,
) -> Result<hypha_wire::frame::ParsedFrame<'a>, Error> {
    let header = sample.header();
    endpoint
        .validator
        .validate_with_clock(&header, sample.payload(), now_nanos, host_now_nanos)
        .inspect_err(|rejection| {
            if let Some(more) = endpoint.rejection_report.due(host_now_nanos()) {
                log::warn!(target: "hypha/subscriber",
                    "hypha: topic '{}' refused a frame: {rejection}; {more} more refused since the last report",
                    endpoint.topic);
            }
        })
        .map_err(|rejection| match rejection {
            FrameRejection::InvalidFrame | FrameRejection::Future => Error::InvalidFrame,
            FrameRejection::CorruptPayload => Error::CorruptPayload,
            FrameRejection::SchemaMismatch => Error::InvalidSchema,
            FrameRejection::Sequence => Error::SequenceGap,
        })
}

/// The number of publishers currently connected to the subscriber's topic.
///
/// # Errors
/// The transport's error when the count cannot be read.
pub fn subscriber_publisher_count(subscriber: &HyphaSubscriber) -> Result<usize, Error> {
    subscriber.port.publisher_count().map_err(Error::from)
}

#[cfg(test)]
mod no_publisher_watch_tests {
    use super::NoPublisherWatch;
    use std::time::{Duration, Instant};

    #[test]
    fn warns_once_after_continuous_absence() {
        let start = Instant::now();
        let watch = NoPublisherWatch::new(Duration::from_secs(5), start);
        assert!(!watch.observe(0, start + Duration::from_secs(4)));
        assert!(!watch.observe(0, start + Duration::from_nanos(4_999_999_999)));
        assert!(watch.observe(0, start + Duration::from_secs(5)));
        assert!(!watch.due(|| 1), "a watch that warned needs no further count");
        assert!(!watch.observe(0, start + Duration::from_secs(20)));
    }

    #[test]
    fn a_connected_publisher_ends_the_watch_and_zero_disables_it() {
        let start = Instant::now();
        let watch = NoPublisherWatch::new(Duration::from_secs(5), start);
        assert!(!watch.observe(1, start + Duration::from_secs(1)));
        assert!(!watch.due(|| 1), "a topic that had a publisher is not misspelled");
        assert!(!watch.observe(0, start + Duration::from_secs(10)));

        let disabled = NoPublisherWatch::new(Duration::ZERO, start);
        assert!(!disabled.due(|| 1));
        assert!(!disabled.observe(0, start + Duration::from_secs(10)));
    }

    #[test]
    fn the_count_is_read_once_per_second_or_once_per_shorter_delay() {
        let second = 1_000_000_000;
        let watch = NoPublisherWatch::new(Duration::from_secs(5), Instant::now());
        assert!(watch.due(|| 7));
        assert!(!watch.due(|| 7 + second - 1));
        assert!(watch.due(|| 7 + second));

        let short = NoPublisherWatch::new(Duration::from_millis(100), Instant::now());
        assert!(short.due(|| 7));
        assert!(!short.due(|| 7 + 99_000_000));
        assert!(short.due(|| 7 + 100_000_000));
    }
}

#[cfg(test)]
mod rejection_report_tests {
    use super::RejectionReport;

    #[test]
    fn reports_the_first_refusal_then_once_per_interval_with_the_suppressed_count() {
        let report = RejectionReport::default();
        let second = RejectionReport::INTERVAL_NANOS;
        assert_eq!(report.due(5), Some(0));
        for _ in 0..99 {
            assert_eq!(report.due(5 + second - 1), None);
        }
        assert_eq!(report.due(5 + second), Some(99));
        assert_eq!(report.due(5 + second + 1), None);
        assert_eq!(report.due(5 + 3 * second), Some(1));
    }
}
