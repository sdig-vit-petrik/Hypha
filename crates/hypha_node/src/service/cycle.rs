//! A server and a client in a cycle: the `Requests` and `Calls` ports
//! (`docs/design/native-services.md` §2, §3, §4).
//!
//! A cycle's requests are an input of its steps, cut as every input is: a step on simulated
//! time at T takes the requests stamped strictly before T, in the order of stamp, client node
//! name, client port name and sequence, at most `per_step` of them. A step takes them only
//! when it asks (`requests()`); those it never asked for stay queued for a later step. A
//! request the step read and left without an answer is refused after it.
//!
//! A call is an output of a step: staged in the step, sent after the step succeeds, stamped
//! with the step's time. Its outcome is an input of a later step, cut like a request. A step
//! that fails sends nothing: its answers end as failed (`ServerLost`), its calls `NotSent`.
//!
//! The cut binds only a peer that takes part in the replay, one that reports its progress:
//! a request of a tool or a plain node, or an answer of a server that does not report, is
//! read at the first step after it came, whatever its stamp. That is not deterministic,
//! and it is logged once per port.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use hypha_iceoryx2::Iox2Transport;

use super::client::{ClientState, Finished};
use super::outcome::{CallResult, CallStatus};
use super::port::{FedServer, Received, ServerPort};
use super::server::{HyphaRequest, ServerCore, begin_step_answers, end_step_answers};
use super::spec::{Role, ServiceDecl};
use crate::node::HyphaNode;
use crate::status::Error;

/// The requests a cycle step takes when its server's table names no `per_step`.
pub const DEFAULT_REQUESTS_PER_STEP: usize = 8;

/// The longest label a client gives its server: iceoryx2's port names.
const MAX_CLIENT_LABEL: usize = 64;

/// The calls in flight of a `Calls` port when its table names no `outstanding`.
pub const DEFAULT_CALLS_OUTSTANDING: usize = 1;

/// The servers and clients of one cycle, owned by the cycle.
#[derive(Default)]
pub struct CycleServices {
    servers: Vec<CycleServer>,
    clients: Vec<CycleClient>,
    /// The clock of the step that runs now; `None` between steps.
    step: Option<StepClock>,
    /// The node ids of the service peers that take part in the replay, sorted, as the
    /// upstream hold found them last: the cut binds their requests and answers only.
    reporters: Vec<u128>,
    /// Whether a service peer of this node outside the cycle's `WaitSet` was logged.
    outside_waitset_logged: std::cell::Cell<bool>,
}

/// The clock of one step.
#[derive(Debug, Clone, Copy)]
struct StepClock {
    /// The step's framework time, which its answers and calls carry.
    stamp: i64,
    /// Its scheduling time, against which the calls' deadlines are judged.
    scheduled: i64,
    /// The stamp cut of a step on simulated time: it reads what is stamped before it.
    cut: Option<i64>,
}

/// One request a cycle step read: answer it through [`HyphaRequest`].
pub struct StepRequest {
    /// The request, which the step answers or defers.
    pub request: Box<HyphaRequest>,
    /// The encoded request.
    pub payload: Vec<u8>,
    /// The client's node name; empty when it cannot be read.
    pub client_node: String,
}

struct Queued {
    received: Received,
    payload: Vec<u8>,
}

struct CycleServer {
    core: Arc<ServerCore>,
    /// Whether a request from outside the replay was logged.
    outside_logged: bool,
    per_step: usize,
    /// Requests received and not yet read by a step, in step order.
    queued: Vec<Queued>,
    /// What the step that runs now read, once it asked.
    taken: Option<Vec<StepRequest>>,
}

struct CycleClient {
    state: Mutex<ClientState>,
    /// The service it calls.
    service: String,
    /// Whether an answer from a server outside the replay was logged.
    outside_logged: bool,
    /// Requests the step that runs now called with.
    staged: Vec<Vec<u8>>,
    /// The outcomes the step that runs now may read, in call order.
    results: VecDeque<CallResult>,
    /// Outcomes of calls a failed step staged, for the next step.
    next: Vec<Finished>,
    /// Outcomes a successful step did not read, dropped after it.
    unread: u64,
    unread_logged: bool,
}

