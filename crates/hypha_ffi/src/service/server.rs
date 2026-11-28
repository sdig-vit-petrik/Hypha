//! The C ABI of a service's server half: create, answer, defer, destroy.

use std::os::raw::c_void;

use hypha_node::HyphaCallbackResult;
use hypha_node::safe::{Answer, HyphaNode, HyphaRequest, HyphaServer, RequestView, ServeHandler};
use hypha_node::status::HyphaStatus;
use hypha_node::waitset::HyphaContextRelease;

use super::{
    Context, HYPHA_ANSWER_REFUSE, HYPHA_ANSWER_REPLY, HyphaRequestView, HyphaServeCallback,
    HyphaServiceDesc, declaration,
};
use crate::boundary::{borrow_bytes, handle_mut, read_in, take_box, write_out};
use crate::status::try_guard;

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
