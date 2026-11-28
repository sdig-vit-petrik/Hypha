//! A server in a plain node or an `ApplicationNode`: each request reaches a handler on the
//! `WaitSet` thread and ends in one answer (`docs/design/native-services.md` §2).
//!
//! The handler replies, refuses, or defers the request into a handle that any thread may
//! answer later. A request left without any of the three is refused when the handle goes:
//! the one rule for a callback that returned and for a deferred handle that was destroyed.
//! A handler that failed answers its open request as failed, which the client reads as
//! `ServerLost`.
//!
//! A deferred handle holds a weak reference to the server and the index of its slot, never
//! the request: destroying the server ends every slot at once, and its clients see it.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use hypha_core::port::ReplyError;
use hypha_iceoryx2::{Iox2Server, RequestHandle};

use crate::node::HyphaNode;
use crate::status::Error;
use crate::waitset::{HYPHA_ACTION_FAIL, HyphaCallbackResult};

use super::spec::{Role, ServiceDecl};

/// What a server shares with the deferred handles of its requests.
pub(crate) struct ServerCore {
    name: String,
    /// `None` once the server was destroyed: every slot ended with it.
    port: Mutex<Option<Iox2Server>>,
    /// Requests left unanswered and refused for it.
    unanswered: AtomicU64,
    /// Whether the first unanswered request was logged.
    unanswered_logged: AtomicBool,
    /// Whether the server's last receive failed, so a failure is logged when it starts.
    receive_failing: AtomicBool,
}

/// A server, owned by its caller. Dropping it ends every request it holds: their clients
/// read `ServerLost`, and a deferred handle's later answer is refused with `Disconnected`.
pub struct HyphaServer {
    core: Arc<ServerCore>,
    node: Weak<HyphaNode>,
    attachment: crate::waitset::HyphaAttachmentId,
}

impl HyphaServer {
    /// The resolved service name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.core.name
    }
}

/// One request a handler received: its payload and who sent it. Borrowed for the call.
#[derive(Debug, Clone, Copy)]
pub struct RequestView<'a> {
    /// The encoded request.
    pub payload: &'a [u8],
    /// The client's node name; empty when it cannot be read.
    pub client_node: &'a str,
}

