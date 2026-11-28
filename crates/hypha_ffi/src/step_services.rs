//! The C ABI of a step test's services (`hypha::testing::StepHarness`), wrappers of
//! `hypha_node::service::HyphaStepServices`. The servers and clients of a cycle that was not
//! started have the test as their peer: it delivers requests and gives calls their outcomes,
//! and a step runs on the emission `hypha_step_services_step` hands it, through the service
//! functions of any cycle step (`hypha_cycle_requests`, `hypha_cycle_call`, ...). What a step
//! reads, and what becomes of unanswered requests, a failed step's requests and calls, and
//! unread outcomes, is decided below this layer, as for a started cycle.

use std::os::raw::{c_char, c_void};

use hypha_node::safe::{
    CallOutcome, FedAnswer, HyphaConfig, HyphaStepServices, RequestState, step_services_create,
};
use hypha_node::status::HyphaStatus;
use hypha_node::{HyphaCycleEmission, safe::EmissionView};

use crate::boundary::{
    borrow_bytes, borrow_str, handle_mut, handle_ref, read_in, take_box, write_out,
};
use crate::service::{HyphaServiceDesc, declaration};
use crate::status::try_guard;

/// A delivered request no step has read yet.
pub const HYPHA_STEP_REQUEST_QUEUED: u32 = 0;
/// A step read the request and deferred it; it has no answer yet.
pub const HYPHA_STEP_REQUEST_DEFERRED: u32 = 1;
/// Answered with a response: the bytes are the encoded response.
pub const HYPHA_STEP_REQUEST_REPLIED: u32 = 2;
/// Refused (`refuse()`, left unanswered by the step that read it, a response above the
/// bound): the bytes are the reason.
pub const HYPHA_STEP_REQUEST_REFUSED: u32 = 3;
/// The step that held it failed; its client reads `ServerLost`: the bytes are the reason.
pub const HYPHA_STEP_REQUEST_FAILED: u32 = 4;

/// What became of a delivered request, borrowed for the callback.
#[repr(C)]
pub struct HyphaStepRequestState {
    /// One of the `HYPHA_STEP_REQUEST_*` states.
    pub state: u32,
    /// The response or the reason, as `state` says; null and 0 for none.
    pub bytes: *const u8,
    /// Bytes of `bytes`.
    pub bytes_len: usize,
    /// The framework time the answer carries: that of the step it left after, or of the
    /// answer of a deferred request given outside a step. 0 without an answer.
    pub answered_nanos: i64,
}

/// One step of a test: run it with `emission` and `context`; return 1 when it succeeded,
/// 0 when it failed (it threw, or latched a failure).
pub type HyphaStepServicesStep =
    Option<extern "C" fn(emission: *mut HyphaCycleEmission, context: *mut c_void) -> u8>;

/// Receives what became of one request, with the caller's context.
pub type HyphaStepRequestStateCallback =
    Option<extern "C" fn(state: *const HyphaStepRequestState, context: *mut c_void)>;

/// Receives one call a client sent: its encoded request, `len` bytes, and its stamp.
pub type HyphaStepSentCallback =
    Option<extern "C" fn(request: *const u8, len: usize, sent_nanos: i64, context: *mut c_void)>;

/// Create the services of the cycle named `cycle_name` (`cycle_name_len` bytes of UTF-8)
/// for a step test, with no node and no bus. `config` is null or a live configuration that
/// holds the ports' tables (`[ports.servers.<port>]`, `[ports.clients.<port>]`), read when
/// a port is bound as a start reads them; a port without a table keeps the defaults.
///
/// `*out` (null on a refusal) is owned by the caller, released with
/// `hypha_step_services_destroy`. `InvalidArgument`: null `out`, a name that is not UTF-8.
///
/// # Safety
/// `config` is null or live for the call. `cycle_name` holds `cycle_name_len` bytes.
/// `out` is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_step_services_create(
    config: *const HyphaConfig,
    cycle_name: *const c_char,
    cycle_name_len: usize,
    out: *mut *mut HyphaStepServices,
) -> HyphaStatus {
    try_guard(|| {
        unsafe { write_out(out, std::ptr::null_mut()) }?;
        let config = if config.is_null() { None } else { Some(unsafe { handle_ref(config) }?) };
        let cycle = unsafe { borrow_str(cycle_name, cycle_name_len) }?;
        let services = Box::new(step_services_create(config, cycle));
        unsafe { write_out(out, Box::into_raw(services)) }
    })
}

