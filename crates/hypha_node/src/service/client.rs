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

use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use hypha_core::port::CallSendError;
use hypha_iceoryx2::{CallEvent, CallId, Iox2Client, Iox2Listener, Iox2Transport, Liveness};

use crate::node::HyphaNode;
use crate::status::Error;
use crate::waitset::HyphaCallbackResult;

use super::call_log::{CallLog, Entry};
use super::outcome::{CallOutcome, CallResult, CallStatus};
use super::spec::{Role, ServiceDecl};

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

/// An outcome ready for its callback.
pub(crate) type Finished = (Box<dyn CallDone>, CallResult);

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
    done: Box<dyn CallDone>,
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

struct ClientState {
    identity: Identity,
    /// `None` once the client was destroyed.
    port: Option<Iox2Client>,
    timeout_nanos: i64,
    /// The timeout and where it came from: `1 s (policy defaultService)`.
    timeout_text: String,
    outstanding: usize,
    calls: Vec<Call>,
    /// Outcomes found outside a turn, delivered at the next one.
    ready: Vec<Finished>,
    /// The call log on `<service>/calls`; `None` when it could not be opened.
    log: Option<CallLog>,
}

/// A client, owned by its caller. Dropping it drops the outcomes of its calls in flight
/// without calling them: their callbacks belong to an owner that is gone.
pub struct HyphaClient {
    core: Arc<ClientCore>,
    node: Weak<HyphaNode>,
    /// The registration on the node's `WaitSet`; `None` for a blocking client, which owns
    /// its listener instead.
    attachment: Option<crate::waitset::HyphaAttachmentId>,
    blocking: Option<Blocking>,
}

/// A blocking client's own wait: its response listener, and the lock that makes its calls
/// one at a time.
struct Blocking {
    listener: Iox2Listener,
    turn: Mutex<()>,
}

/// What `client_status` reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientStatus {
    /// A live server of the client's stream exists now.
    pub ready: bool,
    /// Calls whose outcome is still to come.
    pub pending: usize,
    /// Records the call log dropped: no free sample, or a record above its bound.
    pub log_dropped: u64,
}

/// Create a client of `decl` on `node`. A `blocking` client is attached to nothing; any
/// other is attached to the node's `WaitSet`, whose run delivers its outcomes. A server
/// need not exist.
///
/// # Errors
/// The errors of the declaration (`InvalidArgument`, `InvalidTopic`); `ResourceLimit` when
/// the clients of the stream are at their limit; `QosMismatch` for a stream of another
/// policy; `InvalidState` during a run; the transport's error otherwise. Each is logged
/// with its reason and recorded for `last_error`.
pub fn service_client_create(
    node: &Arc<HyphaNode>,
    decl: &ServiceDecl<'_>,
    blocking: bool,
) -> Result<HyphaClient, Error> {
    crate::last_error::clear();
    let resolved = super::spec::resolve(node, decl, Role::Client)?;
    let spec = &resolved.spec;
    let port = node
        .transport
        .service_client(spec, resolved.outstanding, None)
        .map_err(|cause| super::spec::refuse_spec(&spec.name, &cause))?;
    let listener = port.open_listener().map_err(Error::from)?;
    let client_port = if decl.literal { spec.name.as_str() } else { decl.name };
    let log = CallLog::open(node, spec, client_port);
    let state = ClientState {
        identity: Identity {
            name: spec.name.clone(),
            type_name: spec.type_name.clone(),
            md5: spec.md5.clone(),
        },
        outstanding: resolved.outstanding.unwrap_or(spec.policy.max_outstanding),
        port: Some(port),
        timeout_nanos: i64::try_from(resolved.timeout.as_nanos()).unwrap_or(i64::MAX),
        timeout_text: format!("{} ({})", seconds(resolved.timeout), resolved.timeout_source),
        calls: Vec::new(),
        ready: Vec::new(),
        log,
    };
    let core = Arc::new(ClientCore { state: Mutex::new(state) });
    let mut client =
        HyphaClient { core, node: Arc::downgrade(node), attachment: None, blocking: None };
    if blocking {
        client.blocking = Some(Blocking { listener, turn: Mutex::new(()) });
    } else {
        let target = crate::waitset::ServiceTarget::Client(Arc::clone(&client.core));
        client.attachment = Some(crate::waitset::attach_service(node, listener, target)?);
    }
    Ok(client)
}

