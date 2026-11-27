//! The two endpoints, and the buffers they hand out.
//!
//! `new_set` resolves its streams once: matching costs O(services x nodes)
//! file-lock probes (175 ms with 120 publishers), so it is paid at open.

use std::sync::Arc;

use iceoryx2::port::{LoanError, SendError};
use iceoryx2::prelude::CallbackProgression;
use iceoryx2::service::port_factory::PortFactory;

use hypha_core::port::{PortError, QosRequest, SchemaScope, topic_matches_pattern};
use hypha_wire::frame::{FLAG_LATCHED, FrameHeader, FrameMetadata, TopicCode, seal};
use iceoryx2::port::ReceiveError;
use iceoryx2::port::listener::Listener;
use iceoryx2::port::notifier::Notifier;
use iceoryx2::port::publisher::Publisher;
use iceoryx2::port::subscriber::Subscriber;
use iceoryx2::port::update_connections::UpdateConnections;
use iceoryx2::sample::Sample;
use iceoryx2::sample_mut::SampleMut;

use crate::header::Iox2FrameHeader;
use crate::latch::is_unlatched_history;
use crate::stream::Stream;
use crate::transport::{DataPortFactory, Ipc};

/// Backend code of a loan that failed inside iceoryx2 (`LoanError::InternalFailure`),
/// and of a size the publisher cannot name a limit for.
pub(crate) const BACKEND_LOAN_INTERNAL: u32 = 36;
/// Backend code of a send to a connection whose data is corrupted.
pub(crate) const BACKEND_SEND_CORRUPTED: u32 = 37;
/// Backend code of a send that could not establish a connection to a subscriber.
pub(crate) const BACKEND_SEND_CONNECTION: u32 = 38;
/// Backend code of a send that reached some subscribers but not all.
pub(crate) const BACKEND_SEND_UNDELIVERED: u32 = 39;
/// Backend code of a send that failed inside iceoryx2.
pub(crate) const BACKEND_SEND_INTERNAL: u32 = 40;

/// The status of a refused loan. A full pool and the publisher's loan budget are
/// [`PortError::PoolExhausted`]; every other refusal names its cause.
pub(crate) fn loan_port_error(
    cause: LoanError,
    requested: usize,
    max_payload: Option<usize>,
) -> PortError {
    match cause {
        LoanError::OutOfMemory | LoanError::ExceedsMaxLoans => PortError::PoolExhausted,
        LoanError::ExceedsMaxLoanSize => match max_payload {
            Some(limit) => PortError::TooLarge { requested, limit },
            None => PortError::Backend(BACKEND_LOAN_INTERNAL),
        },
        LoanError::InternalFailure => PortError::Backend(BACKEND_LOAN_INTERNAL),
    }
}

/// The status of a failed send.
pub(crate) fn send_port_error(cause: SendError) -> PortError {
    match cause {
        SendError::ConnectionBrokenSinceSenderNoLongerExists => PortError::Disconnected,
        SendError::LoanError(loan) => loan_port_error(loan, 0, None),
        SendError::ConnectionError(_) => PortError::Backend(BACKEND_SEND_CONNECTION),
        SendError::ConnectionCorrupted => PortError::Backend(BACKEND_SEND_CORRUPTED),
        SendError::UnableToDeliver => PortError::Backend(BACKEND_SEND_UNDELIVERED),
        SendError::InternalError => PortError::Backend(BACKEND_SEND_INTERNAL),
    }
}

/// A slice of the shared segment, lent to write a message into. The slice is the
/// message payload alone; its frame header is written when the loan is sent.
pub struct Iox2Loan {
    sample: SampleMut<Ipc, [u8], Iox2FrameHeader>,
    /// The code of the publisher's topic, which the frame is sealed for.
    topic_code: TopicCode,
    notifier: Arc<Notifier<Ipc>>,
    /// True when the publisher latches its samples.
    latched: bool,
}

