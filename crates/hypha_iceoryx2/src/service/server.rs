//! The server of a service: the one port that answers a name's calls.
//!
//! It owns every request it received in a fixed table of slots, so a request answered
//! later (deferred) is a handle into the table, never the iceoryx2 `ActiveRequest`
//! itself: dropping the server ends every slot at once, and its clients see the
//! disconnect (`docs/design/native-services.md` §2).

use hypha_core::port::{PortError, ReplyError, ServicePolicy};
use hypha_wire::frame::{FLAG_FAILED, FLAG_REFUSED, FrameHeader, FrameMetadata};
use iceoryx2::port::listener::Listener;
use iceoryx2::port::notifier::Notifier;
use iceoryx2::prelude::PortFactory as _;

use super::key::StreamKey;
use super::open::StreamFactory;
use crate::header::Iox2FrameHeader;
use crate::transport::{Ipc, SharedNode};

type RrServer = iceoryx2::port::server::Server<Ipc, [u8], Iox2FrameHeader, [u8], Iox2FrameHeader>;
type RrActive =
    iceoryx2::active_request::ActiveRequest<Ipc, [u8], Iox2FrameHeader, [u8], Iox2FrameHeader>;

/// A request the server holds: the index of its slot and the slot's generation, so a
/// handle to an answered request never reaches the request that took its slot next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RequestHandle {
    index: u32,
    generation: u32,
}

/// What [`Iox2Server::receive`] took: the handle and the request's frame metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReceivedRequest {
    /// The slot that holds the request until it is answered.
    pub handle: RequestHandle,
    /// The client's sender id.
    pub client: u64,
    /// The client's sequence of this request.
    pub sequence: u64,
    /// The client's framework time at the send.
    pub framework_sent_nanos: u64,
    /// The host monotonic time at the send.
    pub transport_sent_nanos: u64,
}

/// What a server counted. Each counter wraps.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ServerCounters {
    /// Requests received and verified.
    pub received: u64,
    /// Requests answered with a response.
    pub answered: u64,
    /// Requests refused.
    pub refused: u64,
    /// Requests answered as failed: a handler that threw, a step that failed.
    pub failed: u64,
    /// Requests whose client left before the answer.
    pub abandoned: u64,
    /// Request frames that did not verify, dropped unanswered.
    pub invalid_requests: u64,
}

struct Slot {
    request: RrActive,
    generation: u32,
    client: u64,
    sequence: u64,
}

/// The one server of a service name. It belongs to one thread at a time.
pub struct Iox2Server {
    key: StreamKey,
    policy: ServicePolicy,
    node: SharedNode,
    slots: Vec<Option<Slot>>,
    next_generation: u32,
    server_id: u64,
    counters: ServerCounters,
    server: RrServer,
    response_notifier: Notifier<Ipc>,
    service: StreamFactory,
    /// The name lock; declared last so it drops last, and no second server starts while
    /// this one stands.
    _lock: Notifier<Ipc>,
}

/// The kind of an answer, as its frame flags carry it.
#[derive(Clone, Copy)]
enum Answer {
    Response,
    Refusal,
    Failure,
}

impl Answer {
    const fn flags(self) -> u32 {
        match self {
            Self::Response => 0,
            Self::Refusal => FLAG_REFUSED,
            Self::Failure => FLAG_FAILED,
        }
    }
}

impl Iox2Server {
    #[expect(clippy::too_many_arguments, reason = "the parts that only server creation assembles")]
    pub(crate) fn new(
        key: StreamKey,
        policy: ServicePolicy,
        node: SharedNode,
        server: RrServer,
        response_notifier: Notifier<Ipc>,
        service: StreamFactory,
        lock: Notifier<Ipc>,
        server_id: u64,
    ) -> Self {
        // iceoryx2 lets a client fill a server again with requests the server still holds
        // after the client dropped them: twice the outstanding calls of each client.
        let capacity =
            policy.admitted_clients().saturating_mul(policy.max_outstanding).saturating_mul(2);
        let mut slots = Vec::with_capacity(capacity);
        slots.resize_with(capacity, || None);
        Self {
            key,
            policy,
            node,
            slots,
            next_generation: 0,
            server_id,
            counters: ServerCounters::default(),
            server,
            response_notifier,
            service,
            _lock: lock,
        }
    }

    /// The public name of the service.
    #[must_use]
    pub fn name(&self) -> &str {
        self.key.public()
    }

