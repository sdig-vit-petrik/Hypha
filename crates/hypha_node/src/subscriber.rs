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

/// The no-publisher warning of a subscriber. Once its stream has had no publisher for
/// `after`, it warns, naming the other schemas of its topic that are published; it warns
/// again when those change while its stream still has none, at most once per
/// [`NoPublisherWatch::QUIET`]. A publisher of its own stream re-arms it: a later absence
/// warns again after `after`.
#[derive(Debug)]
struct NoPublisherWatch {
    /// Zero: no warning. The stream and the other schemas are checked all the same.
    after: Duration,
    /// Host time of the last check; zero before the first.
    checked_nanos: AtomicU64,
    state: std::sync::Mutex<WatchState>,
}

/// What a [`NoPublisherWatch`] knows between two checks.
#[derive(Debug)]
struct WatchState {
    /// Since when the stream has had no publisher; `None` while it has one.
    absent_since: Option<std::time::Instant>,
    /// The other schemas the last warning named; `None` when it has not warned since the
    /// stream last had a publisher.
    reported: Option<Vec<hypha_iceoryx2::RecordedStream>>,
    /// Before this instant no further warning is written.
    quiet_until: Option<std::time::Instant>,
}

/// What a check of a [`NoPublisherWatch`] decided.
#[derive(Debug, PartialEq, Eq)]
enum WatchDecision {
    /// Nothing to say now.
    Quiet,
    /// The stream had no publisher for the delay.
    Absent,
}

impl NoPublisherWatch {
    /// The shortest time between two warnings of one subscriber.
    const QUIET: Duration = Duration::from_secs(10);

    /// A watch that warns after `after`, counted from `since`. Zero warns never.
    fn new(after: Duration, since: std::time::Instant) -> Self {
        Self {
            after,
            checked_nanos: AtomicU64::new(0),
            state: std::sync::Mutex::new(WatchState {
                absent_since: Some(since),
                reported: None,
                quiet_until: None,
            }),
        }
    }

    /// How often the stream is checked: once a second, or once per `after` when that is
    /// shorter.
    fn check_nanos(&self) -> u64 {
        let after = u64::try_from(self.after.as_nanos()).unwrap_or(u64::MAX);
        if after == 0 { 1_000_000_000 } else { after.clamp(1, 1_000_000_000) }
    }

