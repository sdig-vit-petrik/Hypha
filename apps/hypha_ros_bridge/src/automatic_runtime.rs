//! Runtime for automatic same-name ROS 1 and hypha routing.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use hypha_node::sdk::{
    BusNode, FollowError, PublisherOptions, RawPublisher, ReceivedSample, ResolvedSchema,
    StreamFollower, StreamKey, StreamListings, StreamSelector, TopicInfo,
};
use roslibrust::ros1::{NodeHandle, PublisherAny};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::automatic::RosTopicState;
use crate::automatic::{
    AutoFlow, HyphaTopicState, RecentSample, RecentSamples, desired_flows, topic_states,
};
use crate::bridged_streams::{Admission, BridgedStreams, not_bridged_warning};
use crate::error::BridgeError;
use crate::publisher_identity::{RosIdentity, agreed_identity, probe_publisher};
use crate::ros_graph::{RosGraphClient, RosGraphSnapshot};
use crate::routing::{
    BridgeMsg, forward_from_ros, handoff_closed, is_latched_topic, publish_to_ros,
};
use crate::runtime::{BridgeConfig, schema_serving_waitset, serve_bus};
use hypha_node::sdk::SchemaIdentityCache;

const GRAPH_REFRESH: Duration = Duration::from_millis(500);
const TOPIC_REFRESH: Duration = Duration::from_millis(500);
/// Topics this bridge follows. Every hypha topic reaches ROS.
const BRIDGED_TOPICS: &str = "**";
/// Pending samples one followed stream may buffer: zero, the depth of the topic's `QoS`
/// policy depth (8, the `default` policy), which a larger depth would exceed.
const SUBSCRIPTION_DEPTH: usize = 0;
/// Samples taken from the bus per pass, so one busy stream cannot hold the loop open while
/// others wait.
const DRAIN_BUDGET_PER_PASS: usize = 512;
const RECENT_SAMPLE_LIMIT: usize = 256;
const EVENT_QUEUE_DEPTH: usize = 64;
/// How long the listing thread waits for the graph before it looks at its stop flag.
const LISTING_STOP_CHECK: Duration = Duration::from_millis(200);

enum AutoEvent {
    Topics(BTreeMap<String, HyphaTopicState>),
    Sample(BridgeMsg),
}

struct AutoBusRuntime {
    /// What following the streams last refused, so a refusal that lasts is reported once.
    follow_error: Option<String>,
    /// The one stream of each topic that reaches ROS.
    bridged: BridgedStreams,
    /// The topic each sample names for the ROS side, shared by its messages.
    topic_names: BTreeMap<String, Arc<str>>,
    publishers: BTreeMap<String, IngressPublisher>,
    recent: RecentSamples,
    topic_states: BTreeMap<String, HyphaTopicState>,
    ros_to_hypha_rx: mpsc::Receiver<BridgeMsg>,
    event_tx: mpsc::Sender<AutoEvent>,
    last_topic_refresh: Instant,
    schema_resolver: SchemaIdentityCache,
}

/// The hypha publisher of one ROS ingress topic and the identity it announces.
struct IngressPublisher {
    identity: Arc<ResolvedSchema>,
    publisher: RawPublisher,
}

/// Whether two identities are the same. Every sample of one route shares
/// one `Arc`, so the text is compared only when the identity was replaced.
fn same_identity(left: &Arc<ResolvedSchema>, right: &Arc<ResolvedSchema>) -> bool {
    Arc::ptr_eq(left, right) || left == right
}

impl AutoBusRuntime {
    fn new(ros_to_hypha_rx: mpsc::Receiver<BridgeMsg>, event_tx: mpsc::Sender<AutoEvent>) -> Self {
        Self {
            follow_error: None,
            bridged: BridgedStreams::default(),
            topic_names: BTreeMap::new(),
            publishers: BTreeMap::new(),
            recent: RecentSamples::new(RECENT_SAMPLE_LIMIT),
            topic_states: BTreeMap::new(),
            ros_to_hypha_rx,
            event_tx,
            last_topic_refresh: Instant::now(),
            schema_resolver: SchemaIdentityCache::new(),
        }
    }

