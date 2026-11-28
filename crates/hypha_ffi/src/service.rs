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
use hypha_node::safe::{HyphaRequest, ServiceDecl};
use hypha_node::status::HyphaStatus;
use hypha_node::waitset::HyphaContextRelease;

use crate::boundary::borrow_str;

mod client;
mod server;
pub use client::{
    hypha_client_call, hypha_client_status, hypha_client_wait_for_server,
    hypha_service_client_create, hypha_service_client_destroy,
};
pub use server::{
    hypha_request_answer, hypha_request_defer, hypha_request_destroy, hypha_service_server_create,
    hypha_service_server_destroy,
};

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

/// The declaration `desc` describes, borrowing its texts.
///
/// # Safety
/// Each text of `desc` points to its length of readable bytes.
unsafe fn declaration(desc: &HyphaServiceDesc) -> Result<ServiceDecl<'_>, HyphaStatus> {
    let text = |pointer: *const c_char, len: usize| unsafe { borrow_str(pointer, len) };
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
        policy,
        timeout: (desc.timeout_nanos != 0).then(|| Duration::from_nanos(desc.timeout_nanos)),
    })
}