/// A handler of a server's requests, owned by its registration. It runs on the thread
/// that runs the `WaitSet`. A handler that fails returns a failing result.
pub trait ServeHandler: Send {
    /// Handle one request: answer `request`, defer it, or leave it to be refused.
    fn serve(&mut self, request: &mut HyphaRequest, view: &RequestView<'_>) -> HyphaCallbackResult;
}

/// What an answer says.
#[derive(Debug, Clone, Copy)]
pub enum Answer<'a> {
    /// The encoded response.
    Reply(&'a [u8]),
    /// A refusal with its reason, which may be empty.
    Refuse(&'a str),
}

/// A request of a server: borrowed by a handler for its call or by a cycle step, or
/// deferred and owned by its caller, who may answer it from any thread. Dropped open, it
/// refuses the request.
pub struct HyphaRequest {
    server: Weak<ServerCore>,
    handle: RequestHandle,
    open: bool,
    kind: Kind,
    /// The answer a step gave it, sent after the step.
    staged: Option<OwnedAnswer>,
}

/// Who holds a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A handler, for its call.
    Handler,
    /// A cycle step, which read it: its answer leaves after the step.
    Step,
    /// Its caller, who deferred it.
    Deferred,
}

/// An answer kept until the step that gave it ends.
#[derive(Debug, Clone)]
pub(crate) enum OwnedAnswer {
    Reply(Vec<u8>),
    Refuse(String),
}

impl OwnedAnswer {
    fn of(answer: Answer<'_>) -> Self {
        match answer {
            Answer::Reply(bytes) => Self::Reply(bytes.to_vec()),
            Answer::Refuse(reason) => Self::Refuse(reason.to_owned()),
        }
    }
}

/// A deferred request answered during a cycle step: it leaves after the step.
struct StagedAnswer {
    server: Weak<ServerCore>,
    handle: RequestHandle,
    answer: OwnedAnswer,
}

thread_local! {
    /// The answers of deferred requests given during the cycle step that runs on this
    /// thread; `None` outside a step, where an answer leaves at once.
    static STEP_ANSWERS: RefCell<Option<Vec<StagedAnswer>>> = const { RefCell::new(None) };
}

/// Start keeping the answers of deferred requests given on this thread, for a cycle step.
pub(crate) fn begin_step_answers() {
    STEP_ANSWERS.with(|answers| *answers.borrow_mut() = Some(Vec::new()));
}

/// End the cycle step on this thread: send the deferred answers it gave, stamped `stamp`,
/// when it succeeded; answer them as failed when it failed (FSR-38).
pub(crate) fn end_step_answers(succeeded: bool, stamp: i64) {
    let staged = STEP_ANSWERS.with(|answers| answers.borrow_mut().take()).unwrap_or_default();
    for StagedAnswer { server, handle, answer } in staged {
        let sent = if succeeded {
            send_owned(&server, handle, &answer, stamp)
        } else {
            fail_handle(&server, handle, STEP_FAILED, stamp)
        };
        log_unsent(&server, sent);
    }
}

/// Log an answer that could not leave after its step: the step's `reply()` had returned.
fn log_unsent(server: &Weak<ServerCore>, sent: Option<Result<(), ReplyError>>) {
    let name = server.upgrade().map_or_else(String::new, |core| core.name.clone());
    match sent {
        // Sent; or too large, refused to the client with the sizes and reported to the step
        // at reply().
        Some(Ok(()) | Err(ReplyError::TooLarge { .. })) => {}
        // The client left: it has its outcome already, and the server counts it abandoned.
        Some(Err(ReplyError::Disconnected | ReplyError::UnknownRequest)) => {
            log::info!(target: "hypha", "hypha: service '{name}': an answer given in a step found its client gone");
        }
        Some(Err(ReplyError::Port(cause))) => {
            log::warn!(target: "hypha", "hypha: service '{name}': an answer given in a step could not be sent after it: {cause}");
        }
        None => {
            log::info!(target: "hypha", "hypha: service '{name}': an answer given in a step found its server gone");
        }
    }
}

/// The reason a client reads for a request whose server step failed: `ServerLost`.
pub(crate) const STEP_FAILED: &str = "server step failed";

fn send_owned(
    server: &Weak<ServerCore>,
    handle: RequestHandle,
    answer: &OwnedAnswer,
    stamp: i64,
) -> Option<Result<(), ReplyError>> {
    let stamp = u64::try_from(stamp).unwrap_or(0);
    with_port(server, |port| match answer {
        OwnedAnswer::Reply(response) => port.reply(handle, response, stamp),
        OwnedAnswer::Refuse(reason) => port.refuse(handle, reason, stamp),
    })
}

fn fail_handle(
    server: &Weak<ServerCore>,
    handle: RequestHandle,
    reason: &str,
    stamp: i64,
) -> Option<Result<(), ReplyError>> {
    let stamp = u64::try_from(stamp).unwrap_or(0);
    with_port(server, |port| port.fail(handle, reason, stamp))
}

impl HyphaRequest {
    /// A request that a cycle step of `server` read.
    pub(crate) fn of_step(server: &Arc<ServerCore>, handle: RequestHandle) -> Self {
        Self { server: Arc::downgrade(server), handle, open: true, kind: Kind::Step, staged: None }
    }

    /// End a request a cycle step read, after the step: send its answer stamped `stamp`
    /// when the step `succeeded`, or refuse it when the step left it unanswered; answer it
    /// as failed when the step failed. A deferred request is the deferred handle's.
    pub(crate) fn end_step(&mut self, succeeded: bool, stamp: i64) {
        let staged = self.staged.take();
        let open = std::mem::replace(&mut self.open, false);
        if !open && staged.is_none() {
            return;
        }
        if !succeeded {
            log_unsent(&self.server, fail_handle(&self.server, self.handle, STEP_FAILED, stamp));
            return;
        }
        let answer = staged.unwrap_or_else(|| {
            self.count_unanswered("a cycle step read the request and did not answer it");
            OwnedAnswer::Refuse("the server's step read the request and did not answer it".into())
        });
        log_unsent(&self.server, send_owned(&self.server, self.handle, &answer, stamp));
    }

    fn count_unanswered(&self, why: &str) {
        let Some(core) = self.server.upgrade() else {
            return;
        };
        core.unanswered.fetch_add(1, Ordering::Relaxed);
        if !core.unanswered_logged.swap(true, Ordering::AcqRel) {
            log::warn!(target: "hypha", "hypha: service '{}': {why}; it is refused (logged once per server)", core.name);
        }
    }