    fn run(mut self, bus: BusNode, stop: Arc<AtomicBool>) -> Result<(), BridgeError> {
        let bus = Arc::new(bus);
        let mut waitset = schema_serving_waitset(&bus)?;
        // Every stream of every topic, each as the graph event shows it: a version that joins a
        // topic later is read too. A thread of the bridge lists them, so that a listing, tens of
        // milliseconds, never stops the loop that reads the bus.
        let mut follower = StreamFollower::new(
            vec![BRIDGED_TOPICS.to_owned()],
            StreamSelector::default(),
            SUBSCRIPTION_DEPTH,
        );
        let mut listings = StreamListings::new(&bus).map_err(BridgeError::Bus)?;
        let first = listings.list(&bus).map_err(BridgeError::Bus)?;
        self.follow(&bus, &mut follower, vec![Ok(first)]);
        let listed = ListingThread::spawn(&bus, listings);
        while bus.ok() && !stop.load(Ordering::Acquire) {
            serve_bus(&mut waitset)?;
            self.follow(&bus, &mut follower, listed.take());
            self.drain_ros_to_hypha(&bus, &stop);
            self.drain_hypha_to_ros(&bus, &mut follower, &stop)?;
            if self.last_topic_refresh.elapsed() >= TOPIC_REFRESH {
                self.publish_topic_snapshot(&bus, &stop)?;
                self.last_topic_refresh = Instant::now();
            }
        }
        Ok(())
    }

    fn publish_topic_snapshot(
        &mut self,
        bus: &BusNode,
        stop: &AtomicBool,
    ) -> Result<(), BridgeError> {
        // Each stream has its own flags: a name is published or subscribed when any of its
        // streams is, and a stream's choice for ROS ends with its publishers.
        let streams: BTreeMap<StreamKey, HyphaTopicState> = bus
            .list_topics()
            .map_err(BridgeError::Bus)?
            .iter()
            .map(|stream| {
                (
                    StreamKey::from(stream),
                    HyphaTopicState { published: stream.published, subscribed: stream.subscribed },
                )
            })
            .collect();
        self.bridged.retain_published(&streams);
        let topics = topic_states(&streams);
        self.topic_names.retain(|topic, _| topics.contains_key(topic));
        self.topic_states = topics.clone();
        self.publishers.retain(|topic, _| topics.get(topic).is_some_and(|state| state.subscribed));
        if stop.load(Ordering::Acquire) {
            return Ok(());
        }
        if self.event_tx.send(AutoEvent::Topics(topics)).is_err() {
            return handoff_closed(stop, "automatic topic discovery");
        }
        Ok(())
    }

    /// Follows the streams that joined the bus since the last pass. A refusal that lasts is
    /// logged once.
    fn follow(
        &mut self,
        bus: &BusNode,
        follower: &mut StreamFollower,
        listings: Vec<Result<Vec<TopicInfo>, String>>,
    ) {
        for listing in listings {
            let outcome = match listing {
                Ok(listing) => follower.apply(bus, &listing),
                Err(message) => Err(FollowError::Listing(message)),
            };
            let Err(error) = outcome else { continue };
            let text = error.to_string();
            if self.follow_error.as_ref() != Some(&text) {
                log::error!("automatic bridge cannot follow every stream; it will retry: {text}");
                self.follow_error = Some(text);
            }
        }
    }

