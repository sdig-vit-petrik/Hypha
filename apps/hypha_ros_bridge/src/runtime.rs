//! Bridge startup, worker loops, and shutdown coordination.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use clap::Parser;
use clap::error::ErrorKind;
use hypha_node::sdk::{
    BusNode, PublisherOptions, RawPublisher, RawSubscriber, ResolvedSchema, SubscriberOptions,
    WaitSet,
};
use roslibrust::ros1::{NodeHandle, PublisherAny};
use tokio::sync::watch;
use tokio::task::JoinSet;

use crate::error::BridgeError;
use crate::handoff::HyphaToRosHandoff;
use crate::manifest::{BridgeManifest, BridgeRoute};
use crate::routing::{
    BridgeFlow, BridgeMsg, ROS_RECEIVE_TIMEOUT, forward_from_ros, handoff_closed, publish_to_ros,
};

/// Time a bus loop serves the node between two of its own passes.
const BUS_WAIT: Duration = Duration::from_millis(10);

/// Command-line configuration for the bridge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeConfig {
    /// Optional TOML manifest path. When absent, automatic same-name discovery is used.
    pub manifest_path: Option<PathBuf>,
    /// ROS master XMLRPC URI.
    pub master_uri: String,
    /// ROS and hypha node name.
    pub node_name: String,
}

/// Command line of `hypha_ros_bridge`.
#[derive(Debug, Parser)]
#[command(name = "hypha_ros_bridge")]
struct Cli {
    /// Optional TOML manifest path. When absent, automatic same-name discovery is used.
    #[arg(long = "config", value_name = "path", allow_hyphen_values = true)]
    manifest_path: Option<PathBuf>,
    /// ROS master XMLRPC URI.
    #[arg(
        long = "master-uri",
        value_name = "uri",
        default_value = "http://localhost:11311",
        allow_hyphen_values = true
    )]
    master_uri: String,
    /// ROS and hypha node name.
    #[arg(
        long = "node-name",
        value_name = "name",
        default_value = "hypha_ros_bridge",
        allow_hyphen_values = true
    )]
    node_name: String,
}

/// Parses bridge command-line arguments. The first item is the program name.
///
/// # Errors
///
/// Returns an error for an unknown flag, a missing value, or an empty master URI or node name.
pub fn parse_args<I>(args: I) -> Result<Option<BridgeConfig>, BridgeError>
where
    I: IntoIterator<Item = String>,
{
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) if error.kind() == ErrorKind::DisplayHelp => {
            print!("{error}");
            return Ok(None);
        }
        Err(error) => return Err(BridgeError::Config(error.to_string())),
    };
    if cli.master_uri.is_empty() || cli.node_name.is_empty() {
        return Err(BridgeError::Config("master URI and node name must not be empty".into()));
    }
    Ok(Some(BridgeConfig {
        manifest_path: cli.manifest_path,
        master_uri: cli.master_uri,
        node_name: cli.node_name,
    }))
}

/// Runs the bridge from command-line arguments.
///
/// # Errors
///
/// Returns an error when startup, a worker, or shutdown setup fails.
pub fn run_with_args<I>(args: I) -> Result<(), BridgeError>
where
    I: IntoIterator<Item = String>,
{
    let Some(config) = parse_args(args)? else {
        return Ok(());
    };

    let stop = Arc::new(AtomicBool::new(false));
    install_stop_signal(&stop)?;
    run(&config, stop)
}

/// Runs the bridge until `stop` is set or a worker fails.
///
/// # Errors
///
/// Returns an error when startup, a worker, or shutdown fails.
pub fn run(config: &BridgeConfig, stop: Arc<AtomicBool>) -> Result<(), BridgeError> {
    let bus = BusNode::new(&config.node_name).map_err(BridgeError::Bus)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| BridgeError::Runtime(error.to_string()))?;
    let ros = runtime.block_on(connect_ros(config))?;
    let Some(manifest_path) = config.manifest_path.as_ref() else {
        return runtime.block_on(crate::automatic_runtime::run(
            bus,
            ros,
            config,
            Arc::clone(&stop),
        ));
    };
    let manifest = BridgeManifest::load(manifest_path)?;
    let (ros_to_hypha_tx, ros_to_hypha_rx) = mpsc::channel::<BridgeMsg>();
    let (hypha_to_ros_tx, hypha_to_ros_rx) = mpsc::channel::<BridgeMsg>();
    let bus_runtime = BusRuntime::new(&bus, &manifest, hypha_to_ros_tx, ros_to_hypha_rx)?;
    let ros_thread_stop = Arc::clone(&stop);
    let bus_thread = thread::spawn(move || bus_runtime.run(bus, ros_thread_stop));
    let ros_result = runtime.block_on(run_ros(
        ros,
        &manifest,
        ros_to_hypha_tx,
        hypha_to_ros_rx,
        Arc::clone(&stop),
    ));

    stop.store(true, Ordering::Release);
    // A join error follows only a panic of the bus thread: it goes on here.
    let bus_result = bus_thread.join().unwrap_or_else(|payload| std::panic::resume_unwind(payload));
    ros_result?;
    bus_result
}

