//! The session: it opens services and makes endpoints on them.

use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;

use hypha_core::port::{
    PayloadCapacity, PortError, QosPolicy, QosRequest, SchemaScope, TopicSpec,
    is_valid_topic_pattern,
};
use iceoryx2::port::notifier::Notifier;
use iceoryx2::prelude::*;

use crate::domain::MAX_BORROWED_SAMPLES;
use crate::endpoint::{Iox2Publisher, Iox2Subscriber};
use crate::header::Iox2FrameHeader;
use crate::latch::LatchedPort;
use crate::notification::create_notifier;
use crate::qos;
use crate::stream::Stream;

/// The thread-safe service variant, so an endpoint can be served on another thread.
pub(crate) type Ipc = iceoryx2::service::ipc_threadsafe::Service;

/// The first buffer size of a growable publisher that names none.
const GROWABLE_INITIAL_PAYLOAD: usize = 4096;

/// The one domain-wide event, notified whenever a subscriber is created.
///
/// A latched publisher hands its history to new subscribers when this event fires.
pub const GRAPH_EVENT: &str = "hypha/hk/graph/v1";

pub(crate) type SharedNode = Arc<Mutex<Option<Node<Ipc>>>>;

/// A session on the shared segment.
pub struct Iox2Transport {
    pub(crate) node: SharedNode,
    /// The latched publishers of this session.
    latched: Mutex<Vec<LatchedPort>>,
    /// Wakes the graph listeners when a subscriber is created. Opened on first use.
    graph_notifier: Mutex<Option<Arc<Notifier<Ipc>>>>,
}

impl Iox2Transport {
    /// Open a session under a generated node name.
    ///
    /// # Errors
    ///
    /// Returns [`PortError`] if the node cannot be registered in the iceoryx2 registry.
    pub fn new() -> Result<Self, PortError> {
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let name = format!("node_{pid}_{nanos}");
        Self::with_name(&name)
    }

    /// Open a named session.
    ///
    /// # Errors
    ///
    /// Returns [`PortError`] if the node cannot be registered with `name`.
    pub fn with_name(name: &str) -> Result<Self, PortError> {
        let config = crate::domain::config()?;
        let node_name: NodeName = name.try_into().map_err(|_| PortError::InvalidTopic)?;
        // A node reclaims its own name's dead run, so a restart frees its ports.
        reclaim_dead_nodes(config, &node_name)?;
        let raw_node = NodeBuilder::new()
            .config(config)
            .name(&node_name)
            .signal_handling_mode(SignalHandlingMode::Disabled)
            .create::<Ipc>()
            .map_err(|e| {
                log::error!(target: "hypha/transport", "Iox2Transport::with_name: NodeBuilder::create failed: {e:?}");
                PortError::Backend(0)
            })?;
        let node = Arc::new(Mutex::new(Some(raw_node)));
        Ok(Self { node, latched: Mutex::new(Vec::new()), graph_notifier: Mutex::new(None) })
    }

    /// Open a listener on the domain graph event, [`GRAPH_EVENT`].
    ///
    /// A caller-owned `WaitSet` attaches this to call [`Self::replay_latched`].
    ///
    /// # Errors
    /// Returns [`PortError`] when the event service cannot be opened.
    pub fn graph_listener(&self) -> Result<iceoryx2::port::listener::Listener<Ipc>, PortError> {
        crate::notification::create_listener(&self.node, GRAPH_EVENT, None)
    }

    /// Watch the graph event without a `WaitSet`: a reader of the domain that wants to learn
    /// of a new stream when it appears asks the watch whether an endpoint came or went.
    ///
    /// # Errors
    /// Returns [`PortError`] when the event service cannot be opened.
    pub fn graph_watch(&self) -> Result<GraphWatch, PortError> {
        self.graph_listener().map(|listener| GraphWatch { listener })
    }

    /// Hand the last sample of each latched topic to the subscribers that joined
    /// since the last call, and wake them.
    pub fn replay_latched(&self) {
        let mut latched = crate::locked(&self.latched);
        latched.retain_mut(LatchedPort::replay);
    }

    /// Lists data services with their current endpoint counts.
    ///
    /// # Errors
    /// Returns a backend error when the service registry cannot be read.
    pub fn list_service_ports(&self) -> Result<Vec<crate::DiscoveredService>, PortError> {
        crate::list_service_ports(&self.node)
    }