impl Drop for CycleServer {
    fn drop(&mut self) {
        self.core.close();
    }
}

impl CycleServices {
    /// Add the server of `decl` on `node`; returns its index among the cycle's servers.
    pub(crate) fn add_server(
        &mut self,
        node: &HyphaNode,
        decl: &ServiceDecl<'_>,
    ) -> Result<u32, Error> {
        let resolved = super::spec::resolve(node, decl, Role::Server)?;
        let name = resolved.spec.name.clone();
        let port = node
            .transport
            .serve(&resolved.spec)
            .map_err(|cause| super::spec::refuse_spec(&name, &cause))?;
        let capacity = port.capacity().max(1);
        let per_step = resolved.per_step.unwrap_or(DEFAULT_REQUESTS_PER_STEP).min(capacity);
        let index = u32::try_from(self.servers.len()).map_err(|_| Error::ResourceLimit)?;
        self.servers.push(CycleServer {
            core: ServerCore::new(name, ServerPort::Bus(Box::new(port))),
            outside_logged: false,
            per_step,
            queued: Vec::new(),
            taken: None,
        });
        Ok(index)
    }

    /// Add a client of `decl` on `node` to the cycle `cycle`; returns its index among the
    /// cycle's clients. Its server knows it as `<index>/<cycle>`, at most
    /// [`MAX_CLIENT_LABEL`] bytes: the cycle's names are unique in its `WaitSet`, so two
    /// cycles of one node do not tie in the order of requests.
    pub(crate) fn add_client(
        &mut self,
        node: &HyphaNode,
        decl: &ServiceDecl<'_>,
        cycle: &str,
    ) -> Result<u32, Error> {
        let index = u32::try_from(self.clients.len()).map_err(|_| Error::ResourceLimit)?;
        let mut label = format!("{index}/{cycle}");
        while label.len() > MAX_CLIENT_LABEL {
            label.pop();
        }
        let outstanding = Some(DEFAULT_CALLS_OUTSTANDING);
        let state = super::client::open_state(node, decl, outstanding, Some(&label))?;
        let service = state.name().to_owned();
        self.clients.push(CycleClient {
            state: Mutex::new(state),
            service,
            outside_logged: false,
            staged: Vec::new(),
            results: VecDeque::new(),
            next: Vec::new(),
            unread: 0,
            unread_logged: false,
        });
        Ok(index)
    }

    /// Add a server whose clients are a test (`hypha::testing::StepHarness`): its table
    /// read from `config` as [`Self::add_server`] reads it, with no node and no bus.
    /// Returns its index among the cycle's servers.
    pub(crate) fn add_fed_server(
        &mut self,
        config: Option<&crate::config::HyphaConfig>,
        decl: &ServiceDecl<'_>,
    ) -> Result<u32, Error> {
        let resolved = super::spec::resolve_fed(config, decl, Role::Server)?;
        let index = u32::try_from(self.servers.len()).map_err(|_| Error::ResourceLimit)?;
        let port = ServerPort::Fed(FedServer::new(resolved.spec.policy.clone()));
        self.servers.push(CycleServer {
            core: ServerCore::new(resolved.spec.name, port),
            outside_logged: false,
            per_step: resolved.per_step.unwrap_or(DEFAULT_REQUESTS_PER_STEP),
            queued: Vec::new(),
            taken: None,
        });
        Ok(index)
    }

    /// Add a client whose server is a test, as [`Self::add_fed_server`] adds a server.
    pub(crate) fn add_fed_client(
        &mut self,
        config: Option<&crate::config::HyphaConfig>,
        decl: &ServiceDecl<'_>,
    ) -> Result<u32, Error> {
        let index = u32::try_from(self.clients.len()).map_err(|_| Error::ResourceLimit)?;
        let state = super::client::open_fed_state(config, decl, Some(DEFAULT_CALLS_OUTSTANDING))?;
        let service = state.name().to_owned();
        self.clients.push(CycleClient {
            state: Mutex::new(state),
            service,
            outside_logged: false,
            staged: Vec::new(),
            results: VecDeque::new(),
            next: Vec::new(),
            unread: 0,
            unread_logged: false,
        });
        Ok(index)
    }

