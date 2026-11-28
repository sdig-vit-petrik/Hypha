//! The subscriber endpoint: one stream, or every stream of a pattern or schema set.
//!
//! `new_set` resolves its streams once: matching costs O(services x nodes)
//! file-lock probes (175 ms with 120 publishers), so it is paid at open.

use std::sync::Arc;

use iceoryx2::prelude::CallbackProgression;
use iceoryx2::service::port_factory::PortFactory;

use hypha_core::port::{PortError, QosRequest, SchemaScope, topic_matches_pattern};
use iceoryx2::port::ReceiveError;
use iceoryx2::port::listener::Listener;
use iceoryx2::port::subscriber::Subscriber;
use iceoryx2::sample::Sample;

use crate::endpoint::Iox2Sample;
use crate::header::Iox2FrameHeader;
use crate::latch::is_unlatched_history;
use crate::stream::Stream;
use crate::transport::{DataPortFactory, Ipc};

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

    /// The samples one publisher on each topic this subscriber follows can queue: its
    /// [`Self::buffer_size`] for a single stream, the sum of each stream's for a set.
    #[must_use]
    pub fn topics_buffer_size(&self) -> usize {
        match &self.inner {
            SubscriberKind::Single(single) => single.subscriber.buffer_size(),
            SubscriberKind::Set { subscribers, .. } => subscribers
                .iter()
                .map(|(subscriber, _, _)| subscriber.buffer_size())
                .fold(0, usize::saturating_add),
        }
    }

    /// The most samples this subscriber can hold queued: iceoryx2 gives each publisher
    /// its own queue, so one queue of each topic's depth for each publisher the topic
    /// admits.
    #[must_use]
    pub fn queue_capacity(&self) -> usize {
        let capacity = |service: &DataPortFactory, depth: usize| {
            service.static_config().max_publishers().saturating_mul(depth)
        };
        match &self.inner {
            SubscriberKind::Single(single) => {
                capacity(&single.service, single.subscriber.buffer_size())
            }
            SubscriberKind::Set { subscribers, .. } => subscribers
                .iter()
                .map(|(subscriber, _, service)| capacity(service, subscriber.buffer_size()))
                .fold(0, usize::saturating_add),
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
fn dead_node_ids() -> Result<Vec<iceoryx2::identifiers::UniqueNodeId>, PortError> {
    use iceoryx2::node::{NodeState, NodeView as _};

    let config = crate::domain::config()?;
    let mut dead = Vec::new();
    iceoryx2::node::Node::<Ipc>::list(config, |state| {
        if let NodeState::Dead(view) = state {
            dead.push(*view.id());
        }
        CallbackProgression::Continue
    })
    .map_err(crate::data_service::node_list_failed)?;
    Ok(dead)
}

pub(crate) fn steady_nanos() -> Result<u64, PortError> {
    u64::try_from(hypha_time::steady_now().nanos()).map_err(|_| PortError::Backend(3))
}

/// The streams `spec` may select, with their service names. The one stream of a literal topic
/// that a subscription names by its hash is known without a listing, which reads every
/// service of the domain; any other selection looks through the listing.
fn candidate_streams(
    spec: &hypha_core::port::TopicSpec,
) -> Result<Vec<(String, Stream)>, PortError> {
    match spec.schemas {
        SchemaScope::Stream(schema_hash) if !spec.name.contains('*') => {
            let stream = Stream { topic: spec.name.clone(), schema_hash };
            Ok(vec![(stream.service_name(), stream)])
        }
        SchemaScope::Own | SchemaScope::Every | SchemaScope::Stream(_) => {
            crate::discovery::topic_streams()
        }
    }
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
    for (name, stream) in candidate_streams(spec)? {
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
            |node_ref| crate::data_service::data_service_builder(node_ref, &service_name).open(),
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