/// Creates the `WaitSet` that a bus loop runs between its own passes. Its run
/// serves the node's housekeeping (latched replay, the simulated clock).
///
/// # Errors
/// Returns an error when the library refuses the `WaitSet` or the attachment.
pub(crate) fn schema_serving_waitset(bus: &BusNode) -> Result<WaitSet<'_>, BridgeError> {
    WaitSet::new(bus).map_err(BridgeError::Bus)
}

/// Serves the node for [`BUS_WAIT`]. This also paces the bus loop, which
/// reads its own subscribers between two calls.
///
/// # Errors
/// Returns an error when the run fails.
pub(crate) fn serve_bus(waitset: &mut WaitSet<'_>) -> Result<(), BridgeError> {
    waitset.run_for(BUS_WAIT).err().map_or(Ok(()), |error| Err(BridgeError::Bus(error)))
}

fn install_stop_signal(stop: &Arc<AtomicBool>) -> Result<(), BridgeError> {
    for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        signal_hook::flag::register(signal, Arc::clone(stop))
            .map_err(|error| BridgeError::Signal(error.to_string()))?;
    }
    Ok(())
}

struct BusRuntime {
    routes: Vec<BusRoute>,
    hypha_to_ros_tx: mpsc::Sender<BridgeMsg>,
    ros_to_hypha_rx: mpsc::Receiver<BridgeMsg>,
}

struct BusRoute {
    id: usize,
    descriptor: BridgeRoute,
    /// The ROS topic and identity every hypha sample of this route carries to ROS.
    ros_topic: Arc<str>,
    identity: Arc<ResolvedSchema>,
    publisher: Option<RawPublisher>,
    subscriber: Option<RawSubscriber>,
}

/// The typed subscription of a `HyphaToRos1` route, once its hypha publisher has opened
/// the topic. `None` until then. A route that names its ROS identity reads the stream of
/// its MD5 alone, so a frame of another version is never sent to ROS under the route's
/// type; an opaque route reads every stream of the topic.
fn open_hypha_input(
    bus: &BusNode,
    descriptor: &BridgeRoute,
) -> Result<Option<RawSubscriber>, BridgeError> {
    let md5 = if descriptor.has_ros_identity() { descriptor.effective_ros_md5() } else { "" };
    let stream = hypha_node::sdk::schema_hash_from_md5(md5);
    let listed = bus.list_topics().map_err(BridgeError::Bus)?;
    let opened = listed.iter().any(|entry| {
        entry.name == descriptor.hypha_topic
            && stream.is_none_or(|schema_hash| entry.schema_hash == schema_hash)
    });
    if !opened {
        return Ok(None);
    }
    // Typed, declaring no policy; depth zero is the service's depth.
    bus.subscribe_raw_with_schema(
        &descriptor.hypha_topic,
        descriptor.effective_ros_type(),
        md5,
        SubscriberOptions::default().keeping(0),
    )
    .map(Some)
    .map_err(BridgeError::Bus)
}

/// The identity a manifest route gives its samples on both sides.
fn route_identity(descriptor: &BridgeRoute) -> Arc<ResolvedSchema> {
    Arc::new(ResolvedSchema {
        data_type: descriptor.effective_ros_type().to_owned(),
        md5: descriptor.effective_ros_md5().to_owned(),
        definition: descriptor.effective_ros_definition().to_owned(),
    })
}