    /// Run `body` on the fed end of server `server`.
    ///
    /// # Errors
    /// `InvalidArgument` for an unknown server or one of the bus.
    pub(crate) fn with_fed_server<R>(
        &self,
        server: u32,
        body: impl FnOnce(&mut FedServer) -> R,
    ) -> Result<R, Error> {
        let index = usize::try_from(server).map_err(|_| Error::InvalidArgument)?;
        let server = self.servers.get(index).ok_or(Error::InvalidArgument)?;
        let fed = server.core.with_port(|port| match port {
            ServerPort::Fed(fed) => Some(body(fed)),
            ServerPort::Bus(_) => None,
        });
        fed.flatten().ok_or(Error::InvalidArgument)
    }

    /// The fed servers' indexes.
    pub(crate) fn server_count(&self) -> u32 {
        u32::try_from(self.servers.len()).unwrap_or(u32::MAX)
    }

    /// Whether request `slot` of server `server` waits in its queue for a step to read it.
    pub(crate) fn queues(&self, server: u32, slot: super::port::Slot) -> bool {
        let index = usize::try_from(server).unwrap_or(usize::MAX);
        self.servers
            .get(index)
            .is_some_and(|server| server.queued.iter().any(|queued| queued.received.slot == slot))
    }

    /// Run `body` on the fed end of client `client`.
    ///
    /// # Errors
    /// `InvalidArgument` for an unknown client or one of the bus.
    pub(crate) fn with_fed_client<R>(
        &mut self,
        client: u32,
        body: impl FnOnce(&mut super::port::FedClient) -> R,
    ) -> Result<R, Error> {
        let client = self.client(client)?;
        let mut state = crate::locked(&client.state);
        state.fed().map(body).ok_or(Error::InvalidArgument)
    }

    /// The clients' count.
    pub(crate) fn client_count(&self) -> u32 {
        u32::try_from(self.clients.len()).unwrap_or(u32::MAX)
    }

    /// True when the cycle has no server and no client.
    pub(crate) fn is_empty(&self) -> bool {
        self.servers.is_empty() && self.clients.is_empty()
    }

    /// The services the cycle serves.
    pub(crate) fn served(&self) -> impl Iterator<Item = &str> {
        self.servers.iter().map(|server| server.core.name())
    }

    /// The services the cycle calls, as they were bound.
    fn called(&self) -> impl Iterator<Item = &str> {
        self.clients.iter().map(|client| client.service.as_str())
    }

    /// True when this cycle and `other` call each other's services, either way. Names
    /// fixed at bind: no lock, no allocation.
    pub(crate) fn is_peer_of(&self, other: &Self) -> bool {
        let serves = |a: &Self, b: &Self| a.served().any(|name| b.called().any(|c| c == name));
        serves(self, other) || serves(other, self)
    }

    /// Keep the node ids of the service peers that take part in the replay, from the
    /// upstream hold of the cycle's turn.
    pub(crate) fn set_reporters(&mut self, reporters: Vec<u128>) {
        self.reporters = reporters;
    }

    /// Whether a peer that takes part in the replay is the node with id `node`.
    pub(crate) fn has_reporter(&self, node: u128) -> bool {
        self.reporters.binary_search(&node).is_ok()
    }

    /// Log once that a service peer of this node is no cycle of the cycle's `WaitSet`.
    pub(crate) fn warn_outside_waitset(&self, cycle: &str) {
        if !self.outside_waitset_logged.replace(true) {
            log::warn!(target: "hypha", "hypha: cycle '{cycle}': a server or a client of its services is in this node but no cycle of its WaitSet (another WaitSet, or a plain callback): no hold orders their steps, so a replay of them is not deterministic (logged once per cycle)");
        }
    }

    /// The outcomes that successful steps of client `client` did not read and dropped.
    ///
    /// # Errors
    /// `InvalidArgument` for an unknown client.
    pub fn unread_results(&mut self, client: u32) -> Result<u64, Error> {
        Ok(self.client(client)?.unread)
    }