/// Destroy `client`: it is detached from its `WaitSet`, and the outcomes of its calls in
/// flight are dropped uncalled.
pub fn service_client_destroy(client: HyphaClient) {
    drop(client);
}

impl Drop for HyphaClient {
    fn drop(&mut self) {
        let (calls, ready) = {
            let mut state = crate::locked(&self.core.state);
            state.port = None;
            (std::mem::take(&mut state.calls), std::mem::take(&mut state.ready))
        };
        let dropped = calls.len().saturating_add(ready.len());
        if dropped > 0 {
            log::info!(target: "hypha", "hypha: a client was destroyed with {dropped} calls in flight; their outcomes are dropped");
        }
        if let (Some(attachment), Some(node)) = (self.attachment, self.node.upgrade()) {
            crate::waitset::detach_service(&node, attachment);
        }
    }
}

/// Call with `request`. `done` receives the call's one outcome: on the `WaitSet` thread at
/// a later turn, or, for a blocking client, on this thread before the call returns. A call
/// refused as `Busy` or `TooLarge` sends nothing and drops `done` uncalled.
///
/// # Errors
/// `InvalidState` for a blocking call on a thread that runs a `WaitSet` dispatch: that
/// thread would have to deliver the answer it waits for (FSR-43). `done` drops uncalled.
pub fn client_call(
    client: &HyphaClient,
    request: &[u8],
    done: Box<dyn CallDone>,
) -> Result<CallStatus, Error> {
    let Some(blocking) = client.blocking.as_ref() else {
        let now = hypha_time::scheduling_now().nanos();
        return Ok(crate::locked(&client.core.state).call(request, done, now));
    };
    refuse_in_dispatch("callAndWait")?;
    let _one_at_a_time = crate::locked(&blocking.turn);
    let now = hypha_time::scheduling_now().nanos();
    let status = crate::locked(&client.core.state).call(request, done, now);
    if status != CallStatus::Sent {
        return Ok(status);
    }
    let node = client.node.upgrade();
    let transport = node.as_deref().map(|node| &node.transport);
    loop {
        let (finished, next) = {
            let mut state = crate::locked(&client.core.state);
            let finished = state.drive(hypha_time::scheduling_now().nanos(), transport);
            (finished, state.next_deadline())
        };
        if !finished.is_empty() {
            for (done, result) in finished {
                let _ = done.done(&result);
            }
            return Ok(CallStatus::Sent);
        }
        let wait = next.map_or(Duration::from_millis(10), |deadline| {
            let left = deadline.saturating_sub(hypha_time::scheduling_now().nanos()).max(0);
            Duration::from_nanos(left.unsigned_abs()).min(Duration::from_millis(10))
        });
        // A response wakes the wait; a server that appears is seen at the next slice.
        let _ = blocking.listener.timed_wait(|_| {}, wait.max(Duration::from_micros(100)));
    }
}

/// Wait up to `timeout` for a live server of a blocking client's stream.
///
/// # Errors
/// `InvalidState` for a client attached to a `WaitSet`, or on a thread that runs a dispatch.
pub fn client_wait_for_server(client: &HyphaClient, timeout: Duration) -> Result<bool, Error> {
    if client.blocking.is_none() {
        return crate::last_error::refuse(
            Err(Error::InvalidState),
            "waitForServer is for a blocking client (hypha::blocking); a client in a node calls and reads the outcome",
        );
    }
    refuse_in_dispatch("waitForServer")?;
    let started = std::time::Instant::now();
    loop {
        if client_status(client).ready {
            return Ok(true);
        }
        if started.elapsed() >= timeout {
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(1).min(timeout.saturating_sub(started.elapsed())));
    }
}

/// Whether a live server exists now, and how many calls are pending.
#[must_use]
pub fn client_status(client: &HyphaClient) -> ClientStatus {
    let state = crate::locked(&client.core.state);
    ClientStatus {
        ready: state.port.as_ref().is_some_and(Iox2Client::server_available),
        pending: state.calls.len().saturating_add(state.ready.len()),
        log_dropped: state.log.as_ref().map_or(0, CallLog::dropped),
    }
}

fn refuse_in_dispatch(what: &str) -> Result<(), Error> {
    if !crate::waitset::in_dispatch() {
        return Ok(());
    }
    Err(crate::last_error::refuse(
        Error::InvalidState,
        &format!(
            "{what} on the run thread would wait for the thread that must deliver its answer; \
             use call(request, onResult) of a client of the node in a callback"
        ),
    ))
}

