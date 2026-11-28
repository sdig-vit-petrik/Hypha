//! The C ABI of services: the C wrappers of `hypha_node::service`.
//!
//! The boundary is coarse on purpose. A server is one create with its handler; a request
//! is answered, deferred or destroyed; a client is one create and one call, whose kind
//! (attached or blocking) the client decides. Every rule of a call (port tables, local
//! refusals, outcomes, timeouts, the refusal of a blocking call on a dispatch thread, the
//! refusal of an unanswered request) is decided below this layer, once.
//!
//! A context passed with a `release` is owned by the library from the call on, and
//! `release` runs exactly once, also when the call is refused.

use std::os::raw::{c_char, c_void};
use std::time::Duration;

use hypha_core::port::ServicePolicy;
use hypha_node::HyphaCallbackResult;
use hypha_node::safe::{
    Answer, CallDone, CallResult, HyphaClient, HyphaNode, HyphaRequest, HyphaServer, RequestView,
    ServeHandler, ServiceDecl,
};
use hypha_node::status::HyphaStatus;
use hypha_node::waitset::HyphaContextRelease;
use hypha_node::{HyphaCycleEmission, safe::EmissionView};

use crate::cycle::{CycleTarget, HyphaCycle};

use crate::boundary::{
    borrow_bytes, borrow_str, handle_mut, handle_ref, read_in, take_box, write_out,
};
use crate::status::try_guard;

/// Answer a request with a response: `bytes` is the encoded response.
pub const HYPHA_ANSWER_REPLY: u32 = 0;
/// Refuse a request: `bytes` is the reason, UTF-8, possibly empty.
pub const HYPHA_ANSWER_REFUSE: u32 = 1;

/// The limits of a service. A `name_len` of 0 selects `defaultService` and every other
/// field must then be 0.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct HyphaServicePolicy {
    /// The policy's name, `name_len` bytes of UTF-8; not NUL-terminated.
    pub name: *const c_char,
    /// Bytes of `name`.
    pub name_len: usize,
    /// Clients the service admits, beyond the three for tools.
    pub clients: u32,
    /// Calls one client may have in flight.
    pub outstanding: u32,
    /// The largest request payload, in bytes.
    pub request_bytes: u64,
    /// The largest response payload, in bytes.
    pub response_bytes: u64,
    /// A call's timeout when its client names none.
    pub timeout_nanos: u64,
}

/// What a server or a client is made of. Each text is `*_len` bytes of UTF-8, not
/// NUL-terminated, borrowed for the call.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct HyphaServiceDesc {
    /// The port, whose `[ports.servers.<port>]` or `[ports.clients.<port>]` table names the
    /// service; or, with `literal` 1, the service: absolute, relative or `~/`.
    pub name: *const c_char,
    /// Bytes of `name`.
    pub name_len: usize,
    /// 1 when `name` is the service, 0 when it is a port.
    pub literal: u8,
    /// The `.srv` type, as `std_srvs/SetBool`.
    pub type_name: *const c_char,
    /// Bytes of `type_name`.
    pub type_name_len: usize,
    /// The `.srv` MD5: 32 lower-case hexadecimal digits.
    pub md5: *const c_char,
    /// Bytes of `md5`: 32.
    pub md5_len: usize,
    /// The limits, which both ends of the service declare alike.
    pub policy: HyphaServicePolicy,
    /// A client's timeout; 0 for the policy's. A client table's `timeout_ms` wins over both.
    pub timeout_nanos: u64,
    /// A client: 1 for a blocking client, attached to nothing, whose call waits on the
    /// calling thread. 0 for a client of the node's `WaitSet`. A server: 0.
    pub blocking: u8,
    /// A server: the definition of the request half (`<type>Request`) with its
    /// dependencies, registered in the schema store so tools can encode a request. Null
    /// with length 0 for none. A client: unused.
    pub request_definition: *const c_char,
    /// Bytes of `request_definition`.
    pub request_definition_len: usize,
    /// A server: the definition of the response half; null with length 0 for none.
    pub response_definition: *const c_char,
    /// Bytes of `response_definition`.
    pub response_definition_len: usize,
}