    /// Answer it as failed, when its handler failed.
    fn fail(&mut self, reason: &str) {
        if !self.open {
            return;
        }
        self.open = false;
        let _ = with_port(&self.server, |port| port.fail(self.handle, reason, framework_nanos()));
    }
}

impl Drop for HyphaRequest {
    fn drop(&mut self) {
        if !self.open {
            return;
        }
        self.open = false;
        let reason = match self.kind {
            Kind::Handler => "the server's handler did not answer the request",
            Kind::Step => "the server's step read the request and did not answer it",
            Kind::Deferred => "a deferred request was destroyed unanswered",
        };
        self.count_unanswered(reason);
        let _ = with_port(&self.server, |port| port.refuse(self.handle, reason, framework_nanos()));
    }
}

/// Answer `request` with `answer`. Any thread may answer a deferred request; the answer is
/// stamped with the process's framework time. Afterwards the request is answered, whatever
/// the result.
///
/// The answer to a request a cycle step read, and to a deferred request answered during a
/// cycle step on this thread, leaves after the step, stamped with its time; a failed step
/// answers it as failed instead (`docs/design/native-services.md` §2).
///
/// # Errors
/// `InvalidState` for a request answered or deferred already; `TooLarge` for a response
/// above the bound (the client was refused with the sizes); `Disconnected` when the client
/// or the server is gone; the transport's error otherwise. A staged answer reports only
/// `InvalidState`, `TooLarge` and `Disconnected` for a server that is gone.
pub fn request_answer(request: &mut HyphaRequest, answer: Answer<'_>) -> Result<(), Error> {
    if !request.open {
        return Err(Error::InvalidState);
    }
    request.open = false;
    let handle = request.handle;
    let in_step = STEP_ANSWERS.with(|answers| answers.borrow().is_some());
    if request.kind == Kind::Step || (request.kind == Kind::Deferred && in_step) {
        return stage(request, answer);
    }
    let sent = with_port(&request.server, |port| match answer {
        Answer::Reply(response) => port.reply(handle, response, framework_nanos()),
        Answer::Refuse(reason) => port.refuse(handle, reason, framework_nanos()),
    });
    match sent {
        Some(Ok(())) => Ok(()),
        Some(Err(ReplyError::TooLarge { requested, limit })) => {
            log::warn!(target: "hypha", "hypha: a response of {requested} B is above the bound of {limit} B; its client is refused");
            Err(Error::TooLarge)
        }
        // No server any more; or a slot whose client left, dropped each turn, so its handle
        // names nothing.
        None | Some(Err(ReplyError::Disconnected | ReplyError::UnknownRequest)) => {
            Err(Error::Disconnected)
        }
        Some(Err(ReplyError::Port(cause))) => Err(Error::from(cause)),
    }
}

/// Keep `answer` until the step ends: in the step's request, or, for a deferred request,
/// in the step's stage of this thread.
fn stage(request: &mut HyphaRequest, answer: Answer<'_>) -> Result<(), Error> {
    let Some(core) = request.server.upgrade() else {
        return Err(Error::Disconnected);
    };
    let bound = crate::locked(&core.port).as_ref().map(|port| port.policy().max_response_bytes);
    let Some(bound) = bound else {
        return Err(Error::Disconnected);
    };
    let too_large = matches!(answer, Answer::Reply(response) if response.len() > bound);
    let owned = OwnedAnswer::of(answer);
    if request.kind == Kind::Step {
        request.staged = Some(owned);
    } else {
        let staged = StagedAnswer {
            server: Weak::clone(&request.server),
            handle: request.handle,
            answer: owned,
        };
        STEP_ANSWERS.with(|answers| {
            if let Some(answers) = answers.borrow_mut().as_mut() {
                answers.push(staged);
            }
        });
    }
    if too_large {
        // The send after the step refuses the client with the sizes, as an answer at once does.
        log::warn!(target: "hypha", "hypha: service '{}': a response above the bound of {bound} B; its client is refused", core.name);
        return Err(Error::TooLarge);
    }
    Ok(())
}

/// Take `request` over from its handler or its step, to answer later from any thread. The
/// handler's request is then answered as far as the handler is concerned.
///
/// # Errors
/// `InvalidState` for a request answered or deferred already, or one deferred before.
pub fn request_defer(request: &mut HyphaRequest) -> Result<Box<HyphaRequest>, Error> {
    if !request.open || request.kind == Kind::Deferred {
        return Err(Error::InvalidState);
    }
    request.open = false;
    Ok(Box::new(HyphaRequest {
        server: Weak::clone(&request.server),
        handle: request.handle,
        open: true,
        kind: Kind::Deferred,
        staged: None,
    }))
}

/// Create the server of `decl` on `node` and attach it to the node's `WaitSet`, whose run
/// calls `handler` for each request. Create it before the run.
///
/// # Errors
/// The errors of the declaration (`InvalidArgument`, `InvalidTopic`); `ResourceLimit` when
/// a live node serves the name; `QosMismatch` for a stream of another policy;
/// `InvalidState` during a run; the transport's error otherwise. Each is logged with its
/// reason and recorded for `last_error`. `handler` drops on a refusal.
pub fn service_serve(
    node: &Arc<HyphaNode>,
    decl: &ServiceDecl<'_>,
    handler: Box<dyn ServeHandler>,
) -> Result<HyphaServer, Error> {
    crate::last_error::clear();
    let resolved = super::spec::resolve(node, decl, Role::Server)?;
    let name = resolved.spec.name.clone();
    let port = node
        .transport
        .serve(&resolved.spec)
        .map_err(|cause| super::spec::refuse_spec(&name, &cause))?;
    let listener = port.open_listener().map_err(Error::from)?;
    let core = ServerCore::new(name, port);
    let target = crate::waitset::ServiceTarget::Server { core: Arc::clone(&core), handler };
    let attachment = crate::waitset::attach_service(node, listener, target)?;
    Ok(HyphaServer { core, node: Arc::downgrade(node), attachment })
}

/// Destroy `server`: every request it holds ends, and it is detached from its `WaitSet`.
pub fn service_server_destroy(server: HyphaServer) {
    drop(server);
}

impl Drop for HyphaServer {
    fn drop(&mut self) {
        drop(crate::locked(&self.core.port).take());
        if let Some(node) = self.node.upgrade() {
            crate::waitset::detach_service(&node, self.attachment);
        }
    }
}

/// The number of requests `server` refused because none answered them.
#[must_use]
pub fn server_unanswered(server: &HyphaServer) -> u64 {
    server.core.unanswered.load(Ordering::Relaxed)
}

/// Run `body` on the server's port, or `None` once the server is gone.
fn with_port<R>(server: &Weak<ServerCore>, body: impl FnOnce(&mut Iox2Server) -> R) -> Option<R> {
    let core = server.upgrade()?;
    let mut port = crate::locked(&core.port);
    port.as_mut().map(body)
}

fn framework_nanos() -> u64 {
    u64::try_from(hypha_time::now().nanos()).unwrap_or(0)
}

impl ServerCore {
    /// Serve up to `batch` requests with `handler`. Returns the answer of a handler that
    /// did not continue the run, and whether requests may be left.
    pub(crate) fn serve_turn(
        self: &Arc<Self>,
        handler: &mut dyn ServeHandler,
        batch: u32,
    ) -> (Option<HyphaCallbackResult>, bool) {
        for _ in 0..batch {
            let Some((received, payload)) = self.receive() else {
                return (None, false);
            };
            if hypha_core::port::is_cli_node_name(&received.client_node) {
                log::info!(target: "hypha", "hypha: service '{}': called by the tool '{}'", self.name, received.client_node);
            }
            let mut request = HyphaRequest {
                server: Arc::downgrade(self),
                handle: received.handle,
                open: true,
                kind: Kind::Handler,
                staged: None,
            };
            let view = RequestView { payload: &payload, client_node: &received.client_node };
            let result = handler.serve(&mut request, &view);
            let failed = result.action == HYPHA_ACTION_FAIL || result.status != 0;
            if failed {
                request.fail("the server's handler failed");
            }
            // An open request is refused as it drops.
            drop(request);
            if result.action != crate::waitset::HYPHA_ACTION_CONTINUE || failed {
                return (Some(result), true);
            }
        }
        (None, true)
    }