    /// Ask the topic's schema service and wait at most `timeout`.
    ///
    /// # Errors
    /// Returns [`PortError`] when the transport itself failed.
    pub fn schema_lookup(
        &self,
        topic: &str,
        wanted: hypha_core::port::SchemaSelector,
        timeout: StdDuration,
    ) -> Result<Option<Vec<u8>>, PortError> {
        crate::schema::lookup_schema(topic, wanted, timeout)
    }

    /// Create a caller-owned `WaitSet` on this transport's node.
    ///
    /// This call starts no thread.
    ///
    /// # Errors
    /// Returns [`PortError`] when the control event service cannot be opened.
    pub fn waitset(&self, control_service: &str) -> Result<crate::waitset::Iox2WaitSet, PortError> {
        crate::waitset::Iox2WaitSet::new(&self.node, control_service)
    }
}

impl Iox2Transport {
    /// Create a publisher for `spec`.
    ///
    /// # Errors
    /// [`PortError::InvalidTopic`] when the name is empty or refused, and the
    /// other [`PortError`] values that refuse a policy or a service.
    pub fn publish(&self, spec: &TopicSpec) -> Result<Iox2Publisher, PortError> {
        crate::stream::refuse_overlong(spec)?;
        if !spec.has_valid_public_name() {
            refuse_reserved(&spec.name);
            return Err(PortError::InvalidTopic);
        }
        if spec.loaned_samples == 0 {
            return Err(PortError::ResourceLimit);
        }
        if !spec.definition.is_empty() && !spec.has_valid_schema_identity() {
            return Err(PortError::InvalidTopic);
        }
        let stream = Stream::of(spec)?;
        let service_name: ServiceName =
            stream.service_name().as_str().try_into().map_err(|_| PortError::InvalidTopic)?;

        qos::check_request(&spec.name, &spec.qos)?;
        // Resolved before the service exists, so a refused publisher creates none.
        let capacity = qos::resolve_payload(spec)?;
        let bounded_deployment = crate::domain::bounded_memory()?;
        if bounded_deployment && capacity == PayloadCapacity::Growable {
            log::error!(target: "hypha/transport",
                "hypha_iceoryx2: topic '{}': a growable publisher is refused in a bounded deployment; set \
                 maxPayloadDynamic or a policy limit", spec.name);
            return Err(PortError::ResourceLimit);
        }
        let port_name = node_port_name(&self.node)?;
        // Declared before the service, so on an early return it drops after it.
        let schema_release = crate::schema::SchemaRelease::new(&self.node, &stream);
        let opened = open_data_service(&self.node, &service_name, &spec.name, &spec.qos, true)?;
        let policy = opened.policy;
        // A bounded publisher has one fixed data segment; a growable one adds segments on demand.
        let (allocation, initial_payload, max_payload) = match capacity {
            PayloadCapacity::Bounded(limit) => (AllocationStrategy::Static, limit, Some(limit)),
            PayloadCapacity::Growable => {
                let initial = match spec.initial_payload {
                    0 => GROWABLE_INITIAL_PAYLOAD,
                    bytes => bytes,
                };
                (AllocationStrategy::PowerOfTwo, initial, None)
            }
        };
        let service = Arc::new(opened.service);

        let build_publisher = || {
            service
                .publisher_builder()
                .name(&port_name)
                .allocation_strategy(allocation)
                .initial_max_slice_len(initial_payload)
                .max_loaned_samples(spec.loaned_samples)
                .create()
        };
        // No process reclaims the ports of dead nodes, so a full topic stays full.
        let publisher = build_publisher().map_err(|cause| {
            if cause == iceoryx2::port::publisher::PublisherCreateError::ExceedsMaxSupportedPublishers {
                PortError::ResourceLimit
            } else {
                log::error!(target: "hypha/transport", "hypha_iceoryx2: cannot create data publisher: {cause:?}");
                PortError::Backend(3)
            }
        })?;
        match max_payload {
            Some(payload) => {
                let subscribers = qos::subscriber_capacity(&spec.name, &policy);
                let samples = qos::pool_samples(&policy, subscribers, spec.loaned_samples);
                crate::shm_capacity::check(&spec.name, samples, payload, bounded_deployment)?;
            }
            None => log::info!(target: "hypha/transport",
                "hypha_iceoryx2: topic '{}' has a growable payload; its later segments are not checked against \
                 /dev/shm, and an undersized /dev/shm can stop the process with SIGBUS", spec.name),
        }
        let publisher = Arc::new(publisher);
        crate::schema::write_schema_file(spec)?;

        // Every publisher notifies; a scheduled subscriber has no listener to signal.
        let notifier = Arc::new(create_notifier(
            &self.node,
            service_name.as_str(),
            Some(crate::notification::EventLimits::of_topic(&service)),
        )?);

        if spec.latch {
            let mut latched = crate::locked(&self.latched);
            latched.push(LatchedPort {
                publisher: Arc::downgrade(&publisher),
                notifier: Arc::clone(&notifier),
                topic: spec.name.clone(),
            });
        }

        let graph = GraphAnnouncement(self.graph_notifier().ok());
        graph.announce();
        Ok(Iox2Publisher {
            publisher,
            service,
            topic: spec.name.clone(),
            topic_code: hypha_wire::frame::TopicCode::of(&spec.name),
            notifier,
            latched: spec.latch,
            max_payload,
            pool_refusals: std::sync::Mutex::new(crate::refusal_episode::RefusalEpisode::default()),
            pool_refusing: std::sync::atomic::AtomicBool::new(false),
            _schema_release: schema_release,
            _graph: graph,
        })
    }
}