impl Iox2Loan {
    /// The bytes currently in the buffer.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.sample.payload()
    }

    /// The bytes to write the message into.
    pub fn as_mut_bytes(&mut self) -> &mut [u8] {
        self.sample.payload_mut()
    }

    /// How many bytes the buffer holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.sample.payload().len()
    }

    /// True when the buffer holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Seal the payload with `metadata` and `flags`, then send it, consuming the
    /// loan. Dropping the loan without sending returns the slot to the publisher.
    /// A failed wake notification is logged and counted ([`crate::failed_notifications`]),
    /// not returned: the sample is visible, so a retry would send it twice.
    ///
    /// # Errors
    /// The error of the failed send, see [`send_port_error`].
    pub fn send_frame(mut self, metadata: FrameMetadata, flags: u32) -> Result<(), PortError> {
        let flags = if self.latched { flags | FLAG_LATCHED } else { flags };
        let header = seal(self.topic_code, metadata, flags, self.sample.payload());
        *self.sample.user_header_mut() = Iox2FrameHeader::from(header);
        self.sample.send().map_err(send_port_error)?;
        if !crate::notification::notified(&self.notifier.notify()) {
            log::warn!(target: "hypha/transport", "hypha_iceoryx2: data was sent but its wake notification failed");
        }
        Ok(())
    }
}

/// One sample, read where the publisher wrote it.
pub struct Iox2Sample {
    topic: Arc<str>,
    sample: Sample<Ipc, [u8], Iox2FrameHeader>,
}

impl Iox2Sample {
    /// The topic this sample arrived on.
    #[must_use]
    pub fn topic(&self) -> &str {
        &self.topic
    }

    /// The code of the topic this sample arrived on, which its frame must be sealed for.
    #[must_use]
    pub fn topic_code(&self) -> TopicCode {
        TopicCode::of(&self.topic)
    }

    /// The frame header the publisher sealed. Verify it with
    /// [`hypha_wire::frame::verify`] against [`Self::topic_code`] and [`Self::payload`].
    #[must_use]
    pub fn header(&self) -> FrameHeader {
        FrameHeader::from(*self.sample.user_header())
    }

    /// The message bytes, read where the publisher wrote them.
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        self.sample.payload()
    }
}

/// Sends samples on one topic.
pub struct Iox2Publisher {
    /// Shared with the latch registry of a latched topic.
    pub(crate) publisher: Arc<Publisher<Ipc, [u8], Iox2FrameHeader>>,
    /// The topic's service. Its dynamic config lists the subscriber ports.
    pub(crate) service: Arc<DataPortFactory>,
    pub(crate) topic: String,
    /// The code of `topic`, which every frame of this publisher is sealed for.
    pub(crate) topic_code: TopicCode,
    pub(crate) notifier: Arc<Notifier<Ipc>>,
    /// True when this publisher latches its samples.
    pub(crate) latched: bool,
    /// The largest loan, or `None` for a growable publisher.
    pub(crate) max_payload: Option<usize>,
    /// The current run of loans the pool refused; a lock so a loan takes `&self`.
    pub(crate) pool_refusals: std::sync::Mutex<crate::refusal_episode::RefusalEpisode>,
    /// True while a refusal run is open, so a granted loan outside one takes no lock.
    pub(crate) pool_refusing: std::sync::atomic::AtomicBool,
    /// After the port and the service handle, so the record goes when no port is left.
    pub(crate) _schema_release: crate::schema::SchemaRelease,
    /// Last: the graph event of its drop follows the port's.
    pub(crate) _graph: crate::transport::GraphAnnouncement,
}

impl Iox2Publisher {
    /// The largest message this publisher sends, or `None` for a growable publisher.
    #[must_use]
    pub fn max_payload(&self) -> Option<usize> {
        self.max_payload
    }

    /// Borrow a buffer of `size` bytes to write a message into.
    ///
    /// # Errors
    /// [`PortError::TooLarge`] when `size` exceeds the payload capacity,
    /// [`PortError::PoolExhausted`] when no buffer is free; any other refusal names
    /// its cause. A publisher never waits for a slow subscriber.
    pub fn loan(&self, size: usize) -> Result<Iox2Loan, PortError> {
        if let Some(limit) = self.max_payload.filter(|limit| size > *limit) {
            return Err(PortError::TooLarge { requested: size, limit });
        }
        let loaned = self.publisher.loan_slice(size);
        let pool_refused = matches!(loaned, Err(iceoryx2::port::LoanError::OutOfMemory));
        let tracked = pool_refused
            || (loaned.is_ok() && self.pool_refusing.load(std::sync::atomic::Ordering::Relaxed));
        let persists = tracked && {
            self.pool_refusing.store(pool_refused, std::sync::atomic::Ordering::Relaxed);
            crate::locked(&self.pool_refusals).observe(pool_refused, std::time::Instant::now)
        };
        if persists {
            log::warn!(target: "hypha/transport",
                "hypha: topic '{}' refuses loans although this publisher is below its loan limit; its pool \
                 may be exhausted by slow subscribers or corrupted. If it persists, restart the node.",
                self.topic);
        }
        loaned
            .map(|sample| Iox2Loan {
                sample,
                topic_code: self.topic_code,
                notifier: self.notifier.clone(),
                latched: self.latched,
            })
            .map_err(|cause| loan_port_error(cause, size, self.max_payload))
    }