    /// The next request and a copy of its payload, so no lock is held while it is served.
    pub(crate) fn receive(&self) -> Option<(hypha_iceoryx2::ReceivedRequest, Vec<u8>)> {
        let mut port = crate::locked(&self.port);
        let port = port.as_mut()?;
        match port.receive() {
            Ok(Some(received)) => {
                self.receive_failing.store(false, Ordering::Relaxed);
                let payload = port.payload(received.handle).map_or_else(Vec::new, <[u8]>::to_vec);
                Some((received, payload))
            }
            Ok(None) => None,
            Err(cause) => {
                if !self.receive_failing.swap(true, Ordering::Relaxed) {
                    log::error!(target: "hypha", "hypha: service '{}': cannot receive requests: {cause}", self.name);
                }
                None
            }
        }
    }

    /// The resolved service name.
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    /// Push the node id of each client of the stream now onto `ids`.
    pub(crate) fn client_node_ids(&self, ids: &mut Vec<u128>) {
        if let Some(port) = crate::locked(&self.port).as_ref() {
            port.client_node_ids(ids);
        }
    }

    /// A new server core for `port`, named `name`.
    pub(crate) fn new(name: String, port: Iox2Server) -> Arc<Self> {
        Arc::new(Self {
            name,
            port: Mutex::new(Some(port)),
            unanswered: AtomicU64::new(0),
            unanswered_logged: AtomicBool::new(false),
            receive_failing: AtomicBool::new(false),
        })
    }

    /// End every request the server holds: its clients read `ServerLost`.
    pub(crate) fn close(&self) {
        drop(crate::locked(&self.port).take());
    }

    /// Drop the requests whose client left. Each turn.
    pub(crate) fn drop_abandoned(&self) {
        if let Some(port) = crate::locked(&self.port).as_mut() {
            port.drop_abandoned();
        }
    }
}