    /// The service's policy.
    #[must_use]
    pub const fn policy(&self) -> &ServicePolicy {
        &self.policy
    }

    /// What the server counted.
    #[must_use]
    pub const fn counters(&self) -> ServerCounters {
        self.counters
    }

    /// Requests held now, deferred ones included.
    #[must_use]
    pub fn held(&self) -> usize {
        self.slots.iter().filter(|slot| slot.is_some()).count()
    }

    /// The number of slots: every request the clients can make the server hold.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// The clients of the stream now, observers included.
    #[must_use]
    pub fn clients(&self) -> usize {
        self.service.dynamic_config().number_of_clients()
    }

    /// Open a listener on the event that clients notify after each request, for a
    /// `WaitSet`. The server holds none itself.
    ///
    /// # Errors
    /// [`PortError::Backend`] when the event service cannot be opened.
    pub fn open_listener(&self) -> Result<Listener<Ipc>, PortError> {
        crate::notification::create_listener(
            &self.node,
            &self.key.request_event(),
            Some(super::open::request_event_limits(&self.policy)),
        )
    }

    /// Take the next verified request, or `None` when none waits. A frame that does not
    /// verify is dropped unanswered and counted.
    ///
    /// # Errors
    /// [`PortError::Backend`] when iceoryx2 cannot receive.
    pub fn receive(&mut self) -> Result<Option<ReceivedRequest>, PortError> {
        loop {
            let Some(request) = self.server.receive().map_err(|cause| {
                log::error!(target: "hypha/transport",
                    "hypha_iceoryx2: service '{}': receive failed: {cause:?}", self.key.public());
                PortError::Backend(2)
            })?
            else {
                return Ok(None);
            };
            let header = FrameHeader::from(*request.user_header());
            let verified =
                hypha_wire::frame::verify(self.key.request_code(), &header, request.payload())
                    .is_ok()
                    && header.metadata.schema_hash == self.key.hash();
            if !verified {
                self.counters.invalid_requests = self.counters.invalid_requests.wrapping_add(1);
                continue;
            }
            let Some(index) = self.slots.iter().position(Option::is_none) else {
                // Unreachable while clients keep their limits: the table holds what iceoryx2
                // lets them send. A request that finds no slot is dropped and counted.
                self.counters.invalid_requests = self.counters.invalid_requests.wrapping_add(1);
                continue;
            };
            let generation = self.next_generation;
            self.next_generation = self.next_generation.wrapping_add(1);
            let m = header.metadata;
            let received = ReceivedRequest {
                handle: RequestHandle {
                    index: u32::try_from(index).map_err(|_| PortError::ResourceLimit)?,
                    generation,
                },
                client: m.publisher_id,
                sequence: m.sequence,
                framework_sent_nanos: m.framework_sent_nanos,
                transport_sent_nanos: m.transport_sent_nanos,
            };
            if let Some(slot) = self.slots.get_mut(index) {
                *slot = Some(Slot {
                    request,
                    generation,
                    client: m.publisher_id,
                    sequence: m.sequence,
                });
            }
            self.counters.received = self.counters.received.wrapping_add(1);
            return Ok(Some(received));
        }
    }

    /// The payload of a held request.
    #[must_use]
    pub fn payload(&self, handle: RequestHandle) -> Option<&[u8]> {
        self.slot(handle).map(|slot| slot.request.payload())
    }

    /// Answer `handle` with `response`, stamped `framework_nanos`.
    ///
    /// # Errors
    /// [`ReplyError::TooLarge`] for a response above the bound: the client is refused with
    /// the sizes instead. [`ReplyError::Disconnected`] when the client is gone,
    /// [`ReplyError::UnknownRequest`] for a handle the server no longer holds.
    pub fn reply(
        &mut self,
        handle: RequestHandle,
        response: &[u8],
        framework_nanos: u64,
    ) -> Result<(), ReplyError> {
        let limit = self.policy.max_response_bytes;
        if response.len() > limit {
            let reason = format!(
                "response {} B above bound {limit} B, policy {}",
                response.len(),
                self.policy.name
            );
            self.answer(handle, Answer::Refusal, reason.as_bytes(), framework_nanos)?;
            return Err(ReplyError::TooLarge { requested: response.len(), limit });
        }
        self.answer(handle, Answer::Response, response, framework_nanos)
    }