    fn drain_ros_to_hypha(&mut self, bus: &BusNode, stop: &AtomicBool) {
        while let Ok(message) = self.ros_to_hypha_rx.try_recv() {
            if stop.load(Ordering::Acquire) {
                return;
            }
            let topic = &*message.topic;
            let has_external_subscriber =
                self.topic_states.get(topic).is_some_and(|state| state.subscribed);
            if !has_external_subscriber {
                continue;
            }
            let identity_changed = self
                .publishers
                .get(topic)
                .is_some_and(|existing| !same_identity(&existing.identity, &message.identity));
            if identity_changed {
                log::info!("{topic} changed its ROS identity; replacing its hypha publisher");
                self.publishers.remove(topic);
            }
            if !self.publishers.contains_key(topic) {
                // A ROS message can be of any size, so the publisher grows to it.
                let Ok(publisher) = bus.publish_raw_configured(
                    topic,
                    &message.identity.data_type,
                    &message.identity.md5,
                    &message.identity.definition,
                    PublisherOptions::default().growable(),
                ) else {
                    log::error!("automatic hypha publisher creation failed for {topic}");
                    continue;
                };
                let ingress =
                    IngressPublisher { identity: Arc::clone(&message.identity), publisher };
                self.publishers.insert(topic.to_owned(), ingress);
            }
            let Some(ingress) = self.publishers.get_mut(topic) else {
                log::error!("automatic hypha publisher disappeared for {topic}");
                continue;
            };
            if !ingress.publisher.publish(&message.bytes) {
                log::error!("hypha rejected an automatic ROS sample on {topic}");
                continue;
            }
            self.recent.remember(RecentSample {
                topic: message.topic,
                stamp_nanos: message.stamp_nanos,
                bytes: message.bytes,
            });
        }
    }

    fn drain_hypha_to_ros(
        &mut self,
        bus: &BusNode,
        follower: &mut StreamFollower,
        stop: &AtomicBool,
    ) -> Result<(), BridgeError> {
        for _ in 0..DRAIN_BUDGET_PER_PASS {
            let received = follower.next_sample().map_err(BridgeError::Bus)?;
            let Some(sample) = received else { break };
            self.forward_sample(bus, &sample, stop)?;
            if stop.load(Ordering::Acquire) {
                return Ok(());
            }
        }
        Ok(())
    }

    fn forward_sample(
        &mut self,
        bus: &BusNode,
        sample: &ReceivedSample,
        stop: &AtomicBool,
    ) -> Result<(), BridgeError> {
        if stop.load(Ordering::Acquire) {
            return Ok(());
        }
        let Some(identity) = sample.identity() else {
            log::debug!("ignoring automatic hypha sample without identity");
            return Ok(());
        };
        let topic = identity.topic;
        let Some(bytes) = sample.bytes() else {
            log::warn!("dropping malformed automatic hypha sample on {topic}");
            return Ok(());
        };
        // The bridge's own samples come first: they must not choose the stream of their topic.
        if self.recent.take(topic, identity.transport_sent_nanos, bytes) {
            return Ok(());
        }
        match self.bridged.admit(topic, identity.schema_hash) {
            Admission::Bridged => {}
            Admission::Dropped { bridged, report } => {
                if report {
                    self.report_unbridged(bus, topic, bridged, identity.schema_hash);
                }
                return Ok(());
            }
        }
        // A retained record always holds a type, a valid MD5 and a definition.
        let schema =
            match self.schema_resolver.resolve(bus, topic, identity.schema_hash, Instant::now()) {
                Ok(Some(schema)) => schema,
                Ok(None) => {
                    log::debug!("ignoring automatic sample on {topic} without a retained schema");
                    return Ok(());
                }
                Err(error) => {
                    log::warn!("automatic schema lookup failed on {topic}: {error}");
                    return Ok(());
                }
            };
        let topic = Arc::clone(
            self.topic_names.entry(topic.to_owned()).or_insert_with(|| Arc::from(topic)),
        );
        let message = BridgeMsg {
            route_id: None,
            topic,
            identity: schema,
            stamp_nanos: identity.transport_sent_nanos,
            bytes: bytes.to_vec(),
        };
        if self.event_tx.send(AutoEvent::Sample(message)).is_err() {
            return handoff_closed(stop, "automatic hypha handoff");
        }
        Ok(())
    }