/// One request, borrowed for the handler's call.
#[repr(C)]
pub struct HyphaRequestView {
    /// Answer it with `hypha_request_answer`, or take it over with `hypha_request_defer`;
    /// left alone, it is refused when the handler returns. Valid during the call only.
    pub request: *mut HyphaRequest,
    /// The encoded request.
    pub payload: *const u8,
    /// Bytes of `payload`.
    pub payload_len: usize,
    /// The client's node name; empty when it cannot be read.
    pub client_node: *const c_char,
    /// Bytes of `client_node`.
    pub client_node_len: usize,
}

/// The one outcome of a call, borrowed for the outcome callback.
#[repr(C)]
pub struct HyphaCallResult {
    /// One of the `HYPHA_CALL_ANSWERED` ... `HYPHA_CALL_TIMEOUT` outcomes.
    pub outcome: u32,
    /// 1 when the server may have run the request: answered, server lost, timeout.
    pub may_have_run: u8,
    /// 1 when a blind retry is safe: refused, no server, not sent.
    pub safe_to_retry: u8,
    /// The encoded response of an answered call; null and 0 otherwise.
    pub response: *const u8,
    /// Bytes of `response`.
    pub response_len: usize,
    /// What became of the call, as one line: `refused: not armed`,
    /// `timeout after 1 s (policy defaultService)`.
    pub description: *const c_char,
    /// Bytes of `description`.
    pub description_len: usize,
    /// The client's framework time of the call, in nanoseconds.
    pub sent_nanos: i64,
    /// The client's framework time of the outcome, in nanoseconds.
    pub done_nanos: i64,
}

/// What `hypha_client_status` reads.
#[repr(C)]
pub struct HyphaClientStatus {
    /// 1 when a live server of the client's stream exists now.
    pub ready: u8,
    /// Calls whose outcome is still to come.
    pub pending: u32,
}

/// Called for each request of a server, on the thread that runs the node's `WaitSet`.
/// A failing result answers an open request as failed (the client reads `ServerLost`) and
/// fails the run.
///
/// # Safety
/// Must not unwind. `view` is valid only for the call.
pub type HyphaServeCallback = Option<
    extern "C" fn(view: *const HyphaRequestView, context: *mut c_void) -> HyphaCallbackResult,
>;

/// Called once with the outcome of a call. For a client of the node's `WaitSet` it runs on
/// that `WaitSet`'s thread, and a failing result fails the run; for a blocking client it
/// runs on the calling thread before `hypha_client_call` returns, and its result is ignored.
///
/// # Safety
/// Must not unwind. `result` is valid only for the call.
pub type HyphaCallDoneCallback = Option<
    extern "C" fn(result: *const HyphaCallResult, context: *mut c_void) -> HyphaCallbackResult,
>;

/// A caller's context and its release, which runs once when this drops.
struct Context {
    pointer: *mut c_void,
    release: HyphaContextRelease,
}

// SAFETY: by the caller's contract the context may be used from the thread that runs the
// `WaitSet`, or, for a blocking client, the calling thread; one at a time.
unsafe impl Send for Context {}

impl Drop for Context {
    fn drop(&mut self) {
        if let Some(release) = self.release {
            release(self.pointer);
        }
    }
}

struct CServe {
    function: extern "C" fn(*const HyphaRequestView, *mut c_void) -> HyphaCallbackResult,
    context: Context,
}

impl ServeHandler for CServe {
    fn serve(&mut self, request: &mut HyphaRequest, view: &RequestView<'_>) -> HyphaCallbackResult {
        let view = HyphaRequestView {
            request: std::ptr::from_mut(request),
            payload: view.payload.as_ptr(),
            payload_len: view.payload.len(),
            client_node: view.client_node.as_ptr().cast(),
            client_node_len: view.client_node.len(),
        };
        (self.function)(&raw const view, self.context.pointer)
    }
}

struct CDone {
    function: extern "C" fn(*const HyphaCallResult, *mut c_void) -> HyphaCallbackResult,
    context: Context,
}

