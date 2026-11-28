//! The C ABI of a service's client half: create, call, status, wait, destroy.

use std::os::raw::c_void;
use std::time::Duration;

use hypha_node::HyphaCallbackResult;
use hypha_node::safe::{CallDone, CallResult, HyphaClient, HyphaNode};
use hypha_node::status::HyphaStatus;
use hypha_node::waitset::HyphaContextRelease;

use super::{
    Context, HyphaCallDoneCallback, HyphaCallResult, HyphaClientStatus, HyphaServiceDesc,
    declaration,
};
use crate::boundary::{borrow_bytes, handle_ref, read_in, take_box, write_out};
use crate::status::try_guard;

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
pub(super) fn deliver_result(
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