    /// True when a check is due at host time `now_nanos`. At most one caller gets `true`
    /// per interval.
    fn due(&self, now_nanos: u64) -> bool {
        let last = self.checked_nanos.load(Ordering::Acquire);
        if last != 0 && now_nanos.saturating_sub(last) < self.check_nanos() {
            return false;
        }
        self.checked_nanos
            .compare_exchange(last, now_nanos.max(1), Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// Whether the stream's publisher is there (`publisher` true) at `now`, and what that
    /// means for the warning.
    fn observe(&self, publisher: bool, now: std::time::Instant) -> WatchDecision {
        let mut state = crate::locked(&self.state);
        if publisher {
            state.absent_since = None;
            state.reported = None;
            return WatchDecision::Quiet;
        }
        let since = *state.absent_since.get_or_insert(now);
        if self.after.is_zero() || now.duration_since(since) < self.after {
            return WatchDecision::Quiet;
        }
        WatchDecision::Absent
    }

    /// True when a warning naming `published`, the other schemas of the topic, is due at
    /// `now`: they differ from the last warning's and no warning was written in the last
    /// [`Self::QUIET`]. They then become the last warning's.
    fn report(
        &self,
        published: &[hypha_iceoryx2::RecordedStream],
        now: std::time::Instant,
    ) -> bool {
        let mut state = crate::locked(&self.state);
        let unchanged = state.reported.as_deref() == Some(published);
        let quiet = state.quiet_until.is_some_and(|until| now < until);
        if unchanged || quiet {
            return false;
        }
        state.reported = Some(published.to_vec());
        state.quiet_until = now.checked_add(Self::QUIET);
        true
    }
}

/// A schema as a line names it: its type and the first eight hex digits of its MD5.
fn schema_label(data_type: &str, md5: &str) -> String {
    let short = md5.get(..8).unwrap_or(md5);
    let data_type = if data_type.is_empty() { "-" } else { data_type };
    format!("{data_type} ({short})")
}

/// Another stream as a line names it: by its record, or, when its endpoints gave no
/// definition, by the first four bytes of its schema hash, which are those of its MD5.
fn stream_label(stream: &hypha_iceoryx2::RecordedStream) -> String {
    if stream.md5.is_empty() {
        let [a, b, c, d, ..] = stream.schema_hash.to_le_bytes();
        return format!("- ({a:02x}{b:02x}{c:02x}{d:02x})");
    }
    schema_label(&stream.data_type, &stream.md5)
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
    /// The MD5 it was opened with, empty for none.
    pub(crate) md5: String,
    pub(crate) port: BackendSubscriber,
    budget: Arc<LoanBudget>,
    pub(crate) mode: AtomicU8,
    /// True while one `WaitSet` registration dispatches this subscriber.
    attached: AtomicBool,
    validator: FrameValidator,
    /// The warning when this subscriber's stream has no publisher.
    no_publisher_watch: NoPublisherWatch,
    /// The other streams of its topic, for a subscriber of one stream; `None` otherwise.
    pub(crate) watch: Option<crate::topic_streams::Watch>,
    /// Rate limit of the refused-frame warning.
    rejection_report: RejectionReport,
    /// The subscriber row of the node introspection, removed with this subscriber.
    pub(crate) introspection: crate::introspection::IntrospectionRow,
}

impl HyphaSubscriber {
    /// Warn, once per second at most, when the stream has had no publisher for the
    /// warning delay, naming the other schemas of the topic that are published, and again
    /// when those change while it still has none. A check reads the stream's publisher
    /// ports and the topic's streams as the node's housekeeping last found them: no
    /// syscall, no open. A port counts as a publisher until a refresh finds no live one on
    /// the stream. `now_nanos` is the host time the caller read.
    pub(crate) fn check_no_publisher_warning(&self, now_nanos: u64) {
        let watch = &self.no_publisher_watch;
        if !watch.due(now_nanos) {
            return;
        }
        let unpublished =
            self.watch.as_ref().is_some_and(crate::topic_streams::Watch::own_is_unpublished);
        let present = self.port.has_publisher_ports() && !unpublished;
        let now = std::time::Instant::now();
        if watch.observe(present, now) == WatchDecision::Quiet {
            return;
        }
        let published =
            self.watch.as_ref().and_then(crate::topic_streams::Watch::others).unwrap_or_default();
        if watch.report(&published, now) {
            log::warn!(target: "hypha/subscriber", "{}", self.no_publisher_line(&published, watch.after));
        }
    }

    /// The warning of a stream without a publisher, beside the `published` other schemas
    /// of its topic.
    fn no_publisher_line(
        &self,
        published: &[hypha_iceoryx2::RecordedStream],
        after: Duration,
    ) -> String {
        if published.is_empty() {
            let domain = match hypha_iceoryx2::domain::domain() {
                Ok("") => "<default>".to_owned(),
                Ok(domain) => domain.to_owned(),
                Err(cause) => format!("<unavailable: {cause:?}>"),
            };
            return format!(
                "hypha: no publisher connected to topic '{}' in domain '{domain}' after {} ms; if the publisher runs in another container, check that both /dev/shm and /tmp/iceoryx2 are shared",
                self.topic,
                after.as_millis()
            );
        }
        let others: Vec<String> = published.iter().map(stream_label).collect();
        format!(
            "hypha: topic '{}' has publishers, none of type {}: published are {}",
            self.topic,
            schema_label(&self.data_type, &self.md5),
            others.join(", ")
        )
    }

    /// The cumulative count of frames this subscriber lost before it read them. It only grows.
    #[must_use]
    pub fn missing_sequences(&self) -> u64 {
        self.validator.missing_sequences()
    }

    /// Frames sent before the first accepted from each publisher.
    pub(crate) fn validator_sent_before_first(&self) -> u64 {
        self.validator.sent_before_first()
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
        // A delivery borrows one sample while its callback runs, so a callback may keep
        // the others.
        self.budget.budget.store(MAX_BORROWED_SAMPLES.saturating_sub(1), Ordering::Release);
        Ok(previous)
    }

    /// The budget of the samples this subscriber's readers hold.
    pub(crate) fn loan_budget(&self) -> &Arc<LoanBudget> {
        &self.budget
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
    let mut spec = TopicSpec::new(name).with_qos(settings.request.clone());
    if !data_type.is_empty() {
        spec = spec.with_type(data_type);
    }
    // An MD5 selects the stream of its schema; without one the subscriber reads them all.
    spec = match expected_md5 {
        Some(md5) => spec.with_md5(md5),
        None => spec.reading_every_schema(),
    };
    if !definition.is_empty() {
        if expected_md5.is_none() || data_type.is_empty() {
            return Err(Error::InvalidSchema);
        }
        spec = spec.with_definition(definition);
    }
    let watch = (!name.contains('*'))
        .then_some(expected_schema_hash)
        .flatten()
        .map(|own| node.topic_streams.watch(name, own));
    let md5 = expected_md5.unwrap_or_default();
    open(node, spec, &settings, Own { data_type, md5, schema_hash: expected_schema_hash }, watch)
}

/// Subscribe on `node` to the one stream of `schema_hash` on the literal `topic`, which the
/// subscriber names no MD5 for: a reader that follows the streams a listing shows, such as
/// a tool, reads each by its hash whether or not its schema is known. It reads no other
/// stream, and a frame on it that names another hash is refused as in any typed subscriber.
///
/// # Errors
/// `InvalidTopic` for a refused name or a pattern, `NotAvailable` when the stream does not
/// exist, and the transport's error otherwise.
pub fn subscribe_stream(
    node: &HyphaNode,
    topic: &str,
    schema_hash: u64,
    settings: SubscriberSettings,
) -> Result<Arc<HyphaSubscriber>, Error> {
    let resolved = node.resolve_name(topic)?;
    if resolved.contains('*') {
        return Err(Error::InvalidTopic);
    }
    let spec = TopicSpec::new(resolved.as_str())
        .with_qos(settings.request.clone())
        .reading_stream(schema_hash);
    open(
        node,
        spec,
        &settings,
        Own { data_type: "", md5: "", schema_hash: Some(schema_hash) },
        None,
    )
}

/// What a subscriber says it reads: the type and MD5 its introspection row names, and the
/// hash every frame on its stream must carry; `None` for a reader of several streams.
struct Own<'a> {
    data_type: &'a str,
    md5: &'a str,
    schema_hash: Option<u64>,
}

/// The subscriber of `spec`, with the type and MD5 its introspection row names. Its frames
/// must carry the hash of the one stream it reads, when it reads one.
fn open(
    node: &HyphaNode,
    mut spec: TopicSpec,
    settings: &SubscriberSettings,
    own: Own<'_>,
    watch: Option<crate::topic_streams::Watch>,
) -> Result<Arc<HyphaSubscriber>, Error> {
    let Own { data_type, md5, schema_hash: expected_schema_hash } = own;
    if settings.depth > 0 {
        spec = spec.keeping(settings.depth);
    }
    let name = spec.name.clone();
    let port = node.transport.subscribe(&spec).map_err(Error::from)?;
    let depth = port.buffer_size();
    let budget = Arc::new(LoanBudget {
        budget: AtomicUsize::new(depth.min(MAX_BORROWED_SAMPLES)),
        held: AtomicUsize::new(0),
        refused: AtomicU64::new(0),
    });
    Ok(Arc::new(HyphaSubscriber {
        owner_node: std::ptr::from_ref(node) as usize,
        topic: name.clone(),
        data_type: data_type.to_owned(),
        md5: md5.to_owned(),
        port,
        budget,
        mode: AtomicU8::new(MODE_UNSET),
        attached: AtomicBool::new(false),
        validator: FrameValidator::new(expected_schema_hash),
        no_publisher_watch: NoPublisherWatch::new(
            no_publisher_delay(&name, settings.no_publisher_warning_after_nanos),
            std::time::Instant::now(),
        ),
        watch,
        rejection_report: RejectionReport::default(),
        introspection: crate::introspection::IntrospectionRow::new(
            &node.introspection,
            crate::introspection::Row::Subscriber(hypha_core::introspection::EndpointRow {
                queue_depth: depth,
                md5: md5.to_owned(),
                ..hypha_core::introspection::EndpointRow::new(&name, data_type)
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

/// `HyphaSubscriberCounters::other_schemas` before the other streams were checked.
pub const HYPHA_NOT_CHECKED: u64 = u64::MAX;

/// The counters of one subscriber, as `hypha_subscriber_counters` writes them: nine
/// `uint64_t`, 72 bytes, owned by the caller, with nothing to destroy. Every counter but
/// `loan_budget`, `loan_held` and `other_schemas` only grows, and wraps.
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
    /// `integrity.schema_mismatch`: frames whose header names another schema than the
    /// stream they arrived on, a fault of their sender.
    pub schema_mismatch: u64,
    /// `integrity.invalid_timestamp`: frames sent later than the host clock reads.
    pub invalid_timestamp: u64,
    /// `loan.budget`: the owning samples the subscriber may hold at once.
    pub loan_budget: u64,
    /// `loan.held`: the owning samples it holds now.
    pub loan_held: u64,
    /// `loan.refused`: owning receives refused because the budget was full.
    pub loan_refused: u64,
    /// `topic.other_schemas`: the other schemas (MD5s) of the subscriber's topic that a
    /// live publisher sends, as the node's housekeeping last found them, at most a second
    /// old while its default `WaitSet` runs. [`HYPHA_NOT_CHECKED`] before the first check,
    /// and for a subscriber of no single stream. A fact about the topic, not a fault count:
    /// a migration has two versions on it.
    pub other_schemas: u64,
}

/// The counters of `endpoint`. Each counter is read atomically, the nine not at one instant.
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
        loan_budget: u64::try_from(endpoint.budget.budget.load(Ordering::Acquire)).unwrap_or(0),
        loan_held: u64::try_from(endpoint.budget.held.load(Ordering::Acquire)).unwrap_or(0),
        loan_refused: endpoint.budget.refused.load(Ordering::Acquire),
        other_schemas: endpoint
            .watch
            .as_ref()
            .and_then(crate::topic_streams::Watch::others)
            .map_or(HYPHA_NOT_CHECKED, |others| u64::try_from(others.len()).unwrap_or(u64::MAX)),
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
        .validate_with_clock(sample.topic_code(), &header, sample.payload(), now_nanos, host_now_nanos)
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
    use super::{NoPublisherWatch, WatchDecision, schema_label};
    use hypha_iceoryx2::RecordedStream;
    use std::time::{Duration, Instant};

    fn schema(data_type: &str, md5: &str) -> RecordedStream {
        RecordedStream { schema_hash: 1, data_type: data_type.to_owned(), md5: md5.to_owned() }
    }

    #[test]
    fn an_absence_is_reported_after_the_delay_then_on_a_change_at_most_every_ten_seconds() {
        let start = Instant::now();
        let watch = NoPublisherWatch::new(Duration::from_secs(5), start);
        let at = |seconds| start + Duration::from_secs(seconds);
        let other = [schema("pkg/T", "581e4b1b")];
        assert_eq!(watch.observe(false, at(4)), WatchDecision::Quiet);
        assert_eq!(
            watch.observe(false, start + Duration::from_nanos(4_999_999_999)),
            WatchDecision::Quiet
        );
        assert_eq!(watch.observe(false, at(5)), WatchDecision::Absent);
        assert!(watch.report(&[], at(5)), "the first report");
        assert!(!watch.report(&[], at(20)), "the same absence is said once");
        assert!(watch.report(&other, at(21)), "another schema appeared");
        assert!(!watch.report(&[], at(22)), "a change within ten seconds waits");
        assert!(!watch.report(&other, at(30)), "and is not said when it went back");
        assert!(watch.report(&[], at(32)), "a change that lasts is said after the quiet time");
    }

    #[test]
    fn a_publisher_rearms_the_watch_and_zero_never_warns() {
        let start = Instant::now();
        let watch = NoPublisherWatch::new(Duration::from_secs(5), start);
        let at = |seconds| start + Duration::from_secs(seconds);
        assert_eq!(watch.observe(false, at(6)), WatchDecision::Absent);
        assert!(watch.report(&[], at(6)));
        assert_eq!(watch.observe(true, at(7)), WatchDecision::Quiet);
        assert_eq!(watch.observe(false, at(8)), WatchDecision::Quiet, "the delay starts again");
        assert_eq!(watch.observe(false, at(16)), WatchDecision::Absent);
        assert!(watch.report(&[], at(16)), "a new absence is reported again");

        let silent = NoPublisherWatch::new(Duration::ZERO, start);
        assert_eq!(silent.observe(false, at(100)), WatchDecision::Quiet);
        assert!(silent.due(1), "the stream is still checked");
    }

    #[test]
    fn the_stream_is_checked_once_per_second_or_once_per_shorter_delay() {
        let second = 1_000_000_000;
        let watch = NoPublisherWatch::new(Duration::from_secs(5), Instant::now());
        assert!(watch.due(7));
        assert!(!watch.due(7 + second - 1));
        assert!(watch.due(7 + second));

        let short = NoPublisherWatch::new(Duration::from_millis(100), Instant::now());
        assert!(short.due(7));
        assert!(!short.due(7 + 99_000_000));
        assert!(short.due(7 + 100_000_000));
    }

    #[test]
    fn a_schema_is_named_by_its_type_and_the_start_of_its_md5() {
        assert_eq!(
            schema_label("overload_test/First", "d9d213e8c146065ec958333c617eb1a6"),
            "overload_test/First (d9d213e8)"
        );
        assert_eq!(schema_label("", "d9d2"), "- (d9d2)");
        let hash = hypha_wire::schema_hash_from_md5("f79835def461350458ead76f07016dd7");
        let bare = RecordedStream {
            schema_hash: hash.unwrap_or(0),
            data_type: String::new(),
            md5: String::new(),
        };
        assert_eq!(super::stream_label(&bare), "- (f79835de)");
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