    /// Refresh connections to subscribers.
    ///
    /// # Errors
    /// [`PortError::Disconnected`] when the endpoint cannot refresh its links.
    pub fn update_connections(&self) -> Result<(), PortError> {
        self.publisher.update_connections().map_err(|_| PortError::Disconnected)
    }

    /// The number of subscriber ports on this topic on this machine, from any transport.
    #[must_use]
    pub fn subscriber_count(&self) -> Option<usize> {
        Some(self.service.dynamic_config().number_of_subscribers())
    }

    /// The smallest queue among the subscribers connected to this topic, or
    /// `None` when none is connected.
    #[must_use]
    pub fn min_subscriber_buffer(&self) -> Option<usize> {
        let mut smallest: Option<usize> = None;
        self.service.dynamic_config().list_subscribers(|details| {
            smallest = Some(smallest.map_or(details.buffer_size, |s| s.min(details.buffer_size)));
            CallbackProgression::Continue
        });
        smallest
    }

    /// The topic this publisher sends on.
    #[must_use]
    pub fn topic(&self) -> &str {
        &self.topic
    }
}

/// One opened subscriber, paired with the topic it reads.
type NamedSubscriber = (Subscriber<Ipc, [u8], Iox2FrameHeader>, Arc<str>, DataPortFactory);

/// Reads samples from one topic.
pub struct Iox2Subscriber {
    inner: SubscriberKind,
}

struct SingleSubscriber {
    subscriber: Subscriber<Ipc, [u8], Iox2FrameHeader>,
    /// Steady-clock time before the subscriber was created; a sample sent earlier is history.
    created_nanos: u64,
    service: DataPortFactory,
    topic: Arc<str>,
    event_source: EventSource,
    /// Last, so it drops after the port and the service handle.
    _schema_release: crate::schema::SchemaRelease,
}

struct EventSource {
    node: crate::transport::SharedNode,
    service_name: String,
}

enum SubscriberKind {
    /// The stream of one topic and schema.
    Single(Box<SingleSubscriber>),
    /// Every stream of a pattern, or of every schema of a topic, that existed at creation.
    Set {
        topic_pattern: String,
        /// The node that listens to the set's streams; `None` for a pattern, which is
        /// polled.
        events: Option<crate::transport::SharedNode>,
        subscribers: Vec<NamedSubscriber>,
        current_idx: std::sync::atomic::AtomicUsize,
        created_nanos: u64,
        /// Last, so they drop after the subscribers.
        _schema_releases: Vec<crate::schema::SchemaRelease>,
    },
}

/// The next sample of `subscriber` that is not history of an unlatched publisher.
fn receive_current(
    subscriber: &Subscriber<Ipc, [u8], Iox2FrameHeader>,
    topic: &str,
    created_nanos: u64,
) -> Result<Option<Sample<Ipc, [u8], Iox2FrameHeader>>, ReceiveError> {
    loop {
        match subscriber.receive()? {
            Some(sample)
                if is_unlatched_history(
                    topic,
                    sample.user_header(),
                    sample.payload(),
                    created_nanos,
                ) => {}
            other => return Ok(other),
        }
    }
}

fn receive_port_error(cause: ReceiveError) -> PortError {
    match cause {
        ReceiveError::ExceedsMaxBorrows => PortError::BorrowLimit,
        ReceiveError::ConnectionFailure(_) => PortError::Disconnected,
    }
}