/// Destroy `services`: each request it holds ends, and a deferred one answered later is
/// refused with `Disconnected`. Null is accepted.
///
/// # Safety
/// `services` is null or a live handle that no thread uses during the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_step_services_destroy(
    services: *mut HyphaStepServices,
) -> HyphaStatus {
    try_guard(|| {
        drop(unsafe { take_box(services) });
        Ok(())
    })
}

/// Bind the server (`client` 0) or the client (`client` 1) that `desc` describes, as
/// `hypha_cycle_bind_server` and `hypha_cycle_bind_client` bind one to a cycle:
/// `out_index` receives its index among the servers or the clients, from zero, which the
/// step's service functions take. Its table may set `per_step`, or `timeout_ms` and
/// `outstanding`, refused as a start refuses them.
///
/// `InvalidArgument`: a null argument, a malformed declaration or table.
///
/// # Safety
/// `services` is live; `desc` is readable with its texts; `out_index` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_step_services_bind(
    services: *mut HyphaStepServices,
    desc: *const HyphaServiceDesc,
    client: u8,
    out_index: *mut u32,
) -> HyphaStatus {
    try_guard(|| {
        unsafe { write_out(out_index, 0) }?;
        let services = unsafe { handle_mut(services) }?;
        let desc = unsafe { read_in(desc) }?;
        let decl = unsafe { declaration(&desc) }?;
        let index =
            if client == 0 { services.bind_server(&decl)? } else { services.bind_client(&decl)? };
        unsafe { write_out(out_index, index) }
    })
}

/// Deliver the encoded `request` (`len` bytes) from the client node `client_node` to server
/// `server`, sent at `sent_nanos` when `stamped` is 1, or just before the next step when it
/// is 0. A step at T reads the requests sent before T, in step order, at most its
/// `per_step`. `out_id` receives the request's id, for `hypha_step_services_request`.
///
/// `InvalidArgument`: a null argument, an unknown server, a name that is not UTF-8.
///
/// # Safety
/// `services` is live; `request` holds `len` bytes (null when `len` is 0); `client_node`
/// holds `client_node_len` bytes; `out_id` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_step_services_deliver(
    services: *mut HyphaStepServices,
    server: u32,
    request: *const u8,
    len: usize,
    client_node: *const c_char,
    client_node_len: usize,
    sent_nanos: i64,
    stamped: u8,
    out_id: *mut u64,
) -> HyphaStatus {
    try_guard(|| {
        unsafe { write_out(out_id, u64::MAX) }?;
        let services = unsafe { handle_mut(services) }?;
        let request = if len == 0 { &[][..] } else { unsafe { borrow_bytes(request, len) }? };
        let client = unsafe { borrow_str(client_node, client_node_len) }?;
        let stamp = (stamped != 0).then_some(sent_nanos);
        let id = services.deliver(server, request, client, stamp)?;
        unsafe { write_out(out_id, id) }
    })
}

/// End the oldest call in flight of client `client` with `outcome` (`HYPHA_CALL_ANSWERED`
/// ... `HYPHA_CALL_TIMEOUT`): `bytes` is the encoded response of an answered call, else the
/// reason. Its answer is stamped `sent_nanos` when `stamped` is 1, read by the next step
/// whose time is after it; with `stamped` 0 the next step reads it.
///
/// `InvalidArgument`: a null argument, an unknown client or outcome, a reason that is not
/// UTF-8. `InvalidState`: the client has no call in flight.
///
/// # Safety
/// `services` is live; `bytes` holds `len` bytes (null when `len` is 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_step_services_feed(
    services: *mut HyphaStepServices,
    client: u32,
    outcome: u32,
    bytes: *const u8,
    len: usize,
    sent_nanos: i64,
    stamped: u8,
) -> HyphaStatus {
    try_guard(|| {
        let services = unsafe { handle_mut(services) }?;
        let bytes = if len == 0 { &[][..] } else { unsafe { borrow_bytes(bytes, len) }? };
        let reason = || {
            std::str::from_utf8(bytes).map(str::to_owned).map_err(|_| HyphaStatus::InvalidArgument)
        };
        let outcome = match outcome {
            crate::HYPHA_CALL_ANSWERED => CallOutcome::Answered(bytes.to_vec()),
            crate::HYPHA_CALL_REFUSED => CallOutcome::Refused(reason()?),
            crate::HYPHA_CALL_NO_SERVER => CallOutcome::NoServer(reason()?),
            crate::HYPHA_CALL_NOT_SENT => CallOutcome::NotSent(reason()?),
            crate::HYPHA_CALL_SERVER_LOST => CallOutcome::ServerLost(reason()?),
            crate::HYPHA_CALL_TIMEOUT => CallOutcome::Timeout(reason()?),
            _ => return Err(HyphaStatus::InvalidArgument),
        };
        let stamp = (stamped != 0).then_some(sent_nanos);
        services.feed(client, outcome, stamp)?;
        Ok(())
    })
}