    /// Says, once for the stream, that ROS 1 carries one type per topic and which of the
    /// topic's streams it is not given.
    fn report_unbridged(&self, bus: &BusNode, topic: &str, bridged: u64, dropped: u64) {
        let describe = |schema_hash: u64| match self.schema_resolver.resolve(
            bus,
            topic,
            schema_hash,
            Instant::now(),
        ) {
            Ok(Some(schema)) => {
                format!("{} ({})", schema.data_type, schema.md5.get(..8).unwrap_or(&schema.md5))
            }
            Ok(None) | Err(_) => format!("schema {schema_hash:016x}"),
        };
        log::warn!(
            "{topic} carries more than one stream and a ROS 1 topic carries one type: bridging \
             {}, the first; not bridging {}",
            describe(bridged),
            describe(dropped)
        );
    }
}

/// A thread of the bridge that lists the streams when the graph changes and hands the listings
/// to the bus loop.
struct ListingThread {
    listings: mpsc::Receiver<Result<Vec<TopicInfo>, String>>,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl ListingThread {
    fn spawn(bus: &Arc<BusNode>, mut listings: StreamListings) -> Self {
        let (sender, receiver) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let handle = {
            let (bus, stop) = (Arc::clone(bus), Arc::clone(&stop));
            thread::spawn(move || {
                while !stop.load(Ordering::Acquire) {
                    let outcome = listings.wait_for_listing(&bus, LISTING_STOP_CHECK);
                    let failed = outcome.is_err();
                    let Some(outcome) = outcome.transpose() else { continue };
                    if sender.send(outcome).is_err() {
                        return;
                    }
                    if failed {
                        thread::sleep(Duration::from_secs(1));
                    }
                }
            })
        };
        Self { listings: receiver, stop, handle: Some(handle) }
    }

    /// The listings made since the last call.
    fn take(&self) -> Vec<Result<Vec<TopicInfo>, String>> {
        self.listings.try_iter().collect()
    }
}

impl Drop for ListingThread {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Runs automatic discovery and same-name route reconciliation.
///
/// # Errors
/// Returns an error when discovery, endpoint creation, or worker shutdown fails.
pub(crate) async fn run(
    bus: BusNode,
    ros: NodeHandle,
    config: &BridgeConfig,
    stop: Arc<AtomicBool>,
) -> Result<(), BridgeError> {
    let (ros_to_hypha_tx, ros_to_hypha_rx) = mpsc::channel::<BridgeMsg>();
    let (event_tx, event_rx) = mpsc::channel::<AutoEvent>();
    let bus_runtime = AutoBusRuntime::new(ros_to_hypha_rx, event_tx);
    let bus_thread_stop = Arc::clone(&stop);
    let bus_thread = thread::spawn(move || bus_runtime.run(bus, bus_thread_stop));

    let result = run_ros(
        ros,
        &config.master_uri,
        &config.node_name,
        ros_to_hypha_tx,
        event_rx,
        Arc::clone(&stop),
    )
    .await;
    stop.store(true, Ordering::Release);
    // A join error follows only a panic of the bus thread: it goes on here.
    let bus_result = bus_thread.join().unwrap_or_else(|payload| std::panic::resume_unwind(payload));
    result?;
    bus_result
}

async fn run_ros(
    ros: NodeHandle,
    master_uri: &str,
    node_name: &str,
    ros_to_hypha_tx: mpsc::Sender<BridgeMsg>,
    event_rx: mpsc::Receiver<AutoEvent>,
    stop: Arc<AtomicBool>,
) -> Result<(), BridgeError> {
    let graph_client = RosGraphClient::new(master_uri, node_name);
    let mut graph = graph_client.snapshot().await.map_err(BridgeError::RosConnection)?;
    let (mut events, forwarder) = spawn_event_forwarder(event_rx, Arc::clone(&stop));
    let mut hypha_topics = BTreeMap::new();
    let mut ingress = RosIngressSet::default();
    let mut publishers: BTreeMap<String, PublisherAny> = BTreeMap::new();
    let mut publisher_identity: BTreeMap<String, Arc<ResolvedSchema>> = BTreeMap::new();
    // The other types already told of, per topic, so each is told once while it is advertised.
    let mut not_bridged_said: BTreeSet<(String, String)> = BTreeSet::new();
    let mut graph_tick = tokio::time::interval(GRAPH_REFRESH);

    ingress.reconcile(&ros, &graph_client, &graph, &hypha_topics, &ros_to_hypha_tx, &stop).await?;
    loop {
        if stop.load(Ordering::Acquire) {
            break;
        }
        tokio::select! {
            event = events.recv() => {
                match event {
                    Some(AutoEvent::Topics(topics)) => {
                        hypha_topics = topics;
                        ingress.reconcile(
                            &ros,
                            &graph_client,
                            &graph,
                            &hypha_topics,
                            &ros_to_hypha_tx,
                            &stop,
                        ).await?;
                        retain_publishers(
                            &mut publishers,
                            &mut publisher_identity,
                            &mut not_bridged_said,
                            &hypha_topics,
                        );
                    }
                    Some(AutoEvent::Sample(message)) => {
                        forward_to_ros(
                            &ros,
                            &graph,
                            &hypha_topics,
                            &mut publishers,
                            &mut publisher_identity,
                            &mut not_bridged_said,
                            message,
                        ).await?;
                    }
                    None => {}
                }
            }
            _ = graph_tick.tick() => {
                // A master that is briefly unreachable must not stop routing.
                // The previous snapshot stays in use until a poll succeeds.
                match graph_client.snapshot().await {
                    Ok(fresh) => graph = fresh,
                    Err(error) => {
                        log::warn!("ROS master graph poll failed, keeping the last graph: {error}");
                        continue;
                    }
                }
                ingress.reconcile(
                    &ros,
                    &graph_client,
                    &graph,
                    &hypha_topics,
                    &ros_to_hypha_tx,
                    &stop,
                ).await?;
                retain_publishers(
                            &mut publishers,
                            &mut publisher_identity,
                            &mut not_bridged_said,
                            &hypha_topics,
                        );
            }
        }
    }
    ingress.abort_all();
    events.close();
    let _ = forwarder.await;
    drop(ros_to_hypha_tx);
    Ok(())
}

/// Moves events from the bus thread onto a cancel-safe asynchronous channel.
///
/// The blocking receive lives in one task: inside `select!`, a losing arm would
/// discard an event that the blocking call had already taken.
fn spawn_event_forwarder(
    event_rx: mpsc::Receiver<AutoEvent>,
    stop: Arc<AtomicBool>,
) -> (tokio::sync::mpsc::Receiver<AutoEvent>, tokio::task::JoinHandle<()>) {
    let (async_tx, async_rx) = tokio::sync::mpsc::channel::<AutoEvent>(EVENT_QUEUE_DEPTH);
    let forwarder = tokio::task::spawn_blocking(move || {
        while !stop.load(Ordering::Acquire) {
            match event_rx.recv_timeout(crate::routing::ROS_RECEIVE_TIMEOUT) {
                Ok(event) => {
                    if async_tx.blocking_send(event).is_err() {
                        return;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
    });
    (async_rx, forwarder)
}

fn retain_publishers(
    publishers: &mut BTreeMap<String, PublisherAny>,
    publisher_identity: &mut BTreeMap<String, Arc<ResolvedSchema>>,
    not_bridged_said: &mut BTreeSet<(String, String)>,
    hypha_topics: &BTreeMap<String, HyphaTopicState>,
) {
    publishers.retain(|topic, _| {
        let Some(hypha_state) = hypha_topics.get(topic) else {
            return false;
        };
        hypha_state.published
    });
    publisher_identity.retain(|topic, _| publishers.contains_key(topic));
    not_bridged_said.retain(|(topic, _)| publishers.contains_key(topic));
}

/// Automatic ROS 1 ingress of one topic.
struct RosIngress {
    /// Publisher URIs that the current identity was read from.
    publisher_uris: BTreeSet<String>,
    /// Identity that the ingress task gives every sample; `None` drops samples.
    identity: watch::Sender<Option<Arc<ResolvedSchema>>>,
    task: JoinHandle<Result<(), BridgeError>>,
}

/// Every automatic ROS 1 ingress, keyed by topic.
#[derive(Default)]
struct RosIngressSet {
    routes: BTreeMap<String, RosIngress>,
    /// Last identity failure reported per topic, so a retry does not repeat it.
    probe_failures: BTreeMap<String, String>,
}

impl RosIngressSet {
    /// Opens ingress for each topic with a ROS publisher and an hypha
    /// subscriber, and closes ingress that is no longer wanted.
    ///
    /// A topic is routed only after every one of its publishers reported the
    /// same identity; a changed set of publisher URIs reads the identity again.
    async fn reconcile(
        &mut self,
        ros: &NodeHandle,
        graph_client: &RosGraphClient,
        graph: &RosGraphSnapshot,
        hypha_topics: &BTreeMap<String, HyphaTopicState>,
        sender: &mpsc::Sender<BridgeMsg>,
        stop: &Arc<AtomicBool>,
    ) -> Result<(), BridgeError> {
        let desired: BTreeSet<String> = graph
            .topics
            .iter()
            .filter(|(topic, state)| {
                let Some(hypha_state) = hypha_topics.get(*topic) else {
                    return false;
                };
                desired_flows(state, hypha_state).contains(&AutoFlow::RosToHypha)
            })
            .map(|(topic, _)| topic.clone())
            .collect();
        self.routes.retain(|topic, ingress| {
            let keep = desired.contains(topic) && !ingress.task.is_finished();
            if !keep {
                ingress.task.abort();
            }
            keep
        });
        self.probe_failures.retain(|topic, _| desired.contains(topic));

        // One lookupNode per publisher node on every graph poll;
        // the master gives no other sign that a node restarted under its name.
        let mut node_uris: BTreeMap<String, String> = BTreeMap::new();
        for topic in desired {
            let Some(state) = graph.topics.get(&topic) else {
                continue;
            };
            let publisher_uris =
                match lookup_publisher_uris(graph_client, state, &mut node_uris).await {
                    Ok(uris) => uris,
                    Err(error) => {
                        log::warn!("cannot locate the ROS publishers of {topic}: {error}");
                        continue;
                    }
                };
            let unchanged =
                self.routes.get(&topic).is_some_and(|route| route.publisher_uris == publisher_uris);
            if unchanged {
                continue;
            }
            if let Some(route) = self.routes.get(&topic) {
                route.identity.send_replace(None);
            }
            let identity = match probe_topic(graph_client, &topic, state, &publisher_uris).await {
                Ok(identity) => identity,
                Err(error) => {
                    self.report_probe_failure(&topic, &error);
                    continue;
                }
            };
            self.probe_failures.remove(&topic);
            log::info!("{topic} is routed as {} {}", identity.type_name, identity.md5);
            let identity = Arc::new(ResolvedSchema {
                data_type: identity.type_name,
                md5: identity.md5,
                definition: identity.definition,
            });
            if let Some(route) = self.routes.get_mut(&topic) {
                route.identity.send_replace(Some(identity));
                route.publisher_uris = publisher_uris;
                continue;
            }
            let subscriber =
                ros.subscribe_any(&topic, 8).await.map_err(BridgeError::ros_connection)?;
            let (identity_tx, identity_rx) = watch::channel(Some(identity));
            let task = tokio::spawn(forward_from_ros(
                None,
                Arc::from(topic.as_str()),
                identity_rx,
                subscriber,
                sender.clone(),
                Arc::clone(stop),
            ));
            self.routes.insert(topic, RosIngress { publisher_uris, identity: identity_tx, task });
        }
        Ok(())
    }

    fn report_probe_failure(&mut self, topic: &str, error: &BridgeError) {
        let message = error.to_string();
        let already_reported = self.probe_failures.get(topic) == Some(&message);
        if already_reported {
            return;
        }
        log::error!("automatic bridge does not route {topic} to hypha: {message}");
        self.probe_failures.insert(topic.to_owned(), message);
    }

    fn abort_all(self) {
        for route in self.routes.into_values() {
            route.task.abort();
        }
    }
}

async fn lookup_publisher_uris(
    graph_client: &RosGraphClient,
    state: &RosTopicState,
    node_uris: &mut BTreeMap<String, String>,
) -> Result<BTreeSet<String>, String> {
    let mut uris = BTreeSet::new();
    for node in &state.publishers {
        if let Some(uri) = node_uris.get(node) {
            uris.insert(uri.clone());
            continue;
        }
        let uri = graph_client.lookup_node(node).await?;
        node_uris.insert(node.clone(), uri.clone());
        uris.insert(uri);
    }
    Ok(uris)
}

async fn probe_topic(
    graph_client: &RosGraphClient,
    topic: &str,
    state: &RosTopicState,
    publisher_uris: &BTreeSet<String>,
) -> Result<RosIdentity, BridgeError> {
    let mut identities = Vec::with_capacity(publisher_uris.len());
    for publisher_uri in publisher_uris {
        identities.push(probe_publisher(graph_client, topic, publisher_uri).await?);
    }
    agreed_identity(&state.type_name, &identities)
}

async fn forward_to_ros(
    ros: &NodeHandle,
    graph: &RosGraphSnapshot,
    hypha_topics: &BTreeMap<String, HyphaTopicState>,
    publishers: &mut BTreeMap<String, PublisherAny>,
    publisher_identity: &mut BTreeMap<String, Arc<ResolvedSchema>>,
    not_bridged_said: &mut BTreeSet<(String, String)>,
    message: BridgeMsg,
) -> Result<(), BridgeError> {
    let topic = &*message.topic;
    let Some(hypha_state) = hypha_topics.get(topic) else {
        return Ok(());
    };
    if !hypha_state.published {
        return Ok(());
    }
    let has_ros_subscriber = graph
        .topics
        .get(topic)
        .is_some_and(|state| desired_flows(state, hypha_state).contains(&AutoFlow::HyphaToRos));
    // The bus side lets one stream of a topic through at a time, but the stream may change when
    // the bridged one's publishers leave. A ROS 1 topic keeps its type under its subscribers:
    // the type first advertised stays, and another is told once and not bridged.
    if let Some(existing) = publisher_identity.get(topic)
        && !same_identity(existing, &message.identity)
    {
        if let Some(warning) =
            not_bridged_warning(topic, existing, &message.identity, not_bridged_said)
        {
            log::warn!("{warning}");
        }
        return Ok(());
    }
    if !publisher_identity.contains_key(topic) {
        let publisher = ros
            .advertise_any(
                topic,
                &message.identity.data_type,
                &message.identity.definition,
                8,
                is_latched_topic(topic),
            )
            .await
            .map_err(BridgeError::ros_connection)?;
        publishers.insert(topic.to_owned(), publisher);
        publisher_identity.insert(topic.to_owned(), Arc::clone(&message.identity));
    }
    if !has_ros_subscriber {
        return Ok(());
    }
    let Some(publisher) = publishers.get(topic) else {
        return Ok(());
    };
    publish_to_ros(publisher, &message).await
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use hypha_testkit::unique_name;
    use std::time::Duration;

    const V1: &str = "b3087778e93fcd34cc8d65bc54e850d1";
    const V2: &str = "f79835def461350458ead76f07016dd7";

    /// The md5 of each sample the bus side handed to the ROS side so far.
    fn handed_over(events: &mpsc::Receiver<AutoEvent>) -> Vec<String> {
        events
            .try_iter()
            .filter_map(|event| match event {
                AutoEvent::Sample(message) => Some(message.identity.md5.clone()),
                AutoEvent::Topics(_) => None,
            })
            .collect()
    }

    /// Passes of the bus loop, `send` before each, until `done` holds of what was handed over,
    /// or a few seconds.
    fn pass_until(
        runtime: &mut AutoBusRuntime,
        bus: &BusNode,
        follower: &mut StreamFollower,
        events: &mpsc::Receiver<AutoEvent>,
        mut send: impl FnMut(),
        done: impl Fn(&[String]) -> bool,
    ) -> Vec<String> {
        let stop = AtomicBool::new(false);
        let mut seen = Vec::new();
        let deadline = Instant::now().checked_add(Duration::from_secs(10)).expect("a deadline");
        while Instant::now() < deadline && !done(&seen) {
            send();
            let listing = bus.list_topics();
            runtime.follow(bus, follower, vec![listing]);
            runtime.drain_hypha_to_ros(bus, follower, &stop).expect("drain");
            runtime.publish_topic_snapshot(bus, &stop).expect("snapshot");
            seen.extend(handed_over(events));
            std::thread::sleep(Duration::from_millis(10));
        }
        seen
    }

    #[test]
    fn one_stream_of_a_topic_reaches_ros_and_the_next_takes_over_when_it_ends() {
        let name = unique_name("bridge_streams");
        let topic = format!("/{name}/odom");
        let bus = BusNode::new(&name).expect("bridge node");
        let peer = BusNode::new(&unique_name("bridge_streams_peer")).expect("peer");
        let mut v1 = peer.publish_raw(&topic, "t/R", V1, "int32 value\n").expect("v1");
        let mut v2 = peer
            .publish_raw(&topic, "t/R", V2, "int32 value\nstring label\nfloat64 weight\n")
            .expect("v2");
        let (_ros_tx, ros_rx) = mpsc::channel();
        let (event_tx, events) = mpsc::channel();
        let mut runtime = AutoBusRuntime::new(ros_rx, event_tx);
        let mut follower =
            StreamFollower::new(vec![topic.clone()], StreamSelector::default(), SUBSCRIPTION_DEPTH);
        follower.refresh(&bus).expect("refresh");

        let long = [3_u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        for _ in 0..5 {
            assert!(v1.publish(&[1, 0, 0, 0]));
            assert!(v2.publish(&long));
        }
        let first =
            pass_until(&mut runtime, &bus, &mut follower, &events, || {}, |seen| seen.len() >= 5);
        std::thread::sleep(Duration::from_millis(100));
        let first: Vec<String> = first.into_iter().chain(handed_over(&events)).collect();
        assert_eq!(first.len(), 5, "the five samples of one stream, not ten: {first:?}");
        let bridged = first.first().expect("a sample").clone();
        assert!(first.iter().all(|md5| *md5 == bridged), "{first:?}");

        // The bridged version's publishers leave: the other stream takes its place.
        let (mut staying, left) = if bridged == V1 { (v2, v1) } else { (v1, v2) };
        drop(left);
        let other = if bridged == V1 { V2 } else { V1 };
        let payload: &[u8] = if bridged == V1 { &long } else { &[1, 0, 0, 0] };
        let after = pass_until(
            &mut runtime,
            &bus,
            &mut follower,
            &events,
            || assert!(staying.publish(payload)),
            |seen| !seen.is_empty(),
        );
        assert!(after.iter().all(|md5| md5 == other), "the other version now: {after:?}");
    }
}