/// `1 s`, `250 ms`.
fn seconds(duration: Duration) -> String {
    if duration.subsec_nanos() == 0 {
        format!("{} s", duration.as_secs())
    } else {
        format!("{} ms", duration.as_millis())
    }
}

impl ClientCore {
    /// The resolved service name, copied: for a slow-callback line.
    pub(crate) fn name(&self) -> String {
        crate::locked(&self.state).identity.name.clone()
    }

    /// The outcomes found at `now`; see [`ClientState::drive`].
    pub(crate) fn drive(&self, now: i64, transport: &Iox2Transport) -> Vec<Finished> {
        crate::locked(&self.state).drive(now, Some(transport))
    }

    /// The earliest deadline of a pending call, or `now` for an outcome already found.
    pub(crate) fn next_deadline(&self) -> Option<i64> {
        crate::locked(&self.state).next_deadline()
    }
}

impl ClientState {
    fn call(&mut self, request: &[u8], done: Box<dyn CallDone>, now: i64) -> CallStatus {
        let Some(port) = self.port.as_mut() else {
            return CallStatus::Busy;
        };
        if request.len() > port.policy().max_request_bytes {
            return CallStatus::TooLarge;
        }
        let held = self.calls.iter().filter(|call| call.sent.is_none()).count();
        if held.saturating_add(port.in_flight()) >= self.outstanding {
            return CallStatus::Busy;
        }
        let mut call = Call {
            sent: None,
            request: request.to_vec(),
            sent_nanos: hypha_time::now().nanos(),
            deadline_nanos: now.saturating_add(self.timeout_nanos),
            done,
        };
        match send(port, &mut call) {
            Ok(()) => self.calls.push(call),
            Err(CallSendError::Busy { .. }) => return CallStatus::Busy,
            Err(CallSendError::TooLarge { .. }) => return CallStatus::TooLarge,
            Err(cause) => self.ready.push(finish(call, not_sent(&cause), &mut self.log)),
        }
        CallStatus::Sent
    }

    /// Collect what became of the calls at `now`: answers, losses, calls a new server took,
    /// and deadlines that passed.
    fn drive(&mut self, now: i64, transport: Option<&Iox2Transport>) -> Vec<Finished> {
        let mut finished = std::mem::take(&mut self.ready);
        let Some(port) = self.port.as_mut() else {
            return finished;
        };
        // A call that timed out keeps its slot until its late answer or its loss is polled.
        if self.calls.is_empty() && port.in_flight() == 0 {
            return finished;
        }
        let mut reports = Vec::new();
        port.poll(&mut reports);
        for report in reports.into_iter().filter(|report| !report.late) {
            let Some(index) = self.calls.iter().position(|call| call.sent == Some(report.id))
            else {
                continue;
            };
            let call = self.calls.remove(index);
            finished.push(finish(call, outcome_of(report.event), &mut self.log));
        }
        let waiting = self.calls.iter().any(|call| call.sent.is_none());
        if waiting && port.server_available() {
            let mut kept = Vec::with_capacity(self.calls.len());
            for mut call in std::mem::take(&mut self.calls) {
                match send(port, &mut call) {
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
            let outcome = match call.sent {
                None => CallOutcome::NoServer(no_server_text(
                    &self.identity,
                    &self.timeout_text,
                    transport,
                )),
                Some(id) => expire(port, id, &self.timeout_text),
            };
            finished.push(finish(call, outcome, &mut self.log));
        }
        finished
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

/// Send a held `call` now; one that finds no server stays held.
fn send(port: &mut Iox2Client, call: &mut Call) -> Result<(), CallSendError> {
    if call.sent.is_some() {
        return Ok(());
    }
    let stamp = u64::try_from(hypha_time::now().nanos()).unwrap_or(0);
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
fn expire(port: &mut Iox2Client, id: CallId, timeout_text: &str) -> CallOutcome {
    let lost = |port: &mut Iox2Client, why: &str| {
        let _ = port.abandon(id);
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

fn outcome_of(event: CallEvent) -> CallOutcome {
    match event {
        CallEvent::Answered(response) => CallOutcome::Answered(response),
        CallEvent::Refused(reason) => CallOutcome::Refused(reason),
        CallEvent::Failed(reason) => CallOutcome::ServerLost(reason),
        CallEvent::Invalid(check) => CallOutcome::ServerLost(format!("invalid response: {check}")),
        CallEvent::Lost => CallOutcome::ServerLost("the server left".to_owned()),
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
    (call.done, result)
}