impl CallDone for CDone {
    fn done(self: Box<Self>, result: &CallResult) -> HyphaCallbackResult {
        let pointer = self.context.pointer;
        deliver_result(self.function, result, pointer)
    }
}

/// Call `function` with the C view of `result` and `context`.
fn deliver_result(
    function: extern "C" fn(*const HyphaCallResult, *mut c_void) -> HyphaCallbackResult,
    result: &CallResult,
    context: *mut c_void,
) -> HyphaCallbackResult {
    let description = result.outcome.to_string();
    let response = result.outcome.response();
    let view = HyphaCallResult {
        outcome: result.outcome.code(),
        may_have_run: u8::from(result.outcome.may_have_run()),
        safe_to_retry: u8::from(result.outcome.safe_to_retry()),
        response: response.map_or(std::ptr::null(), <[u8]>::as_ptr),
        response_len: response.map_or(0, <[u8]>::len),
        description: description.as_ptr().cast(),
        description_len: description.len(),
        sent_nanos: result.sent_nanos,
        done_nanos: result.done_nanos,
    };
    function(&raw const view, context)
}

/// The declaration `desc` describes, borrowing its texts.
///
/// # Safety
/// Each text of `desc` points to its length of readable bytes.
pub(crate) unsafe fn declaration(desc: &HyphaServiceDesc) -> Result<ServiceDecl<'_>, HyphaStatus> {
    let text = |pointer: *const c_char, len: usize| unsafe { borrow_str(pointer, len) };
    // A definition is optional: null with length 0 is none.
    let optional = |pointer: *const c_char, len: usize| {
        if pointer.is_null() && len == 0 { Ok("") } else { text(pointer, len) }
    };
    let policy = &desc.policy;
    let policy = if policy.name_len == 0 {
        let cleared = policy.clients == 0
            && policy.outstanding == 0
            && policy.request_bytes == 0
            && policy.response_bytes == 0
            && policy.timeout_nanos == 0;
        if !cleared {
            return Err(HyphaStatus::InvalidArgument);
        }
        None
    } else {
        let bytes = |value: u64| usize::try_from(value).map_err(|_| HyphaStatus::InvalidArgument);
        Some(ServicePolicy {
            name: text(policy.name, policy.name_len)?.to_owned(),
            max_clients: usize::try_from(policy.clients)
                .map_err(|_| HyphaStatus::InvalidArgument)?,
            max_outstanding: usize::try_from(policy.outstanding)
                .map_err(|_| HyphaStatus::InvalidArgument)?,
            max_request_bytes: bytes(policy.request_bytes)?,
            max_response_bytes: bytes(policy.response_bytes)?,
            timeout: Duration::from_nanos(policy.timeout_nanos),
        })
    };
    Ok(ServiceDecl {
        name: text(desc.name, desc.name_len)?,
        literal: desc.literal != 0,
        type_name: text(desc.type_name, desc.type_name_len)?,
        md5: text(desc.md5, desc.md5_len)?,
        request_definition: optional(desc.request_definition, desc.request_definition_len)?,
        response_definition: optional(desc.response_definition, desc.response_definition_len)?,
        policy,
        timeout: (desc.timeout_nanos != 0).then(|| Duration::from_nanos(desc.timeout_nanos)),
    })
}