    /// Refuse `handle`; the client's outcome is `Refused(reason)`.
    ///
    /// # Errors
    /// As [`Self::reply`], without `TooLarge`: a reason above the bound is shortened.
    pub fn refuse(
        &mut self,
        handle: RequestHandle,
        reason: &str,
        framework_nanos: u64,
    ) -> Result<(), ReplyError> {
        let reason = self.bounded_reason(reason);
        self.answer(handle, Answer::Refusal, reason.as_bytes(), framework_nanos)
    }

    /// Answer `handle` as failed (a handler that threw, a step that failed); the client's
    /// outcome is `ServerLost(reason)`.
    ///
    /// # Errors
    /// As [`Self::refuse`].
    pub fn fail(
        &mut self,
        handle: RequestHandle,
        reason: &str,
        framework_nanos: u64,
    ) -> Result<(), ReplyError> {
        let reason = self.bounded_reason(reason);
        self.answer(handle, Answer::Failure, reason.as_bytes(), framework_nanos)
    }

    /// Drop every held request whose client has left, and count them. Call it each turn.
    pub fn drop_abandoned(&mut self) -> usize {
        let mut dropped: usize = 0;
        for slot in &mut self.slots {
            let gone = slot.as_ref().is_some_and(|held| !held.request.is_connected());
            if gone {
                *slot = None;
                dropped = dropped.saturating_add(1);
            }
        }
        self.counters.abandoned =
            self.counters.abandoned.wrapping_add(u64::try_from(dropped).unwrap_or(u64::MAX));
        dropped
    }

    fn slot(&self, handle: RequestHandle) -> Option<&Slot> {
        let index = usize::try_from(handle.index).ok()?;
        self.slots.get(index)?.as_ref().filter(|slot| slot.generation == handle.generation)
    }

    fn bounded_reason<'a>(&self, reason: &'a str) -> &'a str {
        let limit = self.policy.max_response_bytes;
        if reason.len() <= limit {
            return reason;
        }
        let end = (0..=limit).rev().find(|end| reason.is_char_boundary(*end)).unwrap_or(0);
        reason.get(..end).unwrap_or_default()
    }

    /// Send one answer and free its slot, whatever the outcome of the send.
    fn answer(
        &mut self,
        handle: RequestHandle,
        kind: Answer,
        payload: &[u8],
        framework_nanos: u64,
    ) -> Result<(), ReplyError> {
        if self.slot(handle).is_none() {
            return Err(ReplyError::UnknownRequest);
        }
        let index = usize::try_from(handle.index).map_err(|_| ReplyError::UnknownRequest)?;
        let slot =
            self.slots.get_mut(index).and_then(Option::take).ok_or(ReplyError::UnknownRequest)?;
        if !slot.request.is_connected() {
            self.counters.abandoned = self.counters.abandoned.wrapping_add(1);
            return Err(ReplyError::Disconnected);
        }
        let now = u64::try_from(hypha_time::steady_now().nanos()).unwrap_or(0);
        let metadata = FrameMetadata {
            schema_hash: self.key.hash(),
            publisher_id: self.server_id,
            // The request's own sequence: a counter of the server's would show a gap to each client.
            sequence: slot.sequence,
            transport_sent_nanos: now,
            framework_sent_nanos: framework_nanos,
        };
        let code = self.key.response_code(slot.client, slot.sequence);
        let header = hypha_wire::frame::seal(code, metadata, kind.flags(), payload);
        let mut loan = slot.request.loan_slice_uninit(payload.len()).map_err(|cause| {
            log::error!(target: "hypha/transport",
                "hypha_iceoryx2: service '{}': cannot loan a response: {cause:?}", self.key.public());
            ReplyError::Port(PortError::PoolExhausted)
        })?;
        *loan.user_header_mut() = Iox2FrameHeader::from(header);
        loan.write_from_slice(payload).send().map_err(|cause| {
            log::error!(target: "hypha/transport",
                "hypha_iceoryx2: service '{}': cannot send a response: {cause:?}", self.key.public());
            ReplyError::Port(PortError::Backend(40))
        })?;
        drop(slot);
        let counter = match kind {
            Answer::Response => &mut self.counters.answered,
            Answer::Refusal => &mut self.counters.refused,
            Answer::Failure => &mut self.counters.failed,
        };
        *counter = counter.wrapping_add(1);
        crate::notification::notified(&self.response_notifier.notify());
        Ok(())
    }
}

impl Drop for Iox2Server {
    fn drop(&mut self) {
        // Every slot ends with the server: the clients' calls disconnect, and the wake
        // makes them see it at their next turn.
        self.slots.clear();
        crate::notification::notified(&self.response_notifier.notify());
    }
}
