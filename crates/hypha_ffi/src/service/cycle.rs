//! The C ABI of a service bound to a cycle: a `Requests` port and a `Calls` port.

use std::os::raw::c_void;

use hypha_node::HyphaCycleEmission;
use hypha_node::safe::EmissionView;
use hypha_node::status::HyphaStatus;

use super::client::deliver_result;
use super::{
    HyphaCallDoneCallback, HyphaRequestView, HyphaServeCallback, HyphaServiceDesc, declaration,
};
use crate::boundary::{borrow_bytes, handle_mut, read_in, write_out};
use crate::cycle::{CycleTarget, HyphaCycle};
use crate::status::try_guard;

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