/// A listener of the graph event that is read on the caller's own thread.
pub struct GraphWatch {
    listener: iceoryx2::port::listener::Listener<Ipc>,
}

impl GraphWatch {
    /// Whether an endpoint came or went since the last call. It takes the pending
    /// notifications and waits for none: one non-blocking read.
    ///
    /// # Errors
    /// [`PortError::Disconnected`] when the listener cannot be read.
    pub fn changed(&self) -> Result<bool, PortError> {
        self.listener.try_wait(|_| {}).map(|events| events > 0).map_err(|cause| {
            log::error!(target: "hypha/transport", "hypha_iceoryx2: failed to read the graph event: {cause:?}");
            PortError::Disconnected
        })
    }
}

impl GraphWatch {
    /// Waits up to `timeout` for an endpoint to come or go, and takes the notifications. True
    /// when one came: the thread sleeps in the kernel meanwhile.
    ///
    /// # Errors
    /// [`PortError::Disconnected`] when the listener cannot be read.
    pub fn wait(&self, timeout: StdDuration) -> Result<bool, PortError> {
        self.listener.timed_wait(|_| {}, timeout).map(|events| events > 0).map_err(|cause| {
            log::error!(target: "hypha/transport", "hypha_iceoryx2: failed to wait for the graph event: {cause:?}");
            PortError::Disconnected
        })
    }
}

/// Notifies the graph event once when made and once when dropped, so the domain's nodes
/// see a publisher come and go. A failed notification is logged; the backstop of the
/// listener's node then covers it.
pub(crate) struct GraphAnnouncement(Option<Arc<Notifier<Ipc>>>);

impl GraphAnnouncement {
    fn announce(&self) {
        let notified = self.0.as_ref().is_some_and(|notifier| notifier.notify().is_ok());
        if !notified {
            log::warn!(target: "hypha/transport", "hypha_iceoryx2: an endpoint changed but the graph event failed");
        }
    }
}

impl Drop for GraphAnnouncement {
    fn drop(&mut self) {
        self.announce();
    }
}

/// The port name matching a node's registration name.
pub(crate) fn node_port_name(
    node: &SharedNode,
) -> Result<iceoryx2::port::port_name::PortName, PortError> {
    let locked_node = crate::locked(node);
    let node_ref = locked_node.as_ref().ok_or(PortError::Backend(1))?;
    iceoryx2::port::port_name::PortName::new(node_ref.name().as_str())
        .map_err(|_| PortError::InvalidTopic)
}

/// Log why a topic name under `/hypha/` is refused; other invalid names need no line.
fn refuse_reserved(topic: &str) {
    if hypha_core::port::is_reserved_topic_name(topic) {
        log::error!(target: "hypha/transport",
            "hypha: topic '{topic}': names under /hypha/ are reserved for the framework (its \
             services live there); choose another namespace");
    }
}

/// The publish-subscribe service of one topic.
pub(crate) type DataPortFactory =
    iceoryx2::service::port_factory::publish_subscribe::PortFactory<Ipc, [u8], Iox2FrameHeader>;

struct DataService {
    service: DataPortFactory,
    policy: QosPolicy,
}

/// How long a starting node waits for another process reclaiming the same dead node.
pub(crate) const RECLAIM_TIMEOUT: StdDuration = StdDuration::from_secs(1);

