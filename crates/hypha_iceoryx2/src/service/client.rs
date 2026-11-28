//! The client of a service: it sends calls and collects what became of each.
//!
//! A call that times out is not dropped: the server may still hold it, and iceoryx2
//! sizes a client for only one generation of such requests. The client keeps it as
//! expired, so its limit counts it (`Busy`), and reports a response that still comes as
//! late.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use hypha_core::port::{CallSendError, PortError, ServicePolicy};
use hypha_wire::frame::{FLAG_FAILED, FLAG_REFUSED, FrameHeader, FrameMetadata};
use iceoryx2::identifiers::UniqueNodeId;
use iceoryx2::port::listener::Listener;
use iceoryx2::port::notifier::Notifier;
use iceoryx2::prelude::{CallbackProgression, PortFactory as _};

use super::key::StreamKey;
use super::open::StreamFactory;
use crate::header::Iox2FrameHeader;
use crate::transport::{Ipc, SharedNode};

type RrClient = iceoryx2::port::client::Client<Ipc, [u8], Iox2FrameHeader, [u8], Iox2FrameHeader>;
type RrPending =
    iceoryx2::pending_response::PendingResponse<Ipc, [u8], Iox2FrameHeader, [u8], Iox2FrameHeader>;

/// One call of one client: its request sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CallId(pub u64);

/// What became of a call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallEvent {
    /// The server answered with this payload.
    Answered(Vec<u8>),
    /// The server refused, for this reason.
    Refused(String),
    /// The server failed while it handled the call (a handler that threw, a step that
    /// failed), for this reason.
    Failed(String),
    /// A response came that did not verify: the named check refused it.
    Invalid(&'static str),
    /// The call disconnected without an answer: the server left or was reclaimed.
    Lost,
}

/// One finished call, as [`Iox2Client::poll`] reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallReport {
    /// The call.
    pub id: CallId,
    /// What became of it.
    pub event: CallEvent,
    /// True for a call that had already timed out ([`Iox2Client::expire`]).
    pub late: bool,
}

/// Whether the server of a stream is alive, as the probe found it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    /// The server's node is alive.
    Alive,
    /// The server's node is dead: its calls are lost.
    Dead,
    /// The stream has no server.
    NoServer,
    /// The server's node state could not be read (inaccessible, undefined, or the node
    /// list failed): neither alive nor dead is known.
    Unknown,
}

/// What a client counted. Each counter wraps.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClientCounters {
    /// Calls sent to a server.
    pub sent: u64,
    /// Sends that found no server.
    pub no_server: u64,
    /// Calls that ended with an answer, a refusal, a failure, an invalid response or a loss.
    pub finished: u64,
    /// Responses that came after their call timed out.
    pub late_responses: u64,
    /// Responses that did not verify.
    pub invalid_responses: u64,
}

struct Call {
    pending: RrPending,
    sequence: u64,
    expired: bool,
}

/// How often one stream's server may be probed, per process.
const PROBE_INTERVAL: Duration = Duration::from_secs(1);

/// One probe of a stream's server: when, which server node, and what it found.
#[derive(Clone, Copy)]
struct Probe {
    at: Instant,
    server: Option<UniqueNodeId>,
    found: Liveness,
}

/// The last probe of each stream in this process. A result is reused only within
/// [`PROBE_INTERVAL`] and while the stream's server is still the node it judged, so a new
/// server that took the name over is never judged by its predecessor's death.
static PROBES: LazyLock<Mutex<HashMap<String, Probe>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// A client of one stream. It belongs to one thread at a time.
pub struct Iox2Client {
    key: StreamKey,
    policy: ServicePolicy,
    outstanding: usize,
    node: SharedNode,
    client_id: u64,
    next_sequence: u64,
    calls: Vec<Option<Call>>,
    counters: ClientCounters,
    client: RrClient,
    request_notifier: Notifier<Ipc>,
    service: StreamFactory,
}

