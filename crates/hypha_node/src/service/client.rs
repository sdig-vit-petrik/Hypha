//! A client: it sends calls and ends each one in exactly one outcome
//! (`docs/design/native-services.md` §3, §5).
//!
//! A client attached to a `WaitSet` delivers each outcome to the call's own callback on the
//! `WaitSet` thread, at a turn after the call, never inside `call()`. A blocking client is
//! attached to nothing: its `call()` waits on the calling thread for the outcome and
//! delivers it before it returns, and is refused on a thread that runs a dispatch.
//!
//! A call made while the stream has no server is held, taking one of the client's slots,
//! and sent when a server appears; it ends `NoServer` at its deadline. A call that timed
//! out stays counted while its server still holds it, so a later call returns `Busy`
//! rather than overflow the server.

use std::sync::Mutex;
use std::time::Duration;

use hypha_core::port::CallSendError;
use hypha_iceoryx2::{CallId, Iox2Transport, Liveness};

use crate::node::HyphaNode;
use crate::status::Error;
use crate::waitset::HyphaCallbackResult;

use super::call_log::{CallLog, Entry};
use super::outcome::{CallOutcome, CallResult, CallStatus};
use super::port::{ClientPort, FedClient, Polled};
use super::spec::{Role, ServiceDecl};

mod handle;
pub use handle::{
    ClientStatus, HyphaClient, client_call, client_status, client_wait_for_server,
    service_client_create, service_client_destroy,
};

/// What runs once with the outcome of one call. A failing result fails the run of the
/// `WaitSet` that delivered it; a blocking call ignores it.
pub trait CallDone: Send {
    /// Take the call's outcome.
    fn done(self: Box<Self>, result: &CallResult) -> HyphaCallbackResult;
}

impl<F> CallDone for F
where
    F: FnOnce(&CallResult) -> HyphaCallbackResult + Send,
{
    fn done(self: Box<Self>, result: &CallResult) -> HyphaCallbackResult {
        (*self)(result)
    }
}

/// An outcome ready for its callback, with the order of its call among the client's
/// calls. A call of a cycle's `Calls` port has no callback: its step reads the outcome.
pub(crate) type Finished = (Option<Box<dyn CallDone>>, CallResult, u64);

/// One call in flight, or held until a server appears.
struct Call {
    /// The transport's call, or `None` while no server took it.
    sent: Option<CallId>,
    /// The request, kept for the call log.
    request: Vec<u8>,
    /// Framework time at the call.
    sent_nanos: i64,
    /// Scheduling time (simulated, else host monotonic) at which it ends.
    deadline_nanos: i64,
    /// The order of the call among the client's calls.
    order: u64,
    done: Option<Box<dyn CallDone>>,
    /// An outcome that arrived stamped at or after the cut of the step that collected it,
    /// with that stamp: a later step reads it (`docs/design/native-services.md` §4).
    arrived: Option<(CallOutcome, i64)>,
}

/// The state a client's owner and its `WaitSet` share.
pub(crate) struct ClientCore {
    state: Mutex<ClientState>,
}

/// The stream a client calls.
struct Identity {
    name: String,
    type_name: String,
    md5: String,
}

pub(crate) struct ClientState {
    identity: Identity,
    /// `None` once the client was destroyed.
    port: Option<ClientPort>,
    timeout_nanos: i64,
    /// The timeout and where it came from: `1 s (policy defaultService)`.
    timeout_text: String,
    outstanding: usize,
    calls: Vec<Call>,
    /// The order the next call takes.
    next_order: u64,
    /// Outcomes found outside a turn, delivered at the next one.
    ready: Vec<Finished>,
    /// The call log on `<service>/calls`; `None` when it could not be opened.
    log: Option<CallLog>,
}