/// Remove the resources of the dead nodes a node called `node_name` reclaims
/// when it starts: its own crashed runs and every dead CLI tool node
/// (`_hyphacli_<tool>`). A dead CLI tool node that cannot be reclaimed is logged and the
/// node still starts.
///
/// # Errors
/// [`PortError::Backend`] with [`BACKEND_RECLAIM_FAILED`] when the node's own dead run
/// cannot be reclaimed, or with code 5 when the node registry cannot be read.
fn reclaim_dead_nodes(
    config: &iceoryx2::config::Config,
    node_name: &NodeName,
) -> Result<(), PortError> {
    use iceoryx2::node::{DeadNodeView, NodeState, NodeView as _};
    use iceoryx2::prelude::CallbackProgression;

    let mut dead_nodes: Vec<(DeadNodeView<Ipc>, String, bool)> = Vec::new();
    iceoryx2::node::Node::<Ipc>::list(config, |state| {
        let NodeState::Dead(view) = state else {
            return CallbackProgression::Continue;
        };
        let Some(dead_name) =
            view.details().as_ref().map(|details| details.name().as_str().to_owned())
        else {
            return CallbackProgression::Continue;
        };
        let own_run = dead_name == node_name.as_str();
        let reclaimable = own_run || hypha_core::port::is_cli_node_name(&dead_name);
        if reclaimable {
            dead_nodes.push((view, dead_name, own_run));
        }
        CallbackProgression::Continue
    })
    .map_err(node_list_failed)?;
    for (dead_node, dead_name, own_run) in dead_nodes {
        let pid = match dead_node.id().pid::<Ipc>() {
            Ok(pid) => pid.value().to_string(),
            Err(_) => "unknown".to_string(),
        };
        let dead_node_id = *dead_node.id();
        match dead_node.blocking_remove_stale_resources(RECLAIM_TIMEOUT) {
            Ok(()) => {
                log::info!(target: "hypha/transport", "hypha: node '{}' reclaimed the dead node '{dead_name}' of pid {pid}",
                        node_name.as_str());
            }
            Err(cause) => {
                let still_dead = is_still_dead(config, &dead_node_id)?;
                if !still_dead {
                    log::info!(target: "hypha/transport", "hypha: node '{}' finds the dead node '{dead_name}' of pid {pid} \
                             already reclaimed by another process", node_name.as_str());
                    continue;
                }
                if own_run {
                    log::error!(target: "hypha/transport", "hypha: node '{}' cannot reclaim its own dead run, of pid {pid}: \
                             {cause:?}; the run still holds the name's ports, so the node does not start. Run \
                             `hypha clean` with the domain stopped", node_name.as_str());
                    return Err(PortError::Backend(BACKEND_RECLAIM_FAILED));
                }
                log::error!(target: "hypha/transport", "hypha: node '{}' cannot reclaim the dead node '{dead_name}' of pid \
                         {pid}: {cause:?}; it stays listed as dead and holds its ports until `hypha clean` \
                         removes the stopped domain", node_name.as_str());
            }
        }
    }
    Ok(())
}

/// Log a failed listing of the domain's nodes, and return its error: code 5.
pub(crate) fn node_list_failed(cause: impl core::fmt::Debug) -> PortError {
    log::error!(target: "hypha/transport", "hypha_iceoryx2: cannot list the nodes of the domain: {cause:?}");
    PortError::Backend(5)
}

/// Backend code of a node whose own dead run cannot be reclaimed, so it does not start.
pub(crate) const BACKEND_RECLAIM_FAILED: u32 = 41;

/// True while the registry lists the node `dead_node_id` as dead.
fn is_still_dead(
    config: &iceoryx2::config::Config,
    dead_node_id: &iceoryx2::identifiers::UniqueNodeId,
) -> Result<bool, PortError> {
    use iceoryx2::node::{NodeState, NodeView as _};
    use iceoryx2::prelude::CallbackProgression;

    let mut still_dead = false;
    iceoryx2::node::Node::<Ipc>::list(config, |state| {
        if let NodeState::Dead(view) = &state
            && view.id() == dead_node_id
        {
            still_dead = true;
        }
        CallbackProgression::Continue
    })
    .map_err(node_list_failed)?;
    Ok(still_dead)
}

