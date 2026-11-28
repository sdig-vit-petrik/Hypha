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

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use hypha_iceoryx2::{Iox2Transport, ReceivedRequest};

use super::client::{ClientState, Finished};
use super::outcome::{CallResult, CallStatus};
use super::server::{HyphaRequest, ServerCore, begin_step_answers, end_step_answers};
use super::spec::{Role, ServiceDecl};
use crate::node::HyphaNode;
use crate::status::Error;

/// The requests a cycle step takes when its server's table names no `per_step`.
pub const DEFAULT_REQUESTS_PER_STEP: usize = 8;

/// The calls in flight of a `Calls` port when its table names no `outstanding`.
pub const DEFAULT_CALLS_OUTSTANDING: usize = 1;

/// The servers and clients of one cycle, owned by the cycle.
#[derive(Default)]
pub struct CycleServices {
    servers: Vec<CycleServer>,
    clients: Vec<CycleClient>,
    /// The clock of the step that runs now; `None` between steps.
    step: Option<StepClock>,
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
    received: ReceivedRequest,
    payload: Vec<u8>,
}

struct CycleServer {
    core: Arc<ServerCore>,
    per_step: usize,
    /// Requests received and not yet read by a step, in step order.
    queued: Vec<Queued>,
    /// What the step that runs now read, once it asked.
    taken: Option<Vec<StepRequest>>,
}

struct CycleClient {
    state: Mutex<ClientState>,
    /// Requests the step that runs now called with.
    staged: Vec<Vec<u8>>,
    /// The outcomes the step that runs now may read, in call order.
    results: VecDeque<CallResult>,
    /// Outcomes of calls a failed step staged, for the next step.
    next: Vec<Finished>,
    /// Outcomes no step read, dropped at the end of their step.
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
            core: ServerCore::new(name, port),
            per_step,
            queued: Vec::new(),
            taken: None,
        });
        Ok(index)
    }

    /// Add a client of `decl` on `node`; returns its index among the cycle's clients.
    pub(crate) fn add_client(
        &mut self,
        node: &HyphaNode,
        decl: &ServiceDecl<'_>,
    ) -> Result<u32, Error> {
        let state = super::client::open_state(node, decl, Some(DEFAULT_CALLS_OUTSTANDING))?;
        let index = u32::try_from(self.clients.len()).map_err(|_| Error::ResourceLimit)?;
        self.clients.push(CycleClient {
            state: Mutex::new(state),
            staged: Vec::new(),
            results: VecDeque::new(),
            next: Vec::new(),
            unread: 0,
            unread_logged: false,
        });
        Ok(index)
    }

    /// True when the cycle has no server and no client.
    pub(crate) fn is_empty(&self) -> bool {
        self.servers.is_empty() && self.clients.is_empty()
    }

    /// The services the cycle serves.
    pub(crate) fn served(&self) -> impl Iterator<Item = &str> {
        self.servers.iter().map(|server| server.core.name())
    }

    /// The services the cycle calls.
    pub(crate) fn called(&self) -> Vec<String> {
        self.clients.iter().map(|client| crate::locked(&client.state).name().to_owned()).collect()
    }

    /// True when this cycle and `other` call each other's services, either way.
    pub(crate) fn is_peer_of(&self, other: &Self) -> bool {
        let serves = |a: &Self, b: &Self| {
            let called = b.called();
            a.served().any(|name| called.iter().any(|other| other == name))
        };
        serves(self, other) || serves(other, self)
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
        transport: &Iox2Transport,
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
            finished.extend(crate::locked(&client.state).drive(scheduled, Some(transport), cut));
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
            let unread = client.results.len();
            client.results.clear();
            if unread > 0 {
                client.unread = client.unread.wrapping_add(u64::try_from(unread).unwrap_or(0));
                if !client.unread_logged {
                    client.unread_logged = true;
                    let name = crate::locked(&client.state).name().to_owned();
                    log::info!(target: "hypha", "hypha: cycle '{cycle}': {unread} outcomes of calls to '{name}' were not read by their step and are dropped (logged once per port)");
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
        let taken = server.taken.get_or_insert_with(|| {
            let due = server
                .queued
                .iter()
                .take_while(|queued| {
                    let stamp = i64::try_from(queued.received.framework_sent_nanos).unwrap_or(0);
                    clock.cut.is_none_or(|cut| stamp < cut)
                })
                .count()
                .min(server.per_step);
            server
                .queued
                .drain(..due)
                .map(|Queued { received, payload }| StepRequest {
                    request: Box::new(HyphaRequest::of_step(&server.core, received.handle)),
                    payload,
                    client_node: received.client_node,
                })
                .collect()
        });
        Ok(taken.as_mut_slice())
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
fn step_order(a: &ReceivedRequest, b: &ReceivedRequest) -> std::cmp::Ordering {
    (a.framework_sent_nanos, &a.client_node, &a.client_port, a.sequence).cmp(&(
        b.framework_sent_nanos,
        &b.client_node,
        &b.client_port,
        b.sequence,
    ))
}
