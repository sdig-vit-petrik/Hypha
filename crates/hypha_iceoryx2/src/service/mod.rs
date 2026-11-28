//! Services: request and response between nodes over iceoryx2's request-response
//! pattern (`docs/design/native-services.md`).
//!
//! Each stream (a name and the `.srv` MD5, [`key`]) is three iceoryx2 services:
//!
//! - `hypha/srv/<name>@<16 hex>`: request-response, `[u8]` payloads, the hypha frame
//!   header as the user header of both directions, at most one server;
//! - `.../request`: an event the clients notify and the server listens on;
//! - `.../response`: an event the server notifies and the clients listen on.
//!
//! The name-level event `hypha/srv/<name>` has a single notifier slot. The server holds
//! it, so a name has one server across all its versions. A server that finds the slot
//! held by a dead node reclaims that node first.
//!
//! Every send returns at once: both backpressure strategies are `DiscardData`, and a
//! client cannot overflow its server, whose queue for each client is that client's limit
//! of calls in flight.

mod client;
mod key;
mod listing;
mod open;
mod server;

pub use client::{CallEvent, CallId, CallReport, ClientCounters, Iox2Client, Liveness};
pub use listing::DiscoveredServiceStream;
pub use server::{Iox2Server, ReceivedRequest, RequestHandle, ServerCounters};

use std::time::{Duration, Instant};

use hypha_core::port::{PortError, ServerHolder, ServiceError, ServiceSpec, is_cli_node_name};
use iceoryx2::node::{NodeState, NodeView as _};
use iceoryx2::port::port_name::PortName;
use iceoryx2::prelude::*;

use crate::notification::create_notifier;
use crate::transport::{Iox2Transport, node_port_name};
use key::StreamKey;
use open::PeerNode;

/// Backend code: the event of a service lock's name is not that service's lock.
pub(crate) const BACKEND_LOCK_INVALID: u32 = 45;
/// Backend code: a service's server or client port could not be created.
pub(crate) const BACKEND_PORT: u32 = 46;
/// Backend code: a request or a response could not be loaned, sent or received.
pub(crate) const BACKEND_EXCHANGE: u32 = 47;
/// Backend code: a dead node that holds a service's server lock could not be reclaimed.
pub(crate) const BACKEND_RECLAIM: u32 = 48;

/// The most file descriptors one service end takes in a process, measured on macOS (server
/// 18, client 20 including the stream's shared ones) and Linux (13, 14).
const DESCRIPTORS_PER_SERVICE: u64 = 20;

/// How long a server's creation waits for a server port that a dead node held while
/// another process reclaims it.
const SERVER_PORT_WINDOW: Duration = Duration::from_millis(500);

fn sender_id() -> u64 {
    let now = u64::try_from(hypha_time::steady_now().nanos()).unwrap_or(0);
    hypha_wire::integrity::next_publisher_id(now, std::process::id())
}

/// A transport failure explained as running out of file descriptors, when fewer than one
/// service needs are left: iceoryx2 reports that only as an internal failure.
fn explain_descriptors(cause: ServiceError) -> ServiceError {
    if !matches!(cause, ServiceError::Port(PortError::Backend(_))) {
        return cause;
    }
    let Some(limit) = rustix::process::getrlimit(rustix::process::Resource::Nofile).current else {
        return cause;
    };
    // The listing takes a descriptor of its own; when none is left, all of them are open.
    let open = std::fs::read_dir("/dev/fd").map_or(limit, |entries| {
        u64::try_from(entries.count()).unwrap_or(u64::MAX).saturating_sub(1)
    });
    if open.saturating_add(DESCRIPTORS_PER_SERVICE) < limit {
        return cause;
    }
    let exhausted = ServiceError::OutOfDescriptors { open, limit };
    log::error!(target: "hypha/transport", "hypha: {exhausted}");
    exhausted
}

impl Iox2Transport {
    /// Create the server of `spec`'s name. It takes the name's server lock, reclaiming a
    /// dead holder first, and wakes the domain's graph listeners, so clients waiting for a
    /// server send.
    ///
    /// # Errors
    /// [`ServiceError::AlreadyServed`] when a live node serves the name (this version or
    /// another), the validation errors of [`ServiceSpec::validate`],
    /// [`ServiceError::QosMismatch`] for a stream of another policy,
    /// [`ServiceError::OutOfDescriptors`], and [`ServiceError::Port`] for another transport
    /// failure.
    pub fn serve(&self, spec: &ServiceSpec) -> Result<Iox2Server, ServiceError> {
        self.serve_once(spec).map_err(explain_descriptors)
    }