impl Iox2Subscriber {
    /// Take the next sample, or `None` when none is pending. Never blocks.
    ///
    /// # Errors
    /// [`PortError::Disconnected`] when the endpoint is closed, or
    /// [`PortError::BorrowLimit`] when outstanding samples reach the budget.
    pub fn try_receive(&self) -> Result<Option<Iox2Sample>, PortError> {
        use std::sync::atomic::Ordering::Relaxed;
        match &self.inner {
            SubscriberKind::Single(single) => {
                let received =
                    receive_current(&single.subscriber, &single.topic, single.created_nanos)
                        .map_err(receive_port_error)?;
                Ok(received.map(|sample| Iox2Sample { sample, topic: Arc::clone(&single.topic) }))
            }
            SubscriberKind::Set { subscribers, current_idx, created_nanos, .. } => {
                if subscribers.is_empty() {
                    return Ok(None);
                }

                let start_idx = current_idx.load(Relaxed);
                let mut index = start_idx;
                let num_subs = subscribers.len();
                let mut first_error: Option<PortError> = None;
                // num_subs > 0 here, so the modulo neither divides by zero nor overflows.
                #[allow(clippy::arithmetic_side_effects)]
                let advance = |idx: usize| -> usize { idx.wrapping_add(1) % num_subs };
                loop {
                    let mut found = None;
                    {
                        let Some((sub, topic, _)) = subscribers.get(index) else {
                            break;
                        };
                        match receive_current(sub, topic, *created_nanos) {
                            Ok(Some(sample)) => found = Some((sample, Arc::clone(topic))),
                            Ok(None) => {}
                            Err(cause) => {
                                if first_error.is_none() {
                                    first_error = Some(receive_port_error(cause));
                                }
                            }
                        }
                    }
                    index = advance(index);
                    current_idx.store(index, Relaxed);
                    if let Some((sample, topic)) = found {
                        return Ok(Some(Iox2Sample { topic, sample }));
                    }
                    if index == start_idx {
                        break;
                    }
                }

                first_error.map_or(Ok(None), Err)
            }
        }
    }

    /// Number of live publishers connected to this input, over every topic it follows.
    /// A publisher whose node iceoryx2 reports dead is not counted: iceoryx2 keeps its port
    /// until a node reclaims it.
    ///
    /// # Errors
    /// [`PortError::Backend`] when the node registry cannot be read.
    pub fn publisher_count(&self) -> Result<usize, PortError> {
        match &self.inner {
            SubscriberKind::Single(single) => live_publisher_count(&single.service),
            SubscriberKind::Set { subscribers, .. } => {
                let mut total = 0_usize;
                for (_, _, service) in subscribers {
                    let live = live_publisher_count(service)?;
                    total = total.saturating_add(live);
                }
                Ok(total)
            }
        }
    }

    /// True while a publisher port is open on a stream this subscriber reads: one atomic
    /// read per stream. A port of a dead node counts until a node reclaims it.
    #[must_use]
    pub fn has_publisher_ports(&self) -> bool {
        self.port_factories().any(|service| service.dynamic_config().number_of_publishers() > 0)
    }

    /// The service of every topic this subscriber reads.
    pub(crate) fn port_factories(&self) -> impl Iterator<Item = &DataPortFactory> {
        let (single, set) = match &self.inner {
            SubscriberKind::Single(single) => (Some(&single.service), None),
            SubscriberKind::Set { subscribers, .. } => (None, Some(subscribers)),
        };
        single.into_iter().chain(set.into_iter().flatten().map(|(_, _, service)| service))
    }

    /// The topic this subscriber reads. A set reports its pattern or topic.
    #[must_use]
    pub fn topic(&self) -> &str {
        match &self.inner {
            SubscriberKind::Single(single) => &single.topic,
            SubscriberKind::Set { topic_pattern, .. } => topic_pattern,
        }
    }

    /// Returns the buffer size of this subscriber.
    #[must_use]
    pub fn buffer_size(&self) -> usize {
        match &self.inner {
            SubscriberKind::Single(single) => single.subscriber.buffer_size(),
            SubscriberKind::Set { subscribers, .. } => subscribers
                .first()
                .map_or(crate::qos::DEFAULT_MAX_DEPTH, |(s, _, _)| s.buffer_size()),
        }
    }

    pub(crate) fn new(
        subscriber: Subscriber<Ipc, [u8], Iox2FrameHeader>,
        created_nanos: u64,
        service: DataPortFactory,
        stream: Stream,
        node: crate::transport::SharedNode,
        schema_release: crate::schema::SchemaRelease,
    ) -> Self {
        let event_source = EventSource { node, service_name: stream.service_name() };
        Self {
            inner: SubscriberKind::Single(Box::new(SingleSubscriber {
                subscriber,
                created_nanos,
                service,
                topic: Arc::from(stream.topic),
                event_source,
                _schema_release: schema_release,
            })),
        }
    }

