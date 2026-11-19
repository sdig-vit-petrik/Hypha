//! Runtime for automatic same-name ROS 1 and hypha routing.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use hypha_node::sdk::{BusNode, PublisherOptions, RawPublisher, ResolvedSchema, SubscriberOptions};
use roslibrust::ros1::{NodeHandle, PublisherAny};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::automatic::RosTopicState;
use crate::automatic::{AutoFlow, HyphaTopicState, RecentSample, RecentSamples, desired_flows};
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
/// Pending samples one followed topic may buffer: zero, the depth of the topic's `QoS`
/// policy depth (8, the `default` policy), which a larger depth would exceed.
const SUBSCRIPTION_DEPTH: usize = 0;
/// Samples taken from one topic per pass, so one busy topic cannot hold the
/// loop open while others wait.
const DRAIN_BUDGET_PER_TOPIC: usize = 64;
const RECENT_SAMPLE_LIMIT: usize = 256;
const EVENT_QUEUE_DEPTH: usize = 64;

enum AutoEvent {
    Topics(BTreeMap<String, HyphaTopicState>),
    Sample(BridgeMsg),
}

struct AutoBusRuntime {
    /// One subscription per followed topic, keyed by topic name, with the
    /// topic its samples carry.
    subscribers: BTreeMap<String, (Arc<str>, hypha_node::sdk::RawSubscriber)>,
    /// Topics whose subscription failure was already reported.
    subscription_failures: BTreeSet<String>,
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
            subscribers: BTreeMap::new(),
            subscription_failures: BTreeSet::new(),
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
        let mut waitset = schema_serving_waitset(&bus)?;
        while bus.ok() && !stop.load(Ordering::Acquire) {
            serve_bus(&mut waitset)?;
            self.drain_ros_to_hypha(&bus, &stop);
            self.drain_hypha_to_ros(&bus, &stop)?;
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
        let topics: BTreeMap<String, HyphaTopicState> = bus
            .list_topics()
            .map_err(BridgeError::Bus)?
            .into_iter()
            .map(|topic| {
                (
                    topic.name,
                    HyphaTopicState { published: topic.published, subscribed: topic.subscribed },
                )
            })
            .collect();
        self.follow_new_topics(bus, &topics);
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

    /// Subscribes to every topic this bridge does not follow yet.
    ///
    /// A pattern subscription resolves its topics once, when it is created.
    fn follow_new_topics(&mut self, bus: &BusNode, topics: &BTreeMap<String, HyphaTopicState>) {
        for name in topics.keys() {
            let already_followed = self.subscribers.contains_key(name);
            if already_followed || !hypha_node::sdk::topic_matches_pattern(BRIDGED_TOPICS, name) {
                continue;
            }
            match bus.subscribe_raw_with_options(
                name,
                SubscriberOptions::default().keeping(SUBSCRIPTION_DEPTH),
            ) {
                Ok(subscriber) => {
                    self.subscription_failures.remove(name);
                    log::debug!("automatic bridge follows {name}");
                    self.subscribers.insert(name.clone(), (Arc::from(name.as_str()), subscriber));
                }
                // A topic that refuses a subscription now is tried again on
                // the next snapshot.
                Err(error) => {
                    if self.subscription_failures.insert(name.clone()) {
                        log::error!(
                            "automatic bridge cannot follow {name}; it will retry: {error}"
                        );
                    }
                }
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

    fn drain_hypha_to_ros(&mut self, bus: &BusNode, stop: &AtomicBool) -> Result<(), BridgeError> {
        for (topic, subscriber) in self.subscribers.values_mut() {
            let mut drained = 0_usize;
            while drained < DRAIN_BUDGET_PER_TOPIC {
                let received = subscriber
                    .try_receive()
                    .map_err(|error| BridgeError::Bus(error.to_string()))?;
                let Some(sample) = received else { break };
                drained = drained.saturating_add(1);
                Self::forward_sample(
                    bus,
                    &mut self.recent,
                    &self.schema_resolver,
                    &self.event_tx,
                    topic,
                    &sample,
                    stop,
                )?;
                if stop.load(Ordering::Acquire) {
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    fn forward_sample(
        bus: &BusNode,
        recent: &mut RecentSamples,
        schema_resolver: &SchemaIdentityCache,
        event_tx: &mpsc::Sender<AutoEvent>,
        topic: &Arc<str>,
        sample: &hypha_node::sdk::ReceivedSample,
        stop: &AtomicBool,
    ) -> Result<(), BridgeError> {
        if stop.load(Ordering::Acquire) {
            return Ok(());
        }
        let Some(identity) = sample.identity() else {
            log::debug!("ignoring automatic hypha sample without identity");
            return Ok(());
        };
        let Some(bytes) = sample.bytes() else {
            log::warn!("dropping malformed automatic hypha sample on {topic}");
            return Ok(());
        };
        if recent.take(topic, identity.transport_sent_nanos, bytes) {
            return Ok(());
        }
        // A retained record always holds a type, a valid MD5 and a definition.
        let schema = match schema_resolver.resolve(bus, topic, identity.schema_hash, Instant::now())
        {
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
        let message = BridgeMsg {
            route_id: None,
            topic: Arc::clone(topic),
            identity: schema,
            stamp_nanos: identity.transport_sent_nanos,
            bytes: bytes.to_vec(),
        };
        if event_tx.send(AutoEvent::Sample(message)).is_err() {
            return handoff_closed(stop, "automatic hypha handoff");
        }
        Ok(())
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
                        retain_publishers(&mut publishers, &mut publisher_identity, &hypha_topics);
                    }
                    Some(AutoEvent::Sample(message)) => {
                        forward_to_ros(
                            &ros,
                            &graph,
                            &hypha_topics,
                            &mut publishers,
                            &mut publisher_identity,
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
                retain_publishers(&mut publishers, &mut publisher_identity, &hypha_topics);
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
    hypha_topics: &BTreeMap<String, HyphaTopicState>,
) {
    publishers.retain(|topic, _| {
        let Some(hypha_state) = hypha_topics.get(topic) else {
            return false;
        };
        hypha_state.published
    });
    publisher_identity.retain(|topic, _| publishers.contains_key(topic));
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
    if let Some(existing) = publisher_identity.get(topic) {
        if !same_identity(existing, &message.identity) {
            log::error!("dropping automatic ROS sample with a conflicting identity on {topic}");
            return Ok(());
        }
    } else {
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