/// Create the server `desc` describes and attach it to the node's `WaitSet`, whose run
/// calls `serve` with `context` for each request. Create it before the run. `release`
/// runs once with `context`: when the server is destroyed, or at once on a refusal.
///
/// `InvalidArgument`: a null argument, text that is not UTF-8, a policy with a name of
/// length 0 and a field that is not 0, `blocking` not 0, or a port with no table (the
/// reason names the stanza to add); `InvalidTopic`: an invalid service name;
/// `ResourceLimit`: a live node serves the name (the reason names it); `QosMismatch`: the
/// service exists with another policy; `InvalidState`: during a run. `hypha_last_error`
/// gives the reason of each.
///
/// # Safety
/// `node` is live; `desc` is readable with its texts; `out` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_service_server_create(
    node: *mut HyphaNode,
    desc: *const HyphaServiceDesc,
    serve: HyphaServeCallback,
    context: *mut c_void,
    release: HyphaContextRelease,
    out: *mut *mut HyphaServer,
) -> HyphaStatus {
    let context = Context { pointer: context, release };
    try_guard(|| {
        unsafe { write_out(out, std::ptr::null_mut()) }?;
        let function = serve.ok_or(HyphaStatus::InvalidArgument)?;
        let desc = unsafe { read_in(desc) }?;
        if node.is_null() || desc.blocking != 0 {
            return Err(HyphaStatus::InvalidArgument);
        }
        let decl = unsafe { declaration(&desc) }?;
        let shared = unsafe { crate::waitset::configuration::share_node(node) };
        let handler = Box::new(CServe { function, context });
        let server = hypha_node::safe::service_serve(&shared, &decl, handler)?;
        unsafe { write_out(out, Box::into_raw(Box::new(server))) }
    })
}

/// Destroy a server: every request it holds ends (their clients read `ServerLost`, and a
/// deferred request's later answer is `Disconnected`), and its handler's context is
/// released. Null does nothing.
///
/// # Safety
/// `server` is null or comes from `hypha_service_server_create`, used no more.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_service_server_destroy(server: *mut HyphaServer) {
    if let Some(server) = unsafe { take_box(server) } {
        hypha_node::safe::service_server_destroy(*server);
    }
}

/// Answer `request`: `kind` `HYPHA_ANSWER_REPLY` with the encoded response in `bytes`, or
/// `HYPHA_ANSWER_REFUSE` with the reason. A request is answered once, from the handler or,
/// deferred, from any thread; afterwards it is answered whatever the status.
///
/// `InvalidState`: answered or deferred already; `TooLarge`: a response above the bound
/// (the client is refused with the sizes); `Disconnected`: the client or the server is
/// gone; `InvalidArgument`: a null argument, another kind, a reason that is not UTF-8.
///
/// # Safety
/// `request` is the handler's request during its call, or a live deferred one; `bytes`
/// points to `len` readable bytes (it may be null when `len` is 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_request_answer(
    request: *mut HyphaRequest,
    kind: u32,
    bytes: *const u8,
    len: usize,
) -> HyphaStatus {
    try_guard(|| {
        let request = unsafe { handle_mut(request) }?;
        let bytes = if len == 0 { &[][..] } else { unsafe { borrow_bytes(bytes, len) }? };
        let answer = match kind {
            HYPHA_ANSWER_REPLY => Answer::Reply(bytes),
            HYPHA_ANSWER_REFUSE => Answer::Refuse(
                std::str::from_utf8(bytes).map_err(|_| HyphaStatus::InvalidArgument)?,
            ),
            _ => return Err(HyphaStatus::InvalidArgument),
        };
        Ok(hypha_node::safe::request_answer(request, answer)?)
    })
}

/// Take the handler's `request` over, to answer later from any thread: `out` receives the
/// deferred request, which `hypha_request_destroy` releases. The handler's request is then
/// answered. `InvalidState`: answered, deferred already, or itself a deferred request.
///
/// # Safety
/// `request` is the handler's request during its call; `out` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_request_defer(
    request: *mut HyphaRequest,
    out: *mut *mut HyphaRequest,
) -> HyphaStatus {
    try_guard(|| {
        unsafe { write_out(out, std::ptr::null_mut()) }?;
        let request = unsafe { handle_mut(request) }?;
        let deferred = hypha_node::safe::request_defer(request)?;
        unsafe { write_out(out, Box::into_raw(deferred)) }
    })
}

/// Release a deferred request. One still unanswered is refused, and counted and logged
/// once per server. Any thread may call it. Null does nothing.
///
/// # Safety
/// `request` is null or comes from `hypha_request_defer`, used no more.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_request_destroy(request: *mut HyphaRequest) {
    drop(unsafe { take_box(request) });
}

