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

use hypha_core::port::{PortError, ServerHolder, ServiceError, ServiceSpec, is_cli_node_name};
use iceoryx2::prelude::*;

use crate::notification::create_notifier;
use crate::transport::{Iox2Transport, node_port_name};
use key::StreamKey;

fn sender_id() -> u64 {
    let now = u64::try_from(hypha_time::steady_now().nanos()).unwrap_or(0);
    hypha_wire::integrity::next_publisher_id(now, std::process::id())
}

impl Iox2Transport {
    /// Create the server of `spec`'s name. It takes the name's server lock, reclaiming a
    /// dead holder first, and wakes the domain's graph listeners, so clients waiting for a
    /// server send.
    ///
    /// # Errors
    /// [`ServiceError::AlreadyServed`] when a live node serves the name (this version or
    /// another), the validation errors of [`ServiceSpec::validate`],
    /// [`ServiceError::QosMismatch`] for a stream of another policy, and
    /// [`ServiceError::Port`] for a transport failure.
    pub fn serve(&self, spec: &ServiceSpec) -> Result<Iox2Server, ServiceError> {
        let key = StreamKey::of(spec)?;
        // The stream first: a server that loses the lock to this one then finds this node
        // at the stream and names the type it serves.
        let service = open::open_stream(&self.node, &key, spec)?;
        let lock = open::acquire_lock(&self.node, &key)?;
        let port_name = node_port_name(&self.node)?;
        let server = service
            .server_builder()
            .name(&port_name)
            .initial_max_slice_len(spec.policy.max_response_bytes)
            .allocation_strategy(AllocationStrategy::Static)
            .backpressure_strategy(BackpressureStrategy::DiscardData)
            .max_loaned_responses_per_request(1)
            .create()
            .map_err(|cause| {
                if cause == iceoryx2::service::port_factory::server::ServerCreateError::ExceedsMaxSupportedServers {
                    // The lock is ours, so the stream's server is a port the lock did not
                    // cover: a holder of the old build, or one being reclaimed now.
                    ServiceError::AlreadyServed(ServerHolder { node: String::new(), pid: None, served: None })
                } else {
                    log::error!(target: "hypha/transport",
                        "hypha_iceoryx2: service '{}': cannot create the server: {cause:?}", key.public());
                    ServiceError::Port(PortError::Backend(3))
                }
            })?;
        let response_notifier = create_notifier(
            &self.node,
            &key.response_event(),
            Some(open::response_event_limits(&spec.policy)),
        )?;
        if let Err(cause) = self.notify_graph_change() {
            log::warn!(target: "hypha/transport",
                "hypha_iceoryx2: service '{}': server created but the graph event failed: {cause:?}", key.public());
        }
        Ok(Iox2Server::new(
            key,
            spec.policy.clone(),
            std::sync::Arc::clone(&self.node),
            server,
            response_notifier,
            service,
            lock,
            sender_id(),
        ))
    }

    /// Create a client of `spec`'s stream with at most `outstanding` calls in flight
    /// (`None`: the policy's). A server need not exist.
    ///
    /// A client of a CLI tool node (`_hyphacli_*`) takes one of the observer slots; any
    /// other client is refused once the policy's own clients are connected, so a tool
    /// never takes a declared client's slot.
    ///
    /// # Errors
    /// [`ServiceError::InvalidPolicy`] for `outstanding` of zero or above the policy's,
    /// [`ServiceError::Port`] with [`PortError::ResourceLimit`] when the stream holds its
    /// clients, and the errors of [`Self::serve`] for the spec and the stream.
    pub fn service_client(
        &self,
        spec: &ServiceSpec,
        outstanding: Option<usize>,
    ) -> Result<Iox2Client, ServiceError> {
        let key = StreamKey::of(spec)?;
        let outstanding = outstanding.unwrap_or(spec.policy.max_outstanding);
        if outstanding == 0 || outstanding > spec.policy.max_outstanding {
            return Err(ServiceError::InvalidPolicy(format!(
                "a client of '{}' asks for {outstanding} calls in flight; the policy {} allows 1 to {}",
                spec.name, spec.policy.name, spec.policy.max_outstanding
            )));
        }
        let service = open::open_stream(&self.node, &key, spec)?;
        let port_name = node_port_name(&self.node)?;
        let is_observer = is_cli_node_name(port_name.as_str());
        if !is_observer {
            let mut declared: usize = 0;
            service.dynamic_config().list_clients(|details| {
                if !is_cli_node_name(details.client_name.as_str()) {
                    declared = declared.saturating_add(1);
                }
                CallbackProgression::Continue
            });
            if declared >= spec.policy.max_clients {
                log::error!(target: "hypha/transport",
                    "hypha: service '{}': {declared} clients are connected, the limit of policy {}",
                    key.public(), spec.policy.name);
                return Err(ServiceError::Port(PortError::ResourceLimit));
            }
        }
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
                    ServiceError::Port(PortError::Backend(4))
                }
            })?;
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
}