    /// Open a listener on the event service of each stream this subscriber reads: one for
    /// a single stream, one per stream for every schema of a topic. Empty for a wildcard
    /// pattern, which is polled.
    /// # Errors
    /// Returns [`PortError`] when the node is gone or an event service has no
    /// room for another listener.
    pub fn open_listeners(&self) -> Result<Vec<Listener<Ipc>>, PortError> {
        let listen = |node: &crate::transport::SharedNode, name: &str, service| {
            let limits = crate::notification::EventLimits::of_topic(service);
            crate::notification::create_listener(node, name, Some(limits))
        };
        match &self.inner {
            SubscriberKind::Single(single) => {
                let source = &single.event_source;
                Ok(vec![listen(&source.node, &source.service_name, &single.service)?])
            }
            SubscriberKind::Set { events: None, .. } => Ok(Vec::new()),
            SubscriberKind::Set { events: Some(node), subscribers, .. } => subscribers
                .iter()
                .map(|(_, _, service)| listen(node, service.name().as_str(), service))
                .collect(),
        }
    }

    /// Resolves the streams of `spec` once and follows every one it matched: of each
    /// topic its pattern matches, the stream of its schema, or every stream when it reads
    /// every schema.
    ///
    /// # Errors
    ///
    /// Returns an error when enumeration fails or a matching data service
    /// refuses the subscriber. A partial subscription is never returned.
    /// [`PortError::NotAvailable`] for a topic, not a pattern, with no stream.
    pub(crate) fn new_set(
        node: &crate::transport::SharedNode,
        spec: &hypha_core::port::TopicSpec,
    ) -> Result<Self, PortError> {
        let created_nanos = steady_nanos()?;
        let (subscribers, streams) = open_matching_streams(node, spec)?;
        let is_topic = !spec.name.contains('*');
        if is_topic && subscribers.is_empty() {
            return Err(PortError::NotAvailable);
        }
        let schema_releases =
            streams.iter().map(|stream| crate::schema::SchemaRelease::new(node, stream)).collect();
        Ok(Self {
            inner: SubscriberKind::Set {
                topic_pattern: spec.name.clone(),
                events: is_topic.then(|| node.clone()),
                subscribers,
                current_idx: std::sync::atomic::AtomicUsize::new(0),
                created_nanos,
                _schema_releases: schema_releases,
            },
        })
    }
}

/// The publishers of `service` whose node iceoryx2 does not report dead.
fn live_publisher_count(service: &DataPortFactory) -> Result<usize, PortError> {
    let dynamic = service.dynamic_config();
    if dynamic.number_of_publishers() == 0 {
        return Ok(0);
    }
    let dead = dead_node_ids()?;
    let mut live = 0_usize;
    dynamic.list_publishers(|details| {
        if !dead.contains(&details.node_id) {
            live = live.saturating_add(1);
        }
        CallbackProgression::Continue
    });
    Ok(live)
}

/// The ids of the nodes of the domain that iceoryx2 reports dead.
pub(crate) fn dead_node_ids() -> Result<Vec<iceoryx2::identifiers::UniqueNodeId>, PortError> {
    use iceoryx2::node::{NodeState, NodeView as _};

    let config = crate::domain::config()?;
    let mut dead = Vec::new();
    iceoryx2::node::Node::<Ipc>::list(config, |state| {
        if let NodeState::Dead(view) = state {
            dead.push(*view.id());
        }
        CallbackProgression::Continue
    })
    .map_err(crate::transport::node_list_failed)?;
    Ok(dead)
}

pub(crate) fn steady_nanos() -> Result<u64, PortError> {
    u64::try_from(hypha_time::steady_now().nanos()).map_err(|_| PortError::Backend(3))
}