/// Open the data service `service_name` of `topic`, or create it when `may_create`.
///
///
/// An endpoint that declares a policy must find it in the service's stamp, or is
/// refused; one that declares nothing adopts it. Only a publisher, or a subscriber
/// that declares a policy or reads a framework topic, creates it.
fn open_data_service(
    node: &SharedNode,
    service_name: &ServiceName,
    topic: &str,
    request: &QosRequest,
    may_create: bool,
) -> Result<DataService, PortError> {
    use iceoryx2::service::builder::publish_subscribe::{
        PublishSubscribeOpenError as Open, PublishSubscribeOpenOrCreateError as OpenOrCreate,
    };
    use iceoryx2::service::port_factory::PortFactory as _;

    let creation = qos::creation_policy(topic, request);
    let stamp = qos::stamp_specifier(&creation)?;
    let subscribers = qos::subscriber_capacity(topic, &creation);
    let opened = crate::service_open::open_within_window(
        "the data service",
        node,
        PortError::Backend(1),
        |node_ref| {
            open_or_create_data_service(
                node_ref,
                service_name,
                &creation,
                &stamp,
                may_create,
                subscribers,
            )
        },
        crate::service_open::publish_subscribe_is_transient,
    )?;
    let service = match opened {
        Ok(service) => service,
        Err(cause) => {
            let absent_and_not_created =
                cause == OpenOrCreate::PublishSubscribeOpenError(Open::DoesNotExist);
            if !absent_and_not_created {
                log::error!(target: "hypha/transport", "hypha_iceoryx2: cannot open data service '{}': {cause:?}{}",
                        service_name.as_str(),
                        crate::service_open::half_created_advice(crate::service_open::hangs_in_creation(cause)));
            }
            return Err(crate::service_open::publish_subscribe_port_error(cause));
        }
    };
    let Ok(stamped) = qos::policy_of_service(service.attributes()) else {
        log::error!(target: "hypha/transport", "hypha_iceoryx2: data service '{}' names no QoS policy of this hypha build \
                 in its attribute {}: another hypha build or a stale segment made it (run `hypha clean` \
                 with the domain stopped)", service_name.as_str(), qos::QOS_ATTRIBUTE);
        return Err(PortError::QosMismatch);
    };
    qos::check_stamped(topic, request, &stamped)?;
    Ok(DataService { service, policy: stamped })
}

/// The builder of a topic's data service: raw bytes behind a hypha frame header.
pub(crate) fn data_service_builder(
    node_ref: &iceoryx2::node::Node<Ipc>,
    service_name: &ServiceName,
) -> iceoryx2::service::builder::publish_subscribe::Builder<[u8], Iox2FrameHeader, Ipc> {
    let service = node_ref.service_builder(service_name);
    let raw = service.publish_subscribe::<[u8]>();
    raw.user_header::<Iox2FrameHeader>()
}

/// One attempt of [`open_data_service`]. A service another endpoint creates in
/// between is `AlreadyExists`, which the caller's retry turns into an open.
fn open_or_create_data_service(
    node_ref: &iceoryx2::node::Node<Ipc>,
    service_name: &ServiceName,
    policy: &QosPolicy,
    stamp: &iceoryx2::service::attribute::AttributeSpecifier,
    may_create: bool,
    subscribers: usize,
) -> Result<
    DataPortFactory,
    iceoryx2::service::builder::publish_subscribe::PublishSubscribeOpenOrCreateError,
> {
    use iceoryx2::service::builder::publish_subscribe::{
        PublishSubscribeOpenError as Open, PublishSubscribeOpenOrCreateError as OpenOrCreate,
    };

    let opened = data_service_builder(node_ref, service_name).open();
    let cause = match opened {
        Ok(service) => return Ok(service),
        Err(cause) => cause,
    };
    let creating = may_create && cause == Open::DoesNotExist;
    if !creating {
        return Err(OpenOrCreate::PublishSubscribeOpenError(cause));
    }
    let builder = data_service_builder(node_ref, service_name);
    let builder = builder.max_publishers(policy.max_publishers);
    let builder = builder.max_subscribers(subscribers);
    let builder = builder.max_nodes(qos::max_nodes(policy, subscribers));
    let builder = builder.subscriber_max_buffer_size(policy.max_depth);
    let builder = builder.subscriber_max_borrowed_samples(MAX_BORROWED_SAMPLES);
    let builder = builder.history_size(qos::HISTORY_SIZE);
    builder.create_with_attributes(stamp).map_err(OpenOrCreate::PublishSubscribeCreateError)
}