/// The state of a new client of `decl` on `node`: its port, its limit of calls in flight
/// (the table's `outstanding`, else `outstanding`, else the policy's) and its call log.
/// `label` names the client to its server (by default the node's name): a cycle's client
/// gives its cycle and port, so the order of requests from one node has no tie.
pub(crate) fn open_state(
    node: &HyphaNode,
    decl: &ServiceDecl<'_>,
    outstanding: Option<usize>,
    label: Option<&str>,
) -> Result<ClientState, Error> {
    let resolved = super::spec::resolve(node, decl, Role::Client)?;
    let spec = &resolved.spec;
    let limit = resolved.outstanding.or(outstanding);
    let port = node
        .transport
        .service_client(spec, limit, label)
        .map_err(|cause| super::spec::refuse_spec(&spec.name, &cause))?;
    let client_port = if decl.literal { spec.name.as_str() } else { decl.name };
    let log = CallLog::open(node, spec, client_port);
    Ok(ClientState::of(&resolved, ClientPort::Bus(Box::new(port)), limit, log))
}

/// The state of a client of `decl` whose server is a test (`super::port::FedClient`): its
/// table read from `config` as a start reads it, with no node, no bus and no call log.
pub(crate) fn open_fed_state(
    config: Option<&crate::config::HyphaConfig>,
    decl: &ServiceDecl<'_>,
    outstanding: Option<usize>,
) -> Result<ClientState, Error> {
    let resolved = super::spec::resolve_fed(config, decl, Role::Client)?;
    let limit = resolved.outstanding.or(outstanding);
    let port = ClientPort::Fed(FedClient::new(resolved.spec.policy.clone()));
    Ok(ClientState::of(&resolved, port, limit, None))
}

impl ClientState {
    fn of(
        resolved: &super::spec::Resolved,
        port: ClientPort,
        limit: Option<usize>,
        log: Option<CallLog>,
    ) -> Self {
        let spec = &resolved.spec;
        Self {
            identity: Identity {
                name: spec.name.clone(),
                type_name: spec.type_name.clone(),
                md5: spec.md5.clone(),
            },
            outstanding: limit.unwrap_or(spec.policy.max_outstanding),
            port: Some(port),
            timeout_nanos: i64::try_from(resolved.timeout.as_nanos()).unwrap_or(i64::MAX),
            timeout_text: format!("{} ({})", seconds(resolved.timeout), resolved.timeout_source),
            calls: Vec::new(),
            next_order: 0,
            ready: Vec::new(),
            log,
        }
    }

    /// The fed end of a client whose server is a test; `None` for a client of the bus.
    pub(crate) fn fed(&mut self) -> Option<&mut FedClient> {
        match self.port.as_mut() {
            Some(ClientPort::Fed(port)) => Some(port),
            _ => None,
        }
    }
}

impl ClientCore {
    /// The outcomes found at `now`; see [`ClientState::drive`].
    pub(crate) fn drive(&self, now: i64, transport: &Iox2Transport) -> Vec<Finished> {
        crate::locked(&self.state).drive(now, Some(transport), None)
    }

    /// The earliest deadline of a pending call, or `now` for an outcome already found.
    pub(crate) fn next_deadline(&self) -> Option<i64> {
        crate::locked(&self.state).next_deadline()
    }

    /// The service the client calls, for a slow-callback warning.
    pub(crate) fn name(&self) -> String {
        crate::locked(&self.state).identity.name.clone()
    }
}

impl ClientState {
    /// The calls this client may still make now: its limit, less the calls held for a
    /// server, those in flight (expired ones a server still holds included) and those
    /// whose outcome arrived for a later step. `None` once the client is gone.
    pub(crate) fn free(&self) -> Option<usize> {
        let port = self.port.as_ref()?;
        let held = self.calls.iter().filter(|call| call.sent.is_none() || call.arrived.is_some());
        Some(self.outstanding.saturating_sub(held.count().saturating_add(port.in_flight())))
    }

    /// The largest request the service takes, in bytes; 0 once the client is gone.
    pub(crate) fn max_request_bytes(&self) -> usize {
        self.port.as_ref().map_or(0, |port| port.policy().max_request_bytes)
    }

    /// The service the client calls.
    pub(crate) fn name(&self) -> &str {
        &self.identity.name
    }

    /// Push the node id of the stream's server now onto `ids`.
    pub(crate) fn server_node_ids(&self, ids: &mut Vec<u128>) {
        if let Some(port) = self.port.as_ref() {
            port.server_node_ids(ids);
        }
    }