/// Create the client `desc` describes. A server need not exist. With `blocking` 0 it is
/// attached to the node's `WaitSet`, whose run delivers its outcomes; create it before
/// the run. With `blocking` 1 it is attached to nothing.
///
/// `InvalidArgument`, `InvalidTopic`, `QosMismatch`, `InvalidState`: as for
/// `hypha_service_server_create`; `ResourceLimit`: the stream's clients of this kind are
/// at their limit (a tool node takes one of three slots of its own).
///
/// # Safety
/// `node` is live; `desc` is readable with its texts; `out` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_service_client_create(
    node: *mut HyphaNode,
    desc: *const HyphaServiceDesc,
    out: *mut *mut HyphaClient,
) -> HyphaStatus {
    try_guard(|| {
        unsafe { write_out(out, std::ptr::null_mut()) }?;
        let desc = unsafe { read_in(desc) }?;
        if node.is_null() || desc.blocking > 1 {
            return Err(HyphaStatus::InvalidArgument);
        }
        let decl = unsafe { declaration(&desc) }?;
        let shared = unsafe { crate::waitset::configuration::share_node(node) };
        let client = hypha_node::safe::service_client_create(&shared, &decl, desc.blocking == 1)?;
        unsafe { write_out(out, Box::into_raw(Box::new(client))) }
    })
}

/// Destroy a client. The outcomes of its calls in flight are dropped uncalled, and their
/// contexts released. Null does nothing.
///
/// # Safety
/// `client` is null or comes from `hypha_service_client_create`, used no more.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_service_client_destroy(client: *mut HyphaClient) {
    if let Some(client) = unsafe { take_box(client) } {
        hypha_node::safe::service_client_destroy(*client);
    }
}

/// Call with the encoded `request`. `status` receives `HYPHA_CALL_SENT`: `done` runs once
/// with the outcome, on the `WaitSet` thread at a later turn, or, for a blocking client, on
/// this thread before the call returns. Or `HYPHA_CALL_BUSY` (the client has its limit of
/// calls in flight) or `HYPHA_CALL_TOO_LARGE` (the request is above the bound): nothing is
/// sent and `done` is not called. `release` runs once with `context` in every case.
///
/// `InvalidState`: a blocking client called on a thread that runs a `WaitSet` dispatch
/// (that thread would deliver the answer it waits for; `hypha_last_error` says what to use
/// instead). `InvalidArgument`: a null argument.
///
/// # Safety
/// `client` is live; `request` points to `len` readable bytes (null when `len` is 0);
/// `status` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_client_call(
    client: *const HyphaClient,
    request: *const u8,
    len: usize,
    done: HyphaCallDoneCallback,
    context: *mut c_void,
    release: HyphaContextRelease,
    status: *mut u32,
) -> HyphaStatus {
    let context = Context { pointer: context, release };
    try_guard(|| {
        let client = unsafe { handle_ref(client) }?;
        let function = done.ok_or(HyphaStatus::InvalidArgument)?;
        crate::boundary::require_non_null(status)?;
        let request = if len == 0 { &[][..] } else { unsafe { borrow_bytes(request, len) }? };
        let called =
            hypha_node::safe::client_call(client, request, Box::new(CDone { function, context }))?;
        unsafe { write_out(status, called.code()) }
    })
}

/// Read whether a live server of the client's stream exists now, and how many of its calls
/// are pending. Any thread may call it.
///
/// # Safety
/// `client` is live; `out` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_client_status(
    client: *const HyphaClient,
    out: *mut HyphaClientStatus,
) -> HyphaStatus {
    try_guard(|| {
        let client = unsafe { handle_ref(client) }?;
        let status = hypha_node::safe::client_status(client);
        let pending = u32::try_from(status.pending).unwrap_or(u32::MAX);
        unsafe { write_out(out, HyphaClientStatus { ready: u8::from(status.ready), pending }) }
    })
}