impl BusRuntime {
    fn new(
        bus: &BusNode,
        manifest: &BridgeManifest,
        hypha_to_ros_tx: mpsc::Sender<BridgeMsg>,
        ros_to_hypha_rx: mpsc::Receiver<BridgeMsg>,
    ) -> Result<Self, BridgeError> {
        let mut routes = Vec::with_capacity(manifest.routes.len());
        for (id, descriptor) in manifest.routes.iter().cloned().enumerate() {
            let publisher = if descriptor.direction.allows(BridgeFlow::Ros1ToHypha) {
                // A ROS message can be of any size, so the publisher grows to it.
                Some(
                    bus.publish_raw_configured(
                        &descriptor.hypha_topic,
                        descriptor.effective_ros_type(),
                        descriptor.effective_ros_md5(),
                        descriptor.effective_ros_definition(),
                        PublisherOptions::default().growable(),
                    )
                    .map_err(BridgeError::Bus)?,
                )
            } else {
                None
            };
            // A subscriber that declares no policy must not create its topic. The route
            // opens its subscription once the hypha publisher has opened the topic.
            let subscriber = if descriptor.direction.allows(BridgeFlow::HyphaToRos1) {
                open_hypha_input(bus, &descriptor)?
            } else {
                None
            };
            let ros_topic = Arc::from(descriptor.ros_topic.as_str());
            let identity = route_identity(&descriptor);
            routes.push(BusRoute { id, descriptor, ros_topic, identity, publisher, subscriber });
        }
        Ok(Self { routes, hypha_to_ros_tx, ros_to_hypha_rx })
    }

    /// Opens the hypha input of every route whose publisher has opened its topic.
    fn open_deferred_inputs(&mut self, bus: &BusNode) -> Result<(), BridgeError> {
        for route in &mut self.routes {
            if route.subscriber.is_some()
                || !route.descriptor.direction.allows(BridgeFlow::HyphaToRos1)
            {
                continue;
            }
            route.subscriber = open_hypha_input(bus, &route.descriptor)?;
        }
        Ok(())
    }

    fn run(mut self, bus: BusNode, stop: Arc<AtomicBool>) -> Result<(), BridgeError> {
        let mut waitset = schema_serving_waitset(&bus)?;
        while bus.ok() && !stop.load(Ordering::Acquire) {
            self.open_deferred_inputs(&bus)?;
            serve_bus(&mut waitset)?;
            self.drain_ros_to_hypha(&stop);
            self.drain_hypha_to_ros(&stop)?;
        }
        Ok(())
    }

    fn drain_ros_to_hypha(&mut self, stop: &AtomicBool) {
        while let Ok(message) = self.ros_to_hypha_rx.try_recv() {
            if stop.load(Ordering::Acquire) {
                return;
            }
            let Some(route_id) = message.route_id else {
                log::error!("dropping automatic ROS 1 sample in static bus dispatcher");
                continue;
            };
            let Some(route) = self.routes.iter_mut().find(|route| route.id == route_id) else {
                log::error!("dropping ROS 1 sample for unknown route {route_id}");
                continue;
            };
            let Some(publisher) = route.publisher.as_mut() else {
                log::error!("dropping ROS 1 sample for route without an hypha publisher");
                continue;
            };
            if !publisher.publish(&message.bytes) {
                log::error!("hypha rejected a ROS 1 sample on {}", route.descriptor.hypha_topic);
            }
        }
    }

    fn drain_hypha_to_ros(&mut self, stop: &AtomicBool) -> Result<(), BridgeError> {
        for route in &mut self.routes {
            let Some(subscriber) = route.subscriber.as_mut() else {
                continue;
            };
            while let Some(received) =
                subscriber.try_receive().map_err(|error| BridgeError::Bus(error.to_string()))?
            {
                if stop.load(Ordering::Acquire) {
                    return Ok(());
                }
                let (Some(identity), Some(bytes)) = (received.identity(), received.bytes()) else {
                    log::error!("hypha returned a malformed raw frame");
                    continue;
                };
                let message = BridgeMsg {
                    route_id: Some(route.id),
                    topic: Arc::clone(&route.ros_topic),
                    identity: Arc::clone(&route.identity),
                    stamp_nanos: identity.transport_sent_nanos,
                    bytes: bytes.to_vec(),
                };
                if self.hypha_to_ros_tx.send(message).is_err() {
                    return handoff_closed(stop, "hypha to ROS 1 handoff");
                }
            }
        }
        Ok(())
    }
}