    /// Call with `request` at scheduling time `now`, stamped with the framework time
    /// `stamp`: a step's time for a call a step staged.
    pub(crate) fn call(
        &mut self,
        request: &[u8],
        done: Option<Box<dyn CallDone>>,
        now: i64,
        stamp: i64,
    ) -> CallStatus {
        if request.len() > self.max_request_bytes() {
            return CallStatus::TooLarge;
        }
        if self.free().is_none_or(|free| free == 0) {
            return CallStatus::Busy;
        }
        let Some(port) = self.port.as_mut() else {
            return CallStatus::Busy;
        };
        let order = self.next_order;
        self.next_order = order.wrapping_add(1);
        let mut call = Call {
            sent: None,
            request: request.to_vec(),
            sent_nanos: stamp,
            deadline_nanos: now.saturating_add(self.timeout_nanos),
            order,
            done,
            arrived: None,
        };
        match send(port, &mut call, stamp) {
            Ok(()) => self.calls.push(call),
            Err(CallSendError::Busy { .. }) => return CallStatus::Busy,
            Err(CallSendError::TooLarge { .. }) => return CallStatus::TooLarge,
            Err(cause) => self.ready.push(finish(call, not_sent(&cause), &mut self.log)),
        }
        CallStatus::Sent
    }

    /// Collect what became of the calls at `now`: answers, losses, calls a new server took,
    /// and deadlines that passed.
    ///
    /// With a `cut` (a step on simulated time) an answer stamped at or after it waits in its
    /// call for a later step, and a deadline at or before `now` ends a call whose answer is
    /// only stamped later as `Timeout`: what a step reads then depends on stamps alone.
    pub(crate) fn drive(
        &mut self,
        now: i64,
        transport: Option<&Iox2Transport>,
        cut: Option<i64>,
    ) -> Vec<Finished> {
        let mut finished = std::mem::take(&mut self.ready);
        let Some(port) = self.port.as_mut() else {
            return finished;
        };
        // A call that timed out keeps its slot until its late answer or its loss is polled.
        if self.calls.is_empty() && port.in_flight() == 0 {
            return finished;
        }
        let before_cut = |stamp: i64| cut.is_none_or(|cut| stamp < cut);
        let (due, kept): (Vec<Call>, Vec<Call>) = std::mem::take(&mut self.calls)
            .into_iter()
            .partition(|call| call.arrived.as_ref().is_some_and(|(_, stamp)| before_cut(*stamp)));
        self.calls = kept;
        for mut call in due {
            if let Some((outcome, _)) = call.arrived.take() {
                finished.push(finish(call, outcome, &mut self.log));
            }
        }
        let mut reports = Vec::new();
        port.poll(&mut reports);
        for Polled { id, outcome, stamp } in reports {
            let Some(index) = self.calls.iter().position(|call| call.sent == Some(id)) else {
                continue;
            };
            match (stamp, self.calls.get_mut(index)) {
                (Some(stamp), Some(call)) if !before_cut(stamp) => {
                    call.arrived = Some((outcome, stamp));
                }
                _ => {
                    let call = self.calls.remove(index);
                    finished.push(finish(call, outcome, &mut self.log));
                }
            }
        }
        let waiting = self.calls.iter().any(|call| call.sent.is_none());
        if waiting && port.server_available() {
            let mut kept = Vec::with_capacity(self.calls.len());
            for mut call in std::mem::take(&mut self.calls) {
                // Stamped with the time of its call, a step's for a call a step staged.
                let stamp = call.sent_nanos;
                match send(port, &mut call, stamp) {
                    // Held until the server takes it, or its deadline.
                    Ok(()) | Err(CallSendError::Busy { .. }) => kept.push(call),
                    Err(cause) => finished.push(finish(call, not_sent(&cause), &mut self.log)),
                }
            }
            self.calls = kept;
        }
        let (due, kept): (Vec<Call>, Vec<Call>) = std::mem::take(&mut self.calls)
            .into_iter()
            .partition(|call| call.deadline_nanos <= now);
        self.calls = kept;
        for call in due {
            let outcome = match (call.sent, call.arrived.is_some()) {
                // Its answer is stamped at or after this step, which a later step would read.
                (Some(_), true) => CallOutcome::Timeout(format!("after {}", self.timeout_text)),
                (None, _) => CallOutcome::NoServer(no_server_text(
                    &self.identity,
                    &self.timeout_text,
                    transport,
                )),
                (Some(id), false) => expire(port, id, &self.timeout_text),
            };
            finished.push(finish(call, outcome, &mut self.log));
        }
        finished
    }