/// Wait up to `timeout_nanos` of host time for a live server of a blocking client's
/// stream; `ready` receives 1 when one exists. `InvalidState`: a client of a `WaitSet`, or
/// a thread that runs a dispatch.
///
/// # Safety
/// `client` is live; `ready` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_client_wait_for_server(
    client: *const HyphaClient,
    timeout_nanos: u64,
    ready: *mut u8,
) -> HyphaStatus {
    try_guard(|| {
        let client = unsafe { handle_ref(client) }?;
        crate::boundary::require_non_null(ready)?;
        let found =
            hypha_node::safe::client_wait_for_server(client, Duration::from_nanos(timeout_nanos))?;
        unsafe { write_out(ready, u8::from(found)) }
    })
}

/// Bind the server `desc` describes to an uncommitted cycle, as a `Requests` port: the
/// cycle's steps read its requests with `hypha_cycle_requests` and answer them, and each
/// answer leaves after the step (`docs/design/native-services.md` §2). `out_index`
/// receives its index among the cycle's servers, from zero. Its table
/// (`[ports.servers.<port>]`) may set `per_step`, the requests one step takes (default 8).
/// The cycle owns the server: destroying the cycle ends every request it holds.
///
/// The errors of `hypha_service_server_create`, and `InvalidState` for a committed cycle
/// or a running `WaitSet`. `desc->blocking` must be 0.
///
/// # Safety
/// `cycle` is a live handle; `desc` is readable with its texts; `out_index` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_cycle_bind_server(
    cycle: *mut HyphaCycle,
    desc: *const HyphaServiceDesc,
    out_index: *mut u32,
) -> HyphaStatus {
    try_guard(|| {
        unsafe { write_out(out_index, 0) }?;
        let cycle = unsafe { CycleTarget::of(cycle) }?;
        let desc = unsafe { read_in(desc) }?;
        if desc.blocking != 0 {
            return Err(HyphaStatus::InvalidArgument);
        }
        let decl = unsafe { declaration(&desc) }?;
        let (set, cycle) = unsafe { cycle.waitset() }?;
        let index = hypha_node::safe::cycle_bind_server(&set, cycle, &decl)?;
        unsafe { write_out(out_index, index) }
    })
}

/// Bind a client of the service `desc` describes to an uncommitted cycle, as a `Calls`
/// port: a step stages calls with `hypha_cycle_call`, sent after the step succeeds, and a
/// later step reads their outcomes with `hypha_cycle_next_result`
/// (`docs/design/native-services.md` §3). `out_index` receives its index among the
/// cycle's clients, from zero. Its calls in flight are at most the table's `outstanding`,
/// else one. A server need not exist.
///
/// The errors of `hypha_service_client_create`, and `InvalidState` for a committed cycle
/// or a running `WaitSet`. `desc->blocking` must be 0.
///
/// # Safety
/// `cycle` is a live handle; `desc` is readable with its texts; `out_index` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_cycle_bind_client(
    cycle: *mut HyphaCycle,
    desc: *const HyphaServiceDesc,
    out_index: *mut u32,
) -> HyphaStatus {
    try_guard(|| {
        unsafe { write_out(out_index, 0) }?;
        let cycle = unsafe { CycleTarget::of(cycle) }?;
        let desc = unsafe { read_in(desc) }?;
        if desc.blocking != 0 {
            return Err(HyphaStatus::InvalidArgument);
        }
        let decl = unsafe { declaration(&desc) }?;
        let (set, cycle) = unsafe { cycle.waitset() }?;
        let index = hypha_node::safe::cycle_bind_client(&set, cycle, &decl)?;
        unsafe { write_out(out_index, index) }
    })
}