    fn serve_once(&self, spec: &ServiceSpec) -> Result<Iox2Server, ServiceError> {
        let key = StreamKey::of(spec)?;
        let service = open::open_stream(&self.node, &key, spec)?;
        let lock = open::acquire_lock(&self.node, &key, spec)?;
        let port_name = node_port_name(&self.node)?;
        let started = Instant::now();
        let server = loop {
            let created = service
                .server_builder()
                .name(&port_name)
                .initial_max_slice_len(spec.policy.max_response_bytes)
                .allocation_strategy(AllocationStrategy::Static)
                .backpressure_strategy(BackpressureStrategy::DiscardData)
                .max_loaned_responses_per_request(1)
                .create();
            let cause = match created {
                Ok(server) => break server,
                Err(cause) => cause,
            };
            if cause != iceoryx2::service::port_factory::server::ServerCreateError::ExceedsMaxSupportedServers {
                log::error!(target: "hypha/transport",
                    "hypha_iceoryx2: service '{}': cannot create the server: {cause:?}", key.public());
                return Err(ServiceError::Port(PortError::Backend(BACKEND_PORT)));
            }
            // The lock is ours, so the port belongs to a dead holder that another process
            // reclaims now, or to one this process reclaims here.
            let mut holder = None;
            service.dynamic_config().list_servers(|details| {
                holder = Some(details.node_id);
                CallbackProgression::Stop
            });
            if let Some(id) = holder {
                match open::peer_node(&service, id)? {
                    PeerNode::Dead(view) => open::reclaim_dead(&key, "its server port", *view),
                    PeerNode::Alive { name, pid } => {
                        return Err(ServiceError::AlreadyServed(ServerHolder {
                            node: name,
                            pid,
                            served: None,
                        }));
                    }
                    PeerNode::Unknown => {}
                }
            }
            if started.elapsed() > SERVER_PORT_WINDOW {
                return Err(ServiceError::Port(PortError::Backend(BACKEND_RECLAIM)));
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        let response_notifier = create_notifier(
            &self.node,
            &key.response_event(),
            Some(open::response_event_limits(&spec.policy)),
        )?;
        if let Err(cause) = self.notify_graph_change() {
            log::warn!(target: "hypha/transport",
                "hypha_iceoryx2: service '{}': server created but the graph event failed: {cause:?}", key.public());
        }
        let mut endpoint = Iox2Server::new(
            key,
            spec.policy.clone(),
            std::sync::Arc::clone(&self.node),
            server,
            response_notifier,
            service,
            lock,
            sender_id(),
        );
        // A server without its records still serves; the tools say no definition is known.
        match crate::schema::write_service_schema(spec) {
            Ok(Some(release)) => endpoint.hold_schema(release),
            Ok(None) => {}
            Err(cause) => log::error!(target: "hypha/schema",
                "hypha_iceoryx2: service '{}': its definitions are not registered, so tools cannot encode its requests: {cause}",
                spec.name),
        }
        Ok(endpoint)
    }

    /// Create a client of `spec`'s stream with at most `outstanding` calls in flight
    /// (`None`: the policy's), labelled `port` (`None`: the node's name; at most 64 bytes),
    /// which the server sees with each request. A server need not exist.
    ///
    /// A client of a CLI tool node (`_hyphacli_*`) takes one of the
    /// [`SERVICE_OBSERVER_CLIENTS`](hypha_core::port::SERVICE_OBSERVER_CLIENTS) observer
    /// slots, any other client one of the policy's: neither kind takes the other's. A
    /// client slot held by a dead node is reclaimed first, as a dead server's is.
    ///
    /// # Errors
    /// [`ServiceError::InvalidPolicy`] for `outstanding` of zero or above the policy's,
    /// [`ServiceError::InvalidName`] for a port label iceoryx2 cannot carry,
    /// [`ServiceError::Port`] with [`PortError::ResourceLimit`] when the slots of the
    /// client's kind are taken, and the errors of [`Self::serve`] for the spec and stream.
    pub fn service_client(
        &self,
        spec: &ServiceSpec,
        outstanding: Option<usize>,
        port: Option<&str>,
    ) -> Result<Iox2Client, ServiceError> {
        self.client_once(spec, outstanding, port).map_err(explain_descriptors)
    }

    fn client_once(
        &self,
        spec: &ServiceSpec,
        outstanding: Option<usize>,
        port: Option<&str>,
    ) -> Result<Iox2Client, ServiceError> {
        let key = StreamKey::of(spec)?;
        let outstanding = outstanding.unwrap_or(spec.policy.max_outstanding);
        if outstanding == 0 || outstanding > spec.policy.max_outstanding {
            return Err(ServiceError::InvalidPolicy(format!(
                "a client of '{}' asks for {outstanding} calls in flight; the policy {} allows 1 to {}",
                spec.name, spec.policy.name, spec.policy.max_outstanding
            )));
        }
        let node_name = node_port_name(&self.node)?;
        let port_name = match port {
            Some(label) => PortName::new(label).map_err(|_| ServiceError::InvalidName)?,
            None => node_name,
        };
        let service = open::open_stream(&self.node, &key, spec)?;
        reclaim_dead_clients(&key, &service)?;
        let client = service
            .client_builder()
            .name(&port_name)
            .initial_max_slice_len(spec.policy.max_request_bytes)
            .allocation_strategy(AllocationStrategy::Static)
            .backpressure_strategy(BackpressureStrategy::DiscardData)
            .max_active_requests(outstanding)
            .create()
            .map_err(|cause| {
                if cause == iceoryx2::service::port_factory::client::ClientCreateError::ExceedsMaxSupportedClients {
                    ServiceError::Port(PortError::ResourceLimit)
                } else {
                    log::error!(target: "hypha/transport",
                        "hypha_iceoryx2: service '{}': cannot create a client: {cause:?}", key.public());
                    ServiceError::Port(PortError::Backend(BACKEND_PORT))
                }
            })?;
        // Counted with this client in, so two created at once cannot both take the last slot.
        let (declared, observers) = live_clients(&service)?;
        let (taken, limit, kind) = if is_cli_node_name(node_name.as_str()) {
            (observers, hypha_core::port::SERVICE_OBSERVER_CLIENTS, "tool")
        } else {
            (declared, spec.policy.max_clients, "declared")
        };
        if taken > limit {
            log::error!(target: "hypha/transport",
                "hypha: service '{}': {limit} {kind} clients are connected, the limit (policy {})",
                key.public(), spec.policy.name);
            return Err(ServiceError::Port(PortError::ResourceLimit));
        }
        let request_notifier = create_notifier(
            &self.node,
            &key.request_event(),
            Some(open::request_event_limits(&spec.policy)),
        )?;
        Ok(Iox2Client::new(
            key,
            spec.policy.clone(),
            outstanding,
            std::sync::Arc::clone(&self.node),
            client,
            request_notifier,
            service,
            sender_id(),
        ))
    }

    /// Every service stream of the domain, with its server and client counts.
    ///
    /// # Errors
    /// [`PortError::Backend`] when the registry cannot be read.
    pub fn list_services(&self) -> Result<Vec<DiscoveredServiceStream>, PortError> {
        listing::list(&self.node)
    }

    /// The request and response definitions the server of `name`'s stream of `md5`
    /// registered, waiting up to `timeout` for a server that is writing them; `Ok(None)`
    /// when none is registered.
    ///
    /// # Errors
    /// [`PortError::InvalidTopic`] for a malformed MD5; another [`PortError`] when the
    /// domain configuration fails.
    pub fn service_schema(
        &self,
        name: &str,
        md5: &str,
        timeout: Duration,
    ) -> Result<Option<crate::schema::ServiceSchema>, PortError> {
        crate::schema::lookup_service_schema(name, md5, timeout)
    }
}

/// Reclaim every dead node that holds a client of the stream (and not its server, which
/// the next server reclaims), so its slot and the requests its server holds for it are
/// freed. A dead node of a name that never returns would hold them forever.
fn reclaim_dead_clients(
    key: &StreamKey,
    service: &open::StreamFactory,
) -> Result<(), ServiceError> {
    let client_nodes = client_nodes(service);
    let mut server_nodes = Vec::new();
    service.dynamic_config().list_servers(|details| {
        server_nodes.push(details.node_id);
        CallbackProgression::Continue
    });
    let mut dead = Vec::new();
    service
        .nodes(|state| {
            if let NodeState::Dead(view) = state
                && client_nodes.contains(view.id())
                && !server_nodes.contains(view.id())
            {
                dead.push(view);
            }
            CallbackProgression::Continue
        })
        .map_err(|cause| ServiceError::Port(crate::data_service::node_list_failed(cause)))?;
    for view in dead {
        open::reclaim_dead(key, "a client slot", view);
    }
    Ok(())
}

/// The clients of a stream whose nodes are alive: (declared, tools). A tool is a client of
/// a CLI tool node, as the node list names it.
fn live_clients(service: &open::StreamFactory) -> Result<(usize, usize), ServiceError> {
    let client_nodes = client_nodes(service);
    let mut alive = Vec::new();
    service
        .nodes(|state| {
            if let NodeState::Alive(view) = state {
                alive.push((*view.id(), open::node_name(&view)));
            }
            CallbackProgression::Continue
        })
        .map_err(|cause| ServiceError::Port(crate::data_service::node_list_failed(cause)))?;
    let names = client_nodes
        .iter()
        .filter_map(|id| alive.iter().find(|(node, _)| node == id).map(|(_, name)| name));
    let (observers, declared): (Vec<&String>, Vec<&String>) =
        names.partition(|name| is_cli_node_name(name));
    Ok((declared.len(), observers.len()))
}

/// The node of each client of the stream, one entry per client.
fn client_nodes(service: &open::StreamFactory) -> Vec<iceoryx2::identifiers::UniqueNodeId> {
    let mut nodes = Vec::new();
    service.dynamic_config().list_clients(|details| {
        nodes.push(details.node_id);
        CallbackProgression::Continue
    });
    nodes
}