/// Run one step at framework time `now_nanos`: `step` runs once with the step's emission,
/// which the service functions of a cycle step take, and `context`. Before it, the step's
/// requests and outcomes are collected as a started cycle on simulated time collects them;
/// after it, a successful step's answers and calls leave (the calls for
/// `hypha_step_services_calls`), and a failed step answers what it read as failed and ends
/// its calls `NotSent`. The emission takes no output message.
///
/// `InvalidArgument`: a null argument.
///
/// # Safety
/// `services` is live and used by no other call during the step.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_step_services_step(
    services: *mut HyphaStepServices,
    now_nanos: i64,
    step: HyphaStepServicesStep,
    context: *mut c_void,
) -> HyphaStatus {
    try_guard(|| {
        let services = unsafe { handle_mut(services) }?;
        let function = step.ok_or(HyphaStatus::InvalidArgument)?;
        services.step(now_nanos, |view: &mut EmissionView<'_, '_>| {
            function(std::ptr::from_mut(view).cast(), context) != 0
        });
        Ok(())
    })
}

/// Read what became of request `id` of server `server`: `done` runs once with it and
/// `context`, its bytes borrowed for the call.
///
/// `InvalidArgument`: a null argument, an unknown server or request.
///
/// # Safety
/// `services` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_step_services_request(
    services: *const HyphaStepServices,
    server: u32,
    id: u64,
    done: HyphaStepRequestStateCallback,
    context: *mut c_void,
) -> HyphaStatus {
    try_guard(|| {
        let services = unsafe { handle_ref(services) }?;
        let function = done.ok_or(HyphaStatus::InvalidArgument)?;
        let state = services.request_state(server, id)?;
        let (code, bytes, stamp): (u32, &[u8], i64) = match &state {
            RequestState::Queued => (HYPHA_STEP_REQUEST_QUEUED, &[], 0),
            RequestState::Deferred => (HYPHA_STEP_REQUEST_DEFERRED, &[], 0),
            RequestState::Answered(FedAnswer::Replied(response), at) => {
                (HYPHA_STEP_REQUEST_REPLIED, response, *at)
            }
            RequestState::Answered(FedAnswer::Refused(reason), at) => {
                (HYPHA_STEP_REQUEST_REFUSED, reason.as_bytes(), *at)
            }
            RequestState::Answered(FedAnswer::Failed(reason), at) => {
                (HYPHA_STEP_REQUEST_FAILED, reason.as_bytes(), *at)
            }
        };
        let view = HyphaStepRequestState {
            state: code,
            bytes: if bytes.is_empty() { std::ptr::null() } else { bytes.as_ptr() },
            bytes_len: bytes.len(),
            answered_nanos: stamp,
        };
        function(&raw const view, context);
        Ok(())
    })
}

/// Read the calls client `client` sent after the last step, in call order: `each` runs
/// once per call with its encoded request, borrowed for the call, its stamp (the step's
/// time) and `context`. A failed step sent none.
///
/// `InvalidArgument`: a null argument or an unknown client.
///
/// # Safety
/// `services` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_step_services_calls(
    services: *const HyphaStepServices,
    client: u32,
    each: HyphaStepSentCallback,
    context: *mut c_void,
) -> HyphaStatus {
    try_guard(|| {
        let services = unsafe { handle_ref(services) }?;
        let function = each.ok_or(HyphaStatus::InvalidArgument)?;
        for call in services.sent(client)? {
            function(call.request.as_ptr(), call.request.len(), call.stamp, context);
        }
        Ok(())
    })
}