/// Read the requests of the step's server `server`: `each` runs once per request, in
/// order, with `context`; its result is ignored. On the first call of a step these are the
/// requests due (on simulated time, those stamped before the step's time, in the order of
/// stamp, client node, client port and sequence), at most the port's `per_step`; a later
/// call in the same step gives the same ones. Each request is answered with
/// `hypha_request_answer` or deferred with `hypha_request_defer` during the step; the
/// answer leaves after the step. One left alone is refused after the step; a failed step
/// answers every request it read as failed (the client reads `ServerLost`). A step that
/// never calls this reads none, and its requests stay queued.
///
/// `InvalidArgument`: a null argument or an unknown server.
///
/// # Safety
/// `emission` is the step's emission handle, during the step; `view->request` is valid for
/// the step.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_cycle_requests(
    emission: *mut HyphaCycleEmission,
    server: u32,
    each: HyphaServeCallback,
    context: *mut c_void,
) -> HyphaStatus {
    try_guard(|| {
        let function = each.ok_or(HyphaStatus::InvalidArgument)?;
        let view = unsafe { handle_mut(emission.cast::<EmissionView<'_, '_>>()) }?;
        for taken in hypha_node::safe::emission_requests(view, server)? {
            let request = HyphaRequestView {
                request: std::ptr::from_mut(taken.request.as_mut()),
                payload: taken.payload.as_ptr(),
                payload_len: taken.payload.len(),
                client_node: taken.client_node.as_ptr().cast(),
                client_node_len: taken.client_node.len(),
            };
            let _ = function(&raw const request, context);
        }
        Ok(())
    })
}

/// Read the next outcome of the step's client `client`, in call order: `found` receives 1
/// and `done` runs with it and `context` (its result ignored), or 0 when the step has read
/// every outcome it holds. A step holds the outcomes that came before it (on simulated
/// time, stamped before its time) and those of calls that timed out by its time; one it
/// does not read is dropped after it, and counted.
///
/// `InvalidArgument`: a null argument or an unknown client.
///
/// # Safety
/// `emission` is the step's emission handle, during the step; `found` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_cycle_next_result(
    emission: *mut HyphaCycleEmission,
    client: u32,
    done: HyphaCallDoneCallback,
    context: *mut c_void,
    found: *mut u8,
) -> HyphaStatus {
    try_guard(|| {
        unsafe { write_out(found, 0) }?;
        let function = done.ok_or(HyphaStatus::InvalidArgument)?;
        let view = unsafe { handle_mut(emission.cast::<EmissionView<'_, '_>>()) }?;
        let Some(result) = hypha_node::safe::emission_next_result(view, client)? else {
            return Ok(());
        };
        let _ = deliver_result(function, &result, context);
        unsafe { write_out(found, 1) }
    })
}

/// Stage a call of the step's client `client` with the encoded `request`: `status`
/// receives `HYPHA_CALL_SENT`, and the call leaves after the step succeeds, stamped with
/// its time (a failed step ends it `NotSent`, read by the next step); or
/// `HYPHA_CALL_BUSY` (the client has its limit of calls in flight, those staged in this
/// step included) or `HYPHA_CALL_TOO_LARGE`: nothing is sent and no outcome follows.
///
/// `InvalidArgument`: a null argument or an unknown client.
///
/// # Safety
/// `emission` is the step's emission handle, during the step; `request` points to `len`
/// readable bytes (null when `len` is 0); `status` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_cycle_call(
    emission: *mut HyphaCycleEmission,
    client: u32,
    request: *const u8,
    len: usize,
    status: *mut u32,
) -> HyphaStatus {
    try_guard(|| {
        unsafe { write_out(status, crate::HYPHA_CALL_BUSY) }?;
        let view = unsafe { handle_mut(emission.cast::<EmissionView<'_, '_>>()) }?;
        let request = if len == 0 { &[][..] } else { unsafe { borrow_bytes(request, len) }? };
        let called = hypha_node::safe::emission_call(view, client, request)?;
        unsafe { write_out(status, called.code()) }
    })
}

/// Read how many outcomes of the step's client `client` successful steps did not read and
/// dropped, since the client was bound: `out` receives the count, which wraps. A failed
/// step drops none: what it did not read goes to the next step.
///
/// `InvalidArgument`: a null argument or an unknown client.
///
/// # Safety
/// `emission` is the step's emission handle, during the step; `out` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_cycle_unread_results(
    emission: *mut HyphaCycleEmission,
    client: u32,
    out: *mut u64,
) -> HyphaStatus {
    try_guard(|| {
        unsafe { write_out(out, 0) }?;
        let view = unsafe { handle_mut(emission.cast::<EmissionView<'_, '_>>()) }?;
        let unread = hypha_node::safe::emission_unread_results(view, client)?;
        unsafe { write_out(out, unread) }
    })
}
