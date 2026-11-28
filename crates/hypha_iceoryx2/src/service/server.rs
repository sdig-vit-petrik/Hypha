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
use iceoryx2::prelude::{CallbackProgression, PortFactory as _};

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

/// What [`Iox2Server::receive`] took: the handle, the request's frame metadata and who sent it.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// The client's node name; empty when it cannot be read.
    pub client_node: String,
    /// The client's port label (its node name unless it gave one).
    pub client_port: String,
    /// The client's node id; 0 when it cannot be read.
    pub client_node_id: u128,
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
    /// Requests refused as invalid: a frame that did not verify, or above the bound.
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
    /// Who each client is, as requests name it: its id, node name, port label and node id.
    identities: Vec<(iceoryx2::identifiers::UniqueClientId, String, String, u128)>,
    server: RrServer,
    response_notifier: Notifier<Ipc>,
    service: StreamFactory,
    /// The records of the request and response definitions, removed before the name lock
    /// goes, so the next server of the name never finds this one's records removed later.
    schema: Option<crate::schema::ServiceSchemaRelease>,
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

/// The refusal a client reads for a response of `len` bytes above the bound of `policy`.
#[must_use]
pub fn response_too_large(len: usize, policy: &ServicePolicy) -> String {
    format!("response {len} B above bound {} B, policy {}", policy.max_response_bytes, policy.name)
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
        // iceoryx2 caps the requests a server holds for one client at that client's limit
        // of calls in flight, so this many slots hold every request it can be made to hold.
        let capacity = policy.admitted_clients().saturating_mul(policy.max_outstanding);
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
            identities: Vec::new(),
            server,
            response_notifier,
            service,
            schema: None,
            _lock: lock,
        }
    }

    /// Keep the records of the stream's definitions while this server stands.
    pub(crate) fn hold_schema(&mut self, release: crate::schema::ServiceSchemaRelease) {
        self.schema = Some(release);
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

    /// Push the node id of each client of the stream now onto `ids`: the upstreams of a
    /// server in a replay (`docs/design/native-services.md` §4).
    pub fn client_node_ids(&self, ids: &mut Vec<u128>) {
        self.service.dynamic_config().list_clients(|details| {
            ids.push(details.node_id.value());
            CallbackProgression::Continue
        });
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

    /// Take the next verified request, or `None` when none waits.
    ///
    /// A request whose frame does not verify, or that is above the request bound, is
    /// refused at once with `invalid request: <check>` and counted: its client gets a
    /// refusal, never a disconnect that reads as the server leaving.
    ///
    /// # Errors
    /// [`PortError::BorrowLimit`] when iceoryx2 refuses another request after the slots of
    /// departed clients were dropped; [`PortError::Backend`] when it cannot receive.
    pub fn receive(&mut self) -> Result<Option<ReceivedRequest>, PortError> {
        let mut dropped_abandoned = false;
        loop {
            let request = match self.server.receive() {
                Ok(Some(request)) => request,
                Ok(None) => return Ok(None),
                Err(iceoryx2::port::ReceiveError::ExceedsMaxBorrows) if !dropped_abandoned => {
                    // iceoryx2 caps the requests held per client at its limit of calls in
                    // flight; a client that left still counts until its slots are dropped.
                    self.drop_abandoned();
                    dropped_abandoned = true;
                    continue;
                }
                Err(iceoryx2::port::ReceiveError::ExceedsMaxBorrows) => {
                    return Err(PortError::BorrowLimit);
                }
                Err(cause) => {
                    log::error!(target: "hypha/transport",
                        "hypha_iceoryx2: service '{}': receive failed: {cause:?}", self.key.public());
                    return Err(PortError::Backend(super::BACKEND_EXCHANGE));
                }
            };
            let header = FrameHeader::from(*request.user_header());
            let m = header.metadata;
            let refusal = self.invalidity(&header, request.payload());
            if let Some(reason) = refusal {
                self.counters.invalid_requests = self.counters.invalid_requests.wrapping_add(1);
                let reason = self.bounded_reason(&reason).to_owned();
                // Sealed with the ids the frame claims: if they are the client's, it reads the
                // refusal; if not, its own check refuses the answer. Either way it is told.
                let _ = self.send_answer(
                    &request,
                    m.publisher_id,
                    m.sequence,
                    Answer::Refusal,
                    reason.as_bytes(),
                    0,
                );
                continue;
            }
            let Some(index) = self.slots.iter().position(Option::is_none) else {
                // Unreachable: the table holds every request iceoryx2 lets the clients make
                // the server hold. Refused rather than dropped, should it ever be reached.
                debug_assert!(false, "the slot table of '{}' is full", self.key.public());
                let reason = self.bounded_reason("server holds its limit of requests").to_owned();
                let _ = self.send_answer(
                    &request,
                    m.publisher_id,
                    m.sequence,
                    Answer::Refusal,
                    reason.as_bytes(),
                    0,
                );
                continue;
            };
            let (client_node, client_port, client_node_id) = self.identify(request.origin());
            let generation = self.next_generation;
            self.next_generation = self.next_generation.wrapping_add(1);
            let received = ReceivedRequest {
                handle: RequestHandle {
                    index: u32::try_from(index).map_err(|_| PortError::ResourceLimit)?,
                    generation,
                },
                client: m.publisher_id,
                sequence: m.sequence,
                framework_sent_nanos: m.framework_sent_nanos,
                transport_sent_nanos: m.transport_sent_nanos,
                client_node,
                client_port,
                client_node_id,
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

    /// Why a received request cannot be served, or `None` when it can.
    fn invalidity(&self, header: &FrameHeader, payload: &[u8]) -> Option<String> {
        if hypha_wire::frame::verify(self.key.request_code(), header, payload).is_err() {
            return Some("invalid request: crc".to_owned());
        }
        if header.metadata.schema_hash != self.key.hash() {
            return Some("invalid request: schema".to_owned());
        }
        let limit = self.policy.max_request_bytes;
        (payload.len() > limit).then(|| {
            format!(
                "invalid request: {} B above bound {limit} B, policy {}",
                payload.len(),
                self.policy.name
            )
        })
    }

    /// The node name, the port label and the node id of the client `origin`, cached per
    /// client; empty names and id 0 when it cannot be read.
    fn identify(
        &mut self,
        origin: iceoryx2::identifiers::UniqueClientId,
    ) -> (String, String, u128) {
        if let Some((_, node, port, id)) =
            self.identities.iter().find(|(client, _, _, _)| *client == origin)
        {
            return (node.clone(), port.clone(), *id);
        }
        let mut found = None;
        self.service.dynamic_config().list_clients(|details| {
            if details.client_id == origin {
                found = Some((details.node_id, details.client_name.as_str().to_owned()));
                return CallbackProgression::Stop;
            }
            CallbackProgression::Continue
        });
        let Some((node_id, port)) = found else {
            return (String::new(), String::new(), 0);
        };
        let node = match super::open::peer_node(&self.service, node_id) {
            Ok(super::open::PeerNode::Alive { name, .. }) => name,
            _ => String::new(),
        };
        // Bounded by the clients the stream admits; departed clients are forgotten first.
        if self.identities.len() >= self.policy.admitted_clients() {
            self.identities.clear();
        }
        self.identities.push((origin, node.clone(), port.clone(), node_id.value()));
        (node, port, node_id.value())
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
            let reason = response_too_large(response.len(), &self.policy);
            // Shortened to the bound itself, so even a tiny bound carries the refusal.
            let reason = self.bounded_reason(&reason).to_owned();
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

    /// Answer `handle` as failed (a handler that threw, a step that failed); the client
    /// reads [`CallEvent::Failed`](super::CallEvent::Failed) with `reason`.
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
        self.send_answer(
            &slot.request,
            slot.client,
            slot.sequence,
            kind,
            payload,
            framework_nanos,
        )?;
        drop(slot);
        let counter = match kind {
            Answer::Response => &mut self.counters.answered,
            Answer::Refusal => &mut self.counters.refused,
            Answer::Failure => &mut self.counters.failed,
        };
        *counter = counter.wrapping_add(1);
        Ok(())
    }

    /// Seal and send one answer to `request`, call `sequence` of `client`, and wake the
    /// clients.
    fn send_answer(
        &self,
        request: &RrActive,
        client: u64,
        sequence: u64,
        kind: Answer,
        payload: &[u8],
        framework_nanos: u64,
    ) -> Result<(), ReplyError> {
        let now = u64::try_from(hypha_time::steady_now().nanos()).unwrap_or(0);
        let metadata = FrameMetadata {
            schema_hash: self.key.hash(),
            publisher_id: self.server_id,
            // The request's own sequence: a counter of the server's would show a gap to each client.
            sequence,
            transport_sent_nanos: now,
            framework_sent_nanos: framework_nanos,
        };
        let code = self.key.response_code(client, sequence);
        let header = hypha_wire::frame::seal(code, metadata, kind.flags(), payload);
        let mut loan = request.loan_slice_uninit(payload.len()).map_err(|cause| {
            log::error!(target: "hypha/transport",
                "hypha_iceoryx2: service '{}': cannot loan a response: {cause:?}", self.key.public());
            ReplyError::Port(PortError::Backend(super::BACKEND_EXCHANGE))
        })?;
        *loan.user_header_mut() = Iox2FrameHeader::from(header);
        loan.write_from_slice(payload).send().map_err(|cause| {
            log::error!(target: "hypha/transport",
                "hypha_iceoryx2: service '{}': cannot send a response: {cause:?}", self.key.public());
            ReplyError::Port(PortError::Backend(super::BACKEND_EXCHANGE))
        })?;
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