    /// Push the node ids of the cycle's service peers now onto `ids`: the clients of its
    /// servers and the servers of its clients, the upstreams of its request and response
    /// edges in a replay.
    pub(crate) fn peer_ids(&self, ids: &mut Vec<u128>) {
        for server in &self.servers {
            server.core.client_node_ids(ids);
        }
        for client in &self.clients {
            crate::locked(&client.state).server_node_ids(ids);
        }
    }

    /// Prepare a step at framework time `stamp` and scheduling time `scheduled`, cut at
    /// `cut` on simulated time: receive the requests that came, and collect the outcomes
    /// the step may read.
    pub(crate) fn begin(
        &mut self,
        stamp: i64,
        scheduled: i64,
        cut: Option<i64>,
        transport: Option<&Iox2Transport>,
    ) {
        self.step = Some(StepClock { stamp, scheduled, cut });
        // A deferred request answered in this step leaves after it, whatever its server.
        begin_step_answers();
        if self.is_empty() {
            return;
        }
        for server in &mut self.servers {
            server.core.drop_abandoned();
            let mut came = false;
            while let Some((received, payload)) = server.core.receive() {
                server.queued.push(Queued { received, payload });
                came = true;
            }
            if came {
                server.queued.sort_by(|a, b| step_order(&a.received, &b.received));
            }
        }
        for client in &mut self.clients {
            let mut finished = std::mem::take(&mut client.next);
            let mut state = crate::locked(&client.state);
            let mut server = Vec::new();
            state.server_node_ids(&mut server);
            let outside = cut.is_some()
                && server.first().is_some_and(|id| self.reporters.binary_search(id).is_err());
            if outside && !client.outside_logged {
                client.outside_logged = true;
                log::warn!(target: "hypha", "hypha: service '{}': its server takes no part in the replay, so its answers are read at the first step after they come, not by their stamp: not deterministic (logged once per port)", client.service);
            }
            let client_cut = if outside { None } else { cut };
            finished.extend(state.drive(scheduled, transport, client_cut));
            drop(state);
            finished.sort_by_key(|(_, _, order)| *order);
            client.results = finished.into_iter().map(|(_, result, _)| result).collect();
        }
    }

    /// End the step that ran: on success send its answers and calls, stamped with its time;
    /// on failure answer what it read as failed and end its calls `NotSent`. Outcomes it
    /// did not read are dropped and counted.
    pub(crate) fn finish(&mut self, succeeded: bool, cycle: &str) {
        let Some(clock) = self.step.take() else {
            return;
        };
        for server in &mut self.servers {
            for mut taken in server.taken.take().unwrap_or_default() {
                taken.request.end_step(succeeded, clock.stamp);
            }
        }
        end_step_answers(succeeded, clock.stamp);
        for client in &mut self.clients {
            let mut state = crate::locked(&client.state);
            for request in std::mem::take(&mut client.staged) {
                if !succeeded {
                    let reason = "the client step that staged the call failed";
                    client.next.push(state.not_sent(request, reason, clock.stamp));
                    continue;
                }
                let status = state.call(&request, None, clock.scheduled, clock.stamp);
                if status != CallStatus::Sent {
                    // Counted against the limit when staged: only a client that is gone gets here.
                    let reason = format!("the client could not send it: {status:?}");
                    client.next.push(state.not_sent(request, &reason, clock.stamp));
                }
            }
            drop(state);
            if !succeeded {
                // A failed step's outcomes go to the next step, before its own NotSent ones.
                let carried = std::mem::take(&mut client.results)
                    .into_iter()
                    .map(|result| (None, result, 0))
                    .chain(std::mem::take(&mut client.next));
                client.next = carried.collect();
                continue;
            }
            let unread = client.results.len();
            client.results.clear();
            if unread > 0 {
                client.unread = client.unread.wrapping_add(u64::try_from(unread).unwrap_or(0));
                if !client.unread_logged {
                    client.unread_logged = true;
                    log::warn!(target: "hypha", "hypha: cycle '{cycle}': {unread} outcomes of calls to '{}' were not read by their step and are dropped; Calls::unreadResults() counts them (logged once per port)", client.service);
                }
            }
        }
    }