/// The subscribers of every stream that `spec` selects, and the streams in the same order.
fn open_matching_streams(
    node: &crate::transport::SharedNode,
    spec: &hypha_core::port::TopicSpec,
) -> Result<(Vec<NamedSubscriber>, Vec<Stream>), PortError> {
    let own_schema = match spec.schemas {
        SchemaScope::Own => Some(spec.stream_schema_hash().ok_or(PortError::InvalidTopic)?),
        SchemaScope::Every => None,
        SchemaScope::Stream(hash) => Some(hash),
    };
    let mut subscribers = Vec::new();
    let mut streams = Vec::new();
    let port_name = crate::transport::node_port_name(node)?;
    for (name, stream) in crate::discovery::topic_streams()? {
        let selected = topic_matches_pattern(&spec.name, &stream.topic)
            && own_schema.is_none_or(|hash| hash == stream.schema_hash);
        if !selected {
            continue;
        }
        let full_topic = stream.topic.clone();
        let service_name = iceoryx2::prelude::ServiceName::new(&name)
            .map_err(|_| PortError::Backend(crate::service_open::BACKEND_SERVICE_CORRUPTED))?;
        let opened = crate::service_open::open_within_window(
            "the data service of a stream set",
            node,
            PortError::Backend(0),
            |node_ref| crate::transport::data_service_builder(node_ref, &service_name).open(),
            crate::service_open::publish_subscribe_open_is_transient,
        )?;
        let service = match opened {
            Ok(service) => service,
            Err(
                iceoryx2::service::builder::publish_subscribe::PublishSubscribeOpenError::DoesNotExist
                | iceoryx2::service::builder::publish_subscribe::PublishSubscribeOpenError::IncompatibleMessagingPattern,
            ) => continue,
            Err(cause) => {
                let hangs = cause
                    == iceoryx2::service::builder::publish_subscribe::PublishSubscribeOpenError::HangsInCreation;
                log::error!(target: "hypha/transport", "hypha_iceoryx2: cannot open data service '{name}': {cause:?}{}",
                        crate::service_open::half_created_advice(hangs));
                return Err(crate::service_open::publish_subscribe_open_port_error(cause));
            }
        };
        // A set that declares a policy accepts only services that carry it.
        crate::qos::check_declared_service(&full_topic, &spec.qos, service.attributes())?;
        let slots = match &spec.qos {
            QosRequest::Declared(_) => {
                let stamped = crate::qos::policy_of_service(service.attributes())?;
                crate::qos::declared_slots(&full_topic, &stamped)
            }
            QosRequest::Undeclared => crate::qos::observer_slots(&full_topic),
        };
        let subscriber = crate::subscriber_port::create_subscriber(
            &service,
            &full_topic,
            spec.buffer_size,
            &port_name,
            slots,
        )?;
        subscribers.push((subscriber, Arc::from(full_topic), service));
        streams.push(stream);
    }
    Ok((subscribers, streams))
}

#[cfg(test)]
mod tests {
    use super::{BACKEND_LOAN_INTERNAL, BACKEND_SEND_INTERNAL, loan_port_error, send_port_error};
    use hypha_core::port::PortError;
    use iceoryx2::port::{LoanError, SendError};

    #[test]
    fn a_full_pool_and_the_loan_budget_are_pool_exhaustion() {
        assert_eq!(loan_port_error(LoanError::OutOfMemory, 8, None), PortError::PoolExhausted);
        assert_eq!(loan_port_error(LoanError::ExceedsMaxLoans, 8, None), PortError::PoolExhausted);
    }

    #[test]
    fn a_loan_over_the_publisher_limit_is_too_large_and_names_the_limit() {
        assert_eq!(
            loan_port_error(LoanError::ExceedsMaxLoanSize, 16, Some(8)),
            PortError::TooLarge { requested: 16, limit: 8 }
        );
        assert_eq!(
            loan_port_error(LoanError::ExceedsMaxLoanSize, 16, None),
            PortError::Backend(BACKEND_LOAN_INTERNAL),
            "a growable publisher has no limit to name"
        );
    }

    #[test]
    fn an_internal_loan_failure_is_a_backend_failure() {
        assert_eq!(
            loan_port_error(LoanError::InternalFailure, 8, None),
            PortError::Backend(BACKEND_LOAN_INTERNAL)
        );
    }

    #[test]
    fn a_send_to_a_publisher_that_is_gone_is_disconnected_and_others_name_their_cause() {
        assert_eq!(
            send_port_error(SendError::ConnectionBrokenSinceSenderNoLongerExists),
            PortError::Disconnected
        );
        assert_eq!(
            send_port_error(SendError::InternalError),
            PortError::Backend(BACKEND_SEND_INTERNAL)
        );
        assert_eq!(
            send_port_error(SendError::LoanError(LoanError::OutOfMemory)),
            PortError::PoolExhausted
        );
    }
}