async fn run_ros(
    ros: NodeHandle,
    manifest: &BridgeManifest,
    ros_to_hypha_tx: mpsc::Sender<BridgeMsg>,
    hypha_to_ros_rx: mpsc::Receiver<BridgeMsg>,
    stop: Arc<AtomicBool>,
) -> Result<(), BridgeError> {
    let mut publishers = Vec::new();
    let mut ros_tasks = JoinSet::new();

    for (id, descriptor) in manifest.routes.iter().cloned().enumerate() {
        if descriptor.direction.allows(BridgeFlow::Ros1ToHypha) {
            let subscriber = ros
                .subscribe_any(&descriptor.ros_topic, 8)
                .await
                .map_err(BridgeError::ros_connection)?;
            // A manifest route's identity never changes; the watch only lends
            // it to the loop that automatic mode shares.
            let (_, identity) = watch::channel(Some(route_identity(&descriptor)));
            ros_tasks.spawn(forward_from_ros(
                Some(id),
                Arc::from(descriptor.hypha_topic.as_str()),
                identity,
                subscriber,
                ros_to_hypha_tx.clone(),
                Arc::clone(&stop),
            ));
        }
        if descriptor.direction.allows(BridgeFlow::HyphaToRos1) {
            let publisher = ros
                .advertise_any(
                    &descriptor.ros_topic,
                    descriptor.effective_ros_type(),
                    descriptor.effective_ros_definition(),
                    8,
                    descriptor.latched,
                )
                .await
                .map_err(BridgeError::ros_connection)?;
            publishers.push((id, publisher));
        }
    }
    drop(ros_to_hypha_tx);

    let mut handoff = HyphaToRosHandoff::new(hypha_to_ros_rx);
    let mut dispatch =
        Box::pin(dispatch_hypha_to_ros(&publishers, &mut handoff, Arc::clone(&stop)));
    let receive_result = loop {
        if ros_tasks.is_empty() {
            break (&mut dispatch).await;
        }
        tokio::select! {
            result = &mut dispatch => break result,
            task_result = ros_tasks.join_next() => {
                match task_result {
                    Some(Ok(Ok(()))) => {}
                    Some(Ok(Err(error))) => break Err(error),
                    // Nothing aborts a task before `stop_ros_tasks`, so a join
                    // error is a panic of the task: it goes on here.
                    Some(Err(error)) => std::panic::resume_unwind(error.into_panic()),
                    None => break (&mut dispatch).await,
                }
            }
        }
    };
    stop.store(true, Ordering::Release);
    let task_result = stop_ros_tasks(&mut ros_tasks).await;
    receive_result.and(task_result)
}

async fn connect_ros(config: &BridgeConfig) -> Result<NodeHandle, BridgeError> {
    NodeHandle::new(&config.master_uri, &config.node_name)
        .await
        .map_err(BridgeError::ros_connection)
}

async fn stop_ros_tasks(tasks: &mut JoinSet<Result<(), BridgeError>>) -> Result<(), BridgeError> {
    tasks.abort_all();
    let mut first_error = None;
    while let Some(task_result) = tasks.join_next().await {
        match task_result {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                first_error.get_or_insert(error);
            }
            Err(error) if error.is_cancelled() => {}
            // Not a cancellation, so the task panicked: the panic goes on here.
            Err(error) => std::panic::resume_unwind(error.into_panic()),
        }
    }
    first_error.map_or(Ok(()), Err)
}

async fn dispatch_hypha_to_ros(
    publishers: &[(usize, PublisherAny)],
    handoff: &mut HyphaToRosHandoff,
    stop: Arc<AtomicBool>,
) -> Result<(), BridgeError> {
    while !stop.load(Ordering::Acquire) {
        let receive = handoff.receive(ROS_RECEIVE_TIMEOUT);
        let result = tokio::select! {
            result = receive => result,
            () = tokio::time::sleep(ROS_RECEIVE_TIMEOUT) => continue,
        };
        let result = match result {
            Ok(result) => result,
            Err(_) if stop.load(Ordering::Acquire) => return Ok(()),
            Err(error) => return Err(error),
        };
        let Some(message) = result else {
            continue;
        };
        let Some(route_id) = message.route_id else {
            log::error!("dropping automatic sample in static dispatcher");
            continue;
        };
        let Some((_, publisher)) = publishers.iter().find(|(id, _)| *id == route_id) else {
            log::error!("dropping hypha sample for unknown route {route_id}");
            continue;
        };
        publish_to_ros(publisher, &message).await?;
    }
    Ok(())
}