    /// The requests the step that runs now reads from server `server`: on the first ask,
    /// up to its `per_step` of those queued (on simulated time, those stamped before the
    /// step's time); the same ones on a later ask in the same step.
    ///
    /// # Errors
    /// `InvalidArgument` for an unknown server; `InvalidState` outside a step.
    pub fn requests(&mut self, server: u32) -> Result<&mut [StepRequest], Error> {
        let clock = self.step.ok_or(Error::InvalidState)?;
        let index = usize::try_from(server).map_err(|_| Error::InvalidArgument)?;
        let server = self.servers.get_mut(index).ok_or(Error::InvalidArgument)?;
        let reporters = &self.reporters;
        if server.taken.is_none() {
            let mut outside = false;
            let mut taken = Vec::new();
            let mut kept = Vec::with_capacity(server.queued.len());
            for queued in std::mem::take(&mut server.queued) {
                let stamp = queued.received.stamp;
                let reports = reporters.binary_search(&queued.received.client_node_id).is_ok();
                let due = clock.cut.is_none_or(|cut| stamp < cut || !reports);
                if !due || taken.len() >= server.per_step {
                    kept.push(queued);
                    continue;
                }
                outside |= clock.cut.is_some() && !reports;
                let Queued { received, payload } = queued;
                taken.push(StepRequest {
                    request: Box::new(HyphaRequest::of_step(&server.core, received.slot)),
                    payload,
                    client_node: received.client_node,
                });
            }
            server.queued = kept;
            if outside && !server.outside_logged {
                server.outside_logged = true;
                log::warn!(target: "hypha", "hypha: service '{}': a client that takes no part in the replay (a tool or a plain node) is served at the first step after its request came, not by its stamp: not deterministic (logged once per server)", server.core.name());
            }
            server.taken = Some(taken);
        }
        Ok(server.taken.as_mut().map_or(&mut [][..], Vec::as_mut_slice))
    }

    /// The next outcome of client `client` that the step that runs now reads, in call order;
    /// `None` when it read them all.
    ///
    /// # Errors
    /// `InvalidArgument` for an unknown client; `InvalidState` outside a step.
    pub fn next_result(&mut self, client: u32) -> Result<Option<CallResult>, Error> {
        if self.step.is_none() {
            return Err(Error::InvalidState);
        }
        Ok(self.client(client)?.results.pop_front())
    }

    /// Stage a call of client `client` with `request`, sent after the step succeeds.
    /// `Busy` when the client has its limit of calls in flight, those staged in this step
    /// included; `TooLarge` above the service's bound. Either sends nothing.
    ///
    /// # Errors
    /// `InvalidArgument` for an unknown client; `InvalidState` outside a step.
    pub fn call(&mut self, client: u32, request: &[u8]) -> Result<CallStatus, Error> {
        if self.step.is_none() {
            return Err(Error::InvalidState);
        }
        let client = self.client(client)?;
        let state = crate::locked(&client.state);
        if request.len() > state.max_request_bytes() {
            return Ok(CallStatus::TooLarge);
        }
        let free = state.free().unwrap_or(0);
        drop(state);
        if client.staged.len() >= free {
            return Ok(CallStatus::Busy);
        }
        client.staged.push(request.to_vec());
        Ok(CallStatus::Sent)
    }

    fn client(&mut self, client: u32) -> Result<&mut CycleClient, Error> {
        let index = usize::try_from(client).map_err(|_| Error::InvalidArgument)?;
        self.clients.get_mut(index).ok_or(Error::InvalidArgument)
    }
}

/// The order in which a server's steps read requests: stamp, then client node name, then
/// client port name, then sequence. Names, unlike ids, are the same on every run.
///
/// A cycle's client names itself `<index>/<cycle>`, so two cycles of one node do not tie; the
/// client's per-run id breaks what is left, a client whose node cannot be read.
fn step_order(a: &Received, b: &Received) -> std::cmp::Ordering {
    (a.stamp, &a.client_node, &a.client_port, a.sequence, a.client).cmp(&(
        b.stamp,
        &b.client_node,
        &b.client_port,
        b.sequence,
        b.client,
    ))
}
