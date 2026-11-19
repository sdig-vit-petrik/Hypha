//! The service policy an endpoint passes through the C ABI: `HyphaQos`.

use crate::status::Error;
use std::os::raw::c_char;

use hypha_core::port::{PayloadCapacity, QosPolicy, QosRequest};

/// The service policy an endpoint declares, and the payload bound of a publisher.
/// The struct holds no allocation and needs no destruction. The name is UTF-8, not
/// NUL-terminated, borrowed for the call that receives it and copied before that call
/// returns. A refused limit or name is `HYPHA_STATUS_INVALID_ARGUMENT`. A policy declared
/// on a framework topic (`/clock`, `/clock_progress`) is checked for validity only.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct HyphaQos {
    /// The policy name bytes, or null when `name_len` is zero. The name is 1 to 32
    /// letters, digits, `_`, `-` or `.`, starting with a letter or digit.
    ///
    /// `name_len == 0` declares no policy; the endpoint adopts the topic's service. A
    /// publisher creates a missing service with hypha's `default` policy. A subscriber
    /// gets `HYPHA_STATUS_NOT_AVAILABLE` until a publisher has opened a non-framework topic.
    ///
    /// `name_len > 0`: the service must carry the same name and limits, or the endpoint is
    /// refused with `HYPHA_STATUS_QOS_MISMATCH`. A missing service is created with them.
    pub name: *const c_char,
    /// The byte count of `name`. Zero declares no policy.
    pub name_len: usize,
    /// Publishers the policy admits: 1 to 256 (`default`: 8; a framework topic: 32).
    /// Ignored when `name_len` is zero.
    pub max_publishers: u32,
    /// Subscribers the policy admits: 1 to 256 (`default`: 16; a framework topic: 32).
    /// Ignored when `name_len` is zero.
    pub max_subscribers: u32,
    /// The deepest queue a subscriber of the policy may ask for: 1 to 1024 (`default`
    /// and a framework topic: 32). Ignored when `name_len` is zero.
    pub max_depth: u32,
    /// The payload bound of a publisher, in bytes, for a type whose size varies. Zero
    /// means growable. A fixed-size type is bounded to its exact encoded size, and a
    /// larger loan or publish than the bound is `HYPHA_STATUS_TOO_LARGE`. A subscriber
    /// ignores it.
    pub max_payload_dynamic: u64,
    /// The first slot size, in bytes, of a growable publisher: a hint, not a bound. Zero
    /// takes the framework default (4096). A value that does not fit `size_t` is
    /// `HYPHA_STATUS_INVALID_ARGUMENT`.
    pub initial_payload: u64,
}

impl Default for HyphaQos {
    /// An endpoint that declares no policy and has no payload bound.
    fn default() -> Self {
        Self {
            name: std::ptr::null(),
            name_len: 0,
            max_publishers: 0,
            max_subscribers: 0,
            max_depth: 0,
            max_payload_dynamic: 0,
            initial_payload: 0,
        }
    }
}

/// What an endpoint asks of the service and of its payload, as the transport takes it.
pub struct Declaration {
    /// The service policy the endpoint declares, or none.
    pub request: QosRequest,
    /// The payload bound of a publisher. Unused by a subscriber.
    pub payload: PayloadCapacity,
    /// The first slot size of a growable publisher; zero is the default.
    pub initial_payload: usize,
}

impl Declaration {
    /// An endpoint that declares no policy, with the payload bound `payload`.
    #[must_use]
    pub fn undeclared(payload: PayloadCapacity) -> Self {
        Self { request: QosRequest::Undeclared, payload, initial_payload: 0 }
    }
}

/// Reads the declaration of an endpoint from its `qos`.
///
/// # Errors
/// `InvalidArgument` for a name that is null, not UTF-8, or not a policy name, and for a
/// limit or payload bound that does not fit `usize`. `name` is not read when
/// `qos.name_len` is zero.
pub fn read_declaration(qos: &HyphaQos, name: &str) -> Result<Declaration, Error> {
    let payload = payload_capacity(qos.max_payload_dynamic)?;
    let initial_payload =
        usize::try_from(qos.initial_payload).map_err(|_| Error::InvalidArgument)?;
    let declares_nothing = qos.name_len == 0;
    if declares_nothing {
        return Ok(Declaration { request: QosRequest::Undeclared, payload, initial_payload });
    }
    let max_publishers = limit(qos.max_publishers)?;
    let max_subscribers = limit(qos.max_subscribers)?;
    let max_depth = limit(qos.max_depth)?;
    let policy = QosPolicy::new(name, max_publishers, max_subscribers, max_depth);
    Ok(Declaration { request: QosRequest::Declared(policy), payload, initial_payload })
}

/// The payload bound of `max_payload_dynamic`: zero is growable.
fn payload_capacity(max_payload_dynamic: u64) -> Result<PayloadCapacity, Error> {
    if max_payload_dynamic == 0 {
        return Ok(PayloadCapacity::Growable);
    }
    usize::try_from(max_payload_dynamic)
        .map(PayloadCapacity::Bounded)
        .map_err(|_| Error::InvalidArgument)
}

/// A limit as a count. The transport checks its bounds.
fn limit(value: u32) -> Result<usize, Error> {
    usize::try_from(value).map_err(|_| Error::InvalidArgument)
}