impl Iox2Client {
    #[expect(clippy::too_many_arguments, reason = "the parts that only client creation assembles")]
    pub(crate) fn new(
        key: StreamKey,
        policy: ServicePolicy,
        outstanding: usize,
        node: SharedNode,
        client: RrClient,
        request_notifier: Notifier<Ipc>,
        service: StreamFactory,
        client_id: u64,
    ) -> Self {
        let mut calls = Vec::with_capacity(outstanding);
        calls.resize_with(outstanding, || None);
        Self {
            key,
            policy,
            outstanding,
            node,
            client_id,
            next_sequence: 0,
            calls,
            counters: ClientCounters::default(),
            client,
            request_notifier,
            service,
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

    /// This client's sender id, as its requests carry it.
    #[must_use]
    pub const fn id(&self) -> u64 {
        self.client_id
    }

    /// What the client counted.
    #[must_use]
    pub const fn counters(&self) -> ClientCounters {
        self.counters
    }

    /// Calls in flight, expired ones included.
    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.calls.iter().filter(|call| call.is_some()).count()
    }

    /// Calls that timed out and are still held by the server.
    #[must_use]
    pub fn expired(&self) -> usize {
        self.calls.iter().flatten().filter(|call| call.expired).count()
    }

    /// True when the stream has a server now that no probe of this process found dead. It
    /// reads the stream's server count and the last probe's result, nothing more: a server
    /// that crashed and was not probed yet still counts.
    #[must_use]
    pub fn server_available(&self) -> bool {
        let Some(server) = self.server_node() else {
            return false;
        };
        let probes = crate::locked(&PROBES);
        !probes
            .get(&self.key.stream())
            .is_some_and(|probe| probe.server == Some(server) && probe.found == Liveness::Dead)
    }

    /// The node of the stream's server now.
    fn server_node(&self) -> Option<UniqueNodeId> {
        let mut server = None;
        self.service.dynamic_config().list_servers(|details| {
            server = Some(details.node_id);
            CallbackProgression::Stop
        });
        server
    }

    /// Open a listener on the event the server notifies after each answer, for a
    /// `WaitSet` or a blocking wait.
    ///
    /// # Errors
    /// [`PortError::Backend`] when the event service cannot be opened.
    pub fn open_listener(&self) -> Result<Listener<Ipc>, PortError> {
        crate::notification::create_listener(
            &self.node,
            &self.key.response_event(),
            Some(super::open::response_event_limits(&self.policy)),
        )
    }

    /// Send `request`, stamped `framework_nanos`, and wake the server. Returns at once,
    /// whatever the server does.
    ///
    /// # Errors
    /// [`CallSendError::TooLarge`] above the bound, [`CallSendError::Busy`] at the limit,
    /// [`CallSendError::NoServer`] when no server received it (nothing is pending),
    /// [`CallSendError::Port`] for a transport failure.
    pub fn send(&mut self, request: &[u8], framework_nanos: u64) -> Result<CallId, CallSendError> {
        let limit = self.policy.max_request_bytes;
        if request.len() > limit {
            return Err(CallSendError::TooLarge { requested: request.len(), limit });
        }
        let busy = CallSendError::Busy { outstanding: self.outstanding, expired: self.expired() };
        let Some(free) = self.calls.iter().position(Option::is_none) else {
            return Err(busy);
        };
        let sequence = self.next_sequence.wrapping_add(1);
        self.next_sequence = sequence;
        let now = u64::try_from(hypha_time::steady_now().nanos()).unwrap_or(0);
        let metadata = FrameMetadata {
            schema_hash: self.key.hash(),
            publisher_id: self.client_id,
            sequence,
            transport_sent_nanos: now,
            framework_sent_nanos: framework_nanos,
        };
        let header = hypha_wire::frame::seal(self.key.request_code(), metadata, 0, request);
        let mut loan = self.client.loan_slice_uninit(request.len()).map_err(|cause| {
            log::error!(target: "hypha/transport",
                "hypha_iceoryx2: service '{}': cannot loan a request: {cause:?}", self.key.public());
            CallSendError::Port(PortError::Backend(super::BACKEND_EXCHANGE))
        })?;
        *loan.user_header_mut() = Iox2FrameHeader::from(header);
        let pending = match loan.write_from_slice(request).send() {
            Ok(pending) => pending,
            Err(iceoryx2::port::client::RequestSendError::ExceedsMaxActiveRequests) => {
                return Err(busy);
            }
            Err(cause) => {
                log::error!(target: "hypha/transport",
                    "hypha_iceoryx2: service '{}': cannot send a request: {cause:?}", self.key.public());
                return Err(CallSendError::Port(PortError::Backend(super::BACKEND_EXCHANGE)));
            }
        };
        if pending.number_of_server_connections() == 0 {
            self.counters.no_server = self.counters.no_server.wrapping_add(1);
            return Err(CallSendError::NoServer);
        }
        if let Some(slot) = self.calls.get_mut(free) {
            *slot = Some(Call { pending, sequence, expired: false });
        }
        self.counters.sent = self.counters.sent.wrapping_add(1);
        crate::notification::notified(&self.request_notifier.notify());
        Ok(CallId(sequence))
    }

    /// Collect every call that finished since the last poll into `out`, and free their slots.
    pub fn poll(&mut self, out: &mut Vec<CallReport>) {
        for slot in &mut self.calls {
            let Some(call) = slot.as_ref() else {
                continue;
            };
            // Read the connection first: the server answers and then drops its request, so a
            // disconnect seen here means any response sent before it is visible below.
            let connected = call.pending.is_connected();
            let event = if call.pending.has_response() {
                match call.pending.receive() {
                    Ok(Some(response)) => Some(classify(
                        &self.key,
                        self.client_id,
                        call.sequence,
                        &FrameHeader::from(*response.user_header()),
                        response.payload(),
                    )),
                    Ok(None) => None,
                    Err(_) => Some(CallEvent::Invalid("receive")),
                }
            } else if connected {
                None
            } else {
                Some(CallEvent::Lost)
            };
            let Some(event) = event else {
                continue;
            };
            let late = call.expired;
            if matches!(event, CallEvent::Invalid(_)) {
                self.counters.invalid_responses = self.counters.invalid_responses.wrapping_add(1);
            }
            if late {
                self.counters.late_responses = self.counters.late_responses.wrapping_add(1);
            }
            self.counters.finished = self.counters.finished.wrapping_add(1);
            out.push(CallReport { id: CallId(call.sequence), event, late });
            *slot = None;
        }
    }

    /// Mark call `id` as timed out. It keeps its slot while the server holds it. True when
    /// it is still connected, so the caller probes the server ([`Self::probe_server`]).
    pub fn expire(&mut self, id: CallId) -> bool {
        self.calls.iter_mut().flatten().find(|call| call.sequence == id.0).is_some_and(|call| {
            call.expired = true;
            call.pending.is_connected()
        })
    }

    /// Drop call `id`, once its server was found dead.
    ///
    /// A call a live server still holds must not be dropped: iceoryx2 caps the requests a
    /// server holds per client at the client's limit, and the server's next receive would
    /// fail. So `abandon` checks that [`Self::probe_server`] found the stream's current
    /// server dead, or that the stream has no server.
    ///
    /// # Errors
    /// [`PortError::InvalidState`] when no probe found the current server dead; nothing is
    /// dropped.
    pub fn abandon(&mut self, id: CallId) -> Result<(), PortError> {
        let current = self.server_node();
        let dead = current.is_none_or(|server| {
            crate::locked(&PROBES)
                .get(&self.key.stream())
                .is_some_and(|probe| probe.server == Some(server) && probe.found == Liveness::Dead)
        });
        if !dead {
            return Err(PortError::InvalidState);
        }
        for slot in &mut self.calls {
            if slot.as_ref().is_some_and(|call| call.sequence == id.0) {
                *slot = None;
            }
        }
        Ok(())
    }

    /// Whether the stream's server is alive. Probes at most once a second per stream in
    /// this process, and gives the last result in between: it reads the node list of the
    /// service, with filesystem calls, on the calling thread.
    pub fn probe_server(&mut self) -> Liveness {
        let stream = self.key.stream();
        let server = self.server_node();
        {
            let probes = crate::locked(&PROBES);
            if let Some(probe) = probes.get(&stream)
                && probe.at.elapsed() < PROBE_INTERVAL
                && probe.server == server
            {
                return probe.found;
            }
        }
        let found = match server {
            None => Liveness::NoServer,
            Some(id) => match super::open::peer_node(&self.service, id) {
                Ok(super::open::PeerNode::Alive { .. }) => Liveness::Alive,
                Ok(super::open::PeerNode::Dead(_)) => Liveness::Dead,
                Ok(super::open::PeerNode::Unknown) | Err(_) => Liveness::Unknown,
            },
        };
        crate::locked(&PROBES).insert(stream, Probe { at: Instant::now(), server, found });
        found
    }
}

/// What a response says, once its frame verified against the request it answers.
fn classify(
    key: &StreamKey,
    client: u64,
    sequence: u64,
    header: &FrameHeader,
    payload: &[u8],
) -> CallEvent {
    if hypha_wire::frame::verify(key.response_code(client, sequence), header, payload).is_err() {
        return CallEvent::Invalid("crc");
    }
    let m = header.metadata;
    if m.schema_hash != key.hash() {
        return CallEvent::Invalid("schema");
    }
    if m.sequence != sequence {
        return CallEvent::Invalid("sequence");
    }
    let text = || String::from_utf8_lossy(payload).into_owned();
    match header.flags {
        0 => CallEvent::Answered(payload.to_vec()),
        FLAG_REFUSED => CallEvent::Refused(text()),
        FLAG_FAILED => CallEvent::Failed(text()),
        _ => CallEvent::Invalid("flags"),
    }
}