    /// End a call that a failed step staged as `NotSent`, recorded in the call log.
    pub(crate) fn not_sent(&mut self, request: Vec<u8>, reason: &str, stamp: i64) -> Finished {
        let order = self.next_order;
        self.next_order = order.wrapping_add(1);
        let call = Call {
            sent: None,
            request,
            sent_nanos: stamp,
            deadline_nanos: 0,
            order,
            done: None,
            arrived: None,
        };
        finish(call, CallOutcome::NotSent(reason.to_owned()), &mut self.log)
    }

    fn next_deadline(&self) -> Option<i64> {
        if !self.ready.is_empty() {
            return Some(i64::MIN);
        }
        self.calls.iter().map(|call| call.deadline_nanos).min()
    }
}

/// Why a held call found no server: its timeout, and any other version served on the name.
fn no_server_text(
    identity: &Identity,
    timeout_text: &str,
    transport: Option<&Iox2Transport>,
) -> String {
    let Identity { name, type_name, md5 } = identity;
    let mut text = format!("no live server of '{name}' within {timeout_text}");
    let others = transport.and_then(|transport| transport.list_services().ok()).unwrap_or_default();
    let other_versions = others
        .iter()
        .filter(|stream| &stream.name == name && &stream.md5 != md5 && stream.servers > 0);
    for other in other_versions {
        let served = format!(
            "served as {} ({}), not as {type_name} ({})",
            other.type_name,
            short(&other.md5),
            short(md5)
        );
        log::warn!(target: "hypha", "hypha: service '{name}' is {served}");
        text.push_str("; it is ");
        text.push_str(&served);
    }
    text
}

fn short(md5: &str) -> &str {
    md5.get(..8).unwrap_or(md5)
}

/// Send a held `call` now, stamped `stamp`; one that finds no server stays held.
fn send(port: &mut ClientPort, call: &mut Call, stamp: i64) -> Result<(), CallSendError> {
    if call.sent.is_some() {
        return Ok(());
    }
    let stamp = u64::try_from(stamp).unwrap_or(0);
    match port.send(&call.request, stamp) {
        Ok(id) => {
            call.sent = Some(id);
            Ok(())
        }
        Err(CallSendError::NoServer) => Ok(()),
        Err(cause) => Err(cause),
    }
}

fn not_sent(cause: &CallSendError) -> CallOutcome {
    CallOutcome::NotSent(format!("the transport refused the request: {cause}"))
}

/// A call whose deadline passed while it was sent: lost when the probe finds its server
/// dead or gone, else timed out. A timed-out call keeps its slot while the server holds it.
fn expire(port: &mut ClientPort, id: CallId, timeout_text: &str) -> CallOutcome {
    let lost = |port: &mut ClientPort, why: &str| {
        port.abandon(id);
        CallOutcome::ServerLost(why.to_owned())
    };
    if !port.expire(id) {
        return lost(port, "the server left");
    }
    match port.probe_server() {
        Liveness::Dead => lost(port, "the server's node is dead"),
        Liveness::NoServer => lost(port, "the server left"),
        Liveness::Alive | Liveness::Unknown => {
            CallOutcome::Timeout(format!("after {timeout_text}"))
        }
    }
}

/// End `call` in `outcome`, recorded in the call log.
fn finish(call: Call, outcome: CallOutcome, log: &mut Option<CallLog>) -> Finished {
    let done_nanos = hypha_time::now().nanos();
    let result = CallResult { outcome, sent_nanos: call.sent_nanos, done_nanos };
    if let Some(log) = log.as_mut() {
        let sequence = call.sent.map_or(0, |id| id.0);
        log.record(&Entry { sequence, request: &call.request, result: &result });
    }
    (call.done, result, call.order)
}

/// `1 s`, `250 ms`.
fn seconds(duration: Duration) -> String {
    if duration.subsec_nanos() == 0 {
        format!("{} s", duration.as_secs())
    } else {
        format!("{} ms", duration.as_millis())
    }
}