impl Iox2Transport {
    /// Create a subscriber for `spec`.
    ///
    ///
    /// A subscriber reads the stream of its own schema. A name with `*`, or a spec that
    /// reads every schema, follows every matching stream that exists now, not later ones. A
    /// spec that reads one stream by its hash opens that service without listing the domain.
    ///
    /// # Errors
    /// [`PortError::InvalidTopic`] when the name is empty or refused. The other
    /// [`PortError`] values refuse a policy or a service.
    pub fn subscribe(&self, spec: &TopicSpec) -> Result<Iox2Subscriber, PortError> {
        if !spec.is_valid() {
            return Err(PortError::InvalidTopic);
        }
        crate::stream::refuse_overlong(spec)?;
        let is_pattern = spec.name.contains('*');
        if is_pattern || spec.schemas != SchemaScope::Own {
            let is_valid_name = if is_pattern {
                is_valid_topic_pattern(&spec.name)
            } else {
                spec.has_valid_public_name()
            };
            if !is_valid_name {
                return Err(PortError::InvalidTopic);
            }
            qos::check_request(&spec.name, &spec.qos)?;
            let subscriber = Iox2Subscriber::new_set(&self.node, spec)?;
            self.announce_subscriber();
            return Ok(subscriber);
        }
        if !spec.has_valid_public_name() {
            refuse_reserved(&spec.name);
            return Err(PortError::InvalidTopic);
        }
        if !spec.definition.is_empty() && !spec.has_valid_schema_identity() {
            return Err(PortError::InvalidTopic);
        }
        qos::check_request(&spec.name, &spec.qos)?;
        let port_name = node_port_name(&self.node)?;
        let stream = Stream::of(spec)?;
        let service_name: ServiceName =
            stream.service_name().as_str().try_into().map_err(|_| PortError::InvalidTopic)?;

        // Declared before the service, so on an early return it drops after it.
        let schema_release = crate::schema::SchemaRelease::new(&self.node, &stream);
        let may_create = qos::subscriber_may_create(&spec.name, &spec.qos);
        let opened =
            open_data_service(&self.node, &service_name, &spec.name, &spec.qos, may_create)?;
        let slots = match &spec.qos {
            QosRequest::Declared(_) => qos::declared_slots(&spec.name, &opened.policy),
            QosRequest::Undeclared => qos::observer_slots(&spec.name),
        };
        let service = opened.service;
        // Taken before the subscriber exists, so a live sample is never older.
        let created_nanos = crate::endpoint::steady_nanos()?;

        let subscriber = crate::subscriber_port::create_subscriber(
            &service,
            &spec.name,
            spec.buffer_size,
            &port_name,
            slots,
        )?;
        // A typed subscriber lets the domain resolve its schema even for a relayed publisher.
        crate::schema::write_schema_file(spec)?;

        let subscriber = Iox2Subscriber::new(
            subscriber,
            created_nanos,
            service,
            stream,
            self.node.clone(),
            schema_release,
        );
        self.announce_subscriber();
        Ok(subscriber)
    }

    /// Notify the graph event after a subscriber exists.
    ///
    /// A failed notification is logged; a latched sample then waits for the next graph event.
    fn announce_subscriber(&self) {
        if let Err(cause) = self.notify_graph_change() {
            log::warn!(target: "hypha/transport", "hypha_iceoryx2: subscriber created but the graph event failed: {cause:?}");
        }
    }

    /// Notify the graph event: a subscriber or a service endpoint came or went.
    pub(crate) fn notify_graph_change(&self) -> Result<(), PortError> {
        self.graph_notifier()?.notify().map(drop).map_err(|_| PortError::Disconnected)
    }

    /// The notifier of the graph event, opened on first use.
    fn graph_notifier(&self) -> Result<Arc<Notifier<Ipc>>, PortError> {
        let cached = {
            let guard = crate::locked(&self.graph_notifier);
            guard.as_ref().map(Arc::clone)
        };
        // Opened without any lock held: the open may wait for another endpoint.
        let notifier = if let Some(notifier) = cached {
            notifier
        } else {
            let created = Arc::new(create_notifier(&self.node, GRAPH_EVENT, None)?);
            let mut guard = crate::locked(&self.graph_notifier);
            Arc::clone(guard.get_or_insert(created))
        };
        Ok(notifier)
    }
}

impl Drop for Iox2Transport {
    fn drop(&mut self) {
        let _ = crate::locked(&self.graph_notifier).take();
        crate::locked(&self.latched).clear();
        let _ = crate::locked(&self.node).take();
    }
}
