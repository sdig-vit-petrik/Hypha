//! The two ends a server and a client speak through: the bus (iceoryx2), or a fed end that
//! a step test fills and reads (`hypha::testing::StepHarness`, `docs/cpp-api.md` §9).
//!
//! A fed end has no node and no bus. It holds the requests a test delivered and the
//! answers the server gave, or the calls a client sent and the outcomes the test gave
//! them. Every rule of what a step reads, answers, sends and carries stays in the cycle's
//! services (`cycle.rs`), the server (`server.rs`) and the client (`client.rs`), which a
//! fed end runs unchanged.

use std::collections::VecDeque;

use hypha_core::port::{CallSendError, PortError, ReplyError, ServicePolicy};
use hypha_iceoryx2::{CallEvent, CallId, Iox2Client, Iox2Server, Liveness, RequestHandle};

use super::outcome::CallOutcome;

/// The node id the fed ends give the test's clients and servers: one peer that takes part
/// in the replay, so a fed step cuts what it reads by stamp, as a replay step does.
pub(crate) const FED_PEER: u128 = 1;

/// Which request a server answers: a slot of the bus, or a request a test delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Slot {
    Bus(RequestHandle),
    Fed(u64),
}

/// A request a server received, with who sent it.
#[derive(Debug, Clone)]
pub(crate) struct Received {
    pub(crate) slot: Slot,
    /// The client's sender id.
    pub(crate) client: u64,
    /// The client's sequence of this request.
    pub(crate) sequence: u64,
    /// The client's framework time at the send.
    pub(crate) stamp: i64,
    /// The client's node name; empty when it cannot be read.
    pub(crate) client_node: String,
    /// The client's port label.
    pub(crate) client_port: String,
    /// The client's node id; 0 when it cannot be read.
    pub(crate) client_node_id: u128,
}

/// A server's end.
pub(crate) enum ServerPort {
    /// Boxed: a fed end is a small fraction of its size.
    Bus(Box<Iox2Server>),
    Fed(FedServer),
}

impl ServerPort {
    pub(crate) fn reply(
        &mut self,
        slot: Slot,
        response: &[u8],
        stamp: u64,
    ) -> Result<(), ReplyError> {
        match (self, slot) {
            (Self::Bus(port), Slot::Bus(handle)) => port.reply(handle, response, stamp),
            (Self::Fed(port), Slot::Fed(id)) => port.reply(id, response, stamp),
            _ => Err(ReplyError::UnknownRequest),
        }
    }

    pub(crate) fn refuse(
        &mut self,
        slot: Slot,
        reason: &str,
        stamp: u64,
    ) -> Result<(), ReplyError> {
        match (self, slot) {
            (Self::Bus(port), Slot::Bus(handle)) => port.refuse(handle, reason, stamp),
            (Self::Fed(port), Slot::Fed(id)) => {
                port.answer(id, FedAnswer::Refused(reason.into()), stamp)
            }
            _ => Err(ReplyError::UnknownRequest),
        }
    }

    pub(crate) fn fail(&mut self, slot: Slot, reason: &str, stamp: u64) -> Result<(), ReplyError> {
        match (self, slot) {
            (Self::Bus(port), Slot::Bus(handle)) => port.fail(handle, reason, stamp),
            (Self::Fed(port), Slot::Fed(id)) => {
                port.answer(id, FedAnswer::Failed(reason.into()), stamp)
            }
            _ => Err(ReplyError::UnknownRequest),
        }
    }

    pub(crate) fn policy(&self) -> &ServicePolicy {
        match self {
            Self::Bus(port) => port.policy(),
            Self::Fed(port) => &port.policy,
        }
    }

    /// The next request and a copy of its payload.
    pub(crate) fn receive(&mut self) -> Result<Option<(Received, Vec<u8>)>, PortError> {
        match self {
            Self::Bus(port) => {
                let Some(received) = port.receive()? else {
                    return Ok(None);
                };
                let payload = port.payload(received.handle).map_or_else(Vec::new, <[u8]>::to_vec);
                let request = Received {
                    slot: Slot::Bus(received.handle),
                    client: received.client,
                    sequence: received.sequence,
                    stamp: i64::try_from(received.framework_sent_nanos).unwrap_or(0),
                    client_node: received.client_node,
                    client_port: received.client_port,
                    client_node_id: received.client_node_id,
                };
                Ok(Some((request, payload)))
            }
            Self::Fed(port) => Ok(port.queue.pop_front()),
        }
    }

    pub(crate) fn client_node_ids(&self, ids: &mut Vec<u128>) {
        match self {
            Self::Bus(port) => port.client_node_ids(ids),
            Self::Fed(_) => ids.push(FED_PEER),
        }
    }

    pub(crate) fn drop_abandoned(&mut self) {
        if let Self::Bus(port) = self {
            port.drop_abandoned();
        }
    }
}

/// What a server's step answered to a request a test delivered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FedAnswer {
    /// The encoded response.
    Replied(Vec<u8>),
    /// A refusal with its reason: `refuse()`, a request left unanswered, a response above the
    /// bound.
    Refused(String),
    /// The step that held it failed: its client reads `ServerLost` with the reason.
    Failed(String),
}

/// A request a test delivered.
struct FedRequest {
    /// The answer and its stamp, once the server gave one.
    answer: Option<(FedAnswer, i64)>,
}

/// The end of a server whose clients are a test.
pub(crate) struct FedServer {
    policy: ServicePolicy,
    /// Delivered and not yet received by the cycle, with their payloads.
    queue: VecDeque<(Received, Vec<u8>)>,
    /// Requests stamped at the next step, which `stamp_unstamped` stamps.
    unstamped: Vec<u64>,
    requests: Vec<FedRequest>,
}

impl FedServer {
    pub(crate) fn new(policy: ServicePolicy) -> Self {
        Self { policy, queue: VecDeque::new(), unstamped: Vec::new(), requests: Vec::new() }
    }

    /// Deliver a request of `client`, stamped `stamp` (`None`: just before the next step).
    /// Returns its id.
    pub(crate) fn deliver(&mut self, payload: Vec<u8>, client: &str, stamp: Option<i64>) -> u64 {
        let id = u64::try_from(self.requests.len()).unwrap_or(u64::MAX);
        self.requests.push(FedRequest { answer: None });
        if stamp.is_none() {
            self.unstamped.push(id);
        }
        let received = Received {
            slot: Slot::Fed(id),
            client: id,
            sequence: id,
            stamp: stamp.unwrap_or(0),
            client_node: client.to_owned(),
            client_port: client.to_owned(),
            client_node_id: FED_PEER,
        };
        self.queue.push_back((received, payload));
        id
    }

    /// Stamp each request delivered with no stamp at `stamp`.
    pub(crate) fn stamp_unstamped(&mut self, stamp: i64) {
        for id in std::mem::take(&mut self.unstamped) {
            let queued = self.queue.iter_mut().find(|(received, _)| received.slot == Slot::Fed(id));
            if let Some((received, _)) = queued {
                received.stamp = stamp;
            }
        }
    }

    /// Whether request `id` was delivered and not yet received by the cycle.
    pub(crate) fn holds(&self, id: u64) -> bool {
        self.queue.iter().any(|(received, _)| received.slot == Slot::Fed(id))
    }

    /// The answer to request `id` and its stamp; `None` while it has none.
    ///
    /// # Errors
    /// `UnknownRequest` for an id never delivered.
    pub(crate) fn answer_of(&self, id: u64) -> Result<Option<&(FedAnswer, i64)>, ReplyError> {
        let index = usize::try_from(id).map_err(|_| ReplyError::UnknownRequest)?;
        let request = self.requests.get(index).ok_or(ReplyError::UnknownRequest)?;
        Ok(request.answer.as_ref())
    }

    fn reply(&mut self, id: u64, response: &[u8], stamp: u64) -> Result<(), ReplyError> {
        let limit = self.policy.max_response_bytes;
        if response.len() > limit {
            let reason = hypha_iceoryx2::response_too_large(response.len(), &self.policy);
            self.answer(id, FedAnswer::Refused(reason), stamp)?;
            return Err(ReplyError::TooLarge { requested: response.len(), limit });
        }
        self.answer(id, FedAnswer::Replied(response.to_vec()), stamp)
    }

    fn answer(&mut self, id: u64, answer: FedAnswer, stamp: u64) -> Result<(), ReplyError> {
        let index = usize::try_from(id).map_err(|_| ReplyError::UnknownRequest)?;
        let request = self.requests.get_mut(index).ok_or(ReplyError::UnknownRequest)?;
        if request.answer.is_some() {
            return Err(ReplyError::UnknownRequest);
        }
        request.answer = Some((answer, i64::try_from(stamp).unwrap_or(i64::MAX)));
        Ok(())
    }
}

/// An outcome a client found, with the stamp of the server's answer (`None` when the call
/// ended without one).
pub(crate) struct Polled {
    pub(crate) id: CallId,
    pub(crate) outcome: CallOutcome,
    pub(crate) stamp: Option<i64>,
}

/// A client's end.
pub(crate) enum ClientPort {
    /// Boxed: a fed end is a small fraction of its size.
    Bus(Box<Iox2Client>),
    Fed(FedClient),
}

impl ClientPort {
    pub(crate) fn in_flight(&self) -> usize {
        match self {
            Self::Bus(port) => port.in_flight(),
            Self::Fed(port) => port.open.len(),
        }
    }

    pub(crate) fn policy(&self) -> &ServicePolicy {
        match self {
            Self::Bus(port) => port.policy(),
            Self::Fed(port) => &port.policy,
        }
    }

    pub(crate) fn server_available(&self) -> bool {
        match self {
            Self::Bus(port) => port.server_available(),
            Self::Fed(_) => true,
        }
    }

    pub(crate) fn server_node_ids(&self, ids: &mut Vec<u128>) {
        match self {
            Self::Bus(port) => port.server_node_ids(ids),
            Self::Fed(_) => ids.push(FED_PEER),
        }
    }

    pub(crate) fn send(&mut self, request: &[u8], stamp: u64) -> Result<CallId, CallSendError> {
        match self {
            Self::Bus(port) => port.send(request, stamp),
            Self::Fed(port) => Ok(port.send(request, stamp)),
        }
    }

    /// Push the outcomes found now onto `out`; a late answer to a call that timed out is
    /// not one.
    pub(crate) fn poll(&mut self, out: &mut Vec<Polled>) {
        match self {
            Self::Bus(port) => {
                let mut reports = Vec::new();
                port.poll(&mut reports);
                out.extend(reports.into_iter().filter(|report| !report.late).map(|report| {
                    Polled {
                        id: report.id,
                        outcome: outcome_of(report.event),
                        stamp: report
                            .framework_sent_nanos
                            .and_then(|stamp| i64::try_from(stamp).ok()),
                    }
                }));
            }
            Self::Fed(port) => out.append(&mut port.outcomes),
        }
    }

    /// End the wait for call `id` at its deadline: false when the call is gone already.
    pub(crate) fn expire(&mut self, id: CallId) -> bool {
        match self {
            Self::Bus(port) => port.expire(id),
            // The test's server lets a call it never answered go: its slot is free again.
            Self::Fed(port) => port.close(id),
        }
    }

    pub(crate) fn abandon(&mut self, id: CallId) {
        match self {
            Self::Bus(port) => {
                let _ = port.abandon(id);
            }
            Self::Fed(port) => {
                port.close(id);
            }
        }
    }

    pub(crate) fn probe_server(&mut self) -> Liveness {
        match self {
            Self::Bus(port) => port.probe_server(),
            Self::Fed(_) => Liveness::Alive,
        }
    }
}

/// A call a client sent to a test.
#[derive(Debug, Clone)]
pub struct FedCall {
    /// The encoded request.
    pub request: Vec<u8>,
    /// The framework time it carries: the time of the step that made it.
    pub stamp: i64,
}

/// The end of a client whose server is a test.
pub(crate) struct FedClient {
    policy: ServicePolicy,
    next_id: u64,
    /// The calls in flight, oldest first.
    open: Vec<CallId>,
    /// The calls sent since `take_sent`.
    sent: Vec<FedCall>,
    /// The outcomes the test gave, for the next poll.
    outcomes: Vec<Polled>,
}

impl FedClient {
    pub(crate) fn new(policy: ServicePolicy) -> Self {
        Self { policy, next_id: 0, open: Vec::new(), sent: Vec::new(), outcomes: Vec::new() }
    }

    fn send(&mut self, request: &[u8], stamp: u64) -> CallId {
        let id = CallId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        self.open.push(id);
        let stamp = i64::try_from(stamp).unwrap_or(i64::MAX);
        self.sent.push(FedCall { request: request.to_vec(), stamp });
        id
    }

    fn close(&mut self, id: CallId) -> bool {
        let open = self.open.iter().position(|call| *call == id);
        open.map(|index| self.open.remove(index)).is_some()
    }

    /// End the oldest call in flight with `outcome`, stamped `stamp` (`None`: read by the
    /// next step). False when no call is in flight.
    pub(crate) fn finish_oldest(&mut self, outcome: CallOutcome, stamp: Option<i64>) -> bool {
        if self.open.is_empty() {
            return false;
        }
        let id = self.open.remove(0);
        self.outcomes.push(Polled { id, outcome, stamp });
        true
    }

    /// The calls sent since the last take, in call order.
    pub(crate) fn take_sent(&mut self) -> Vec<FedCall> {
        std::mem::take(&mut self.sent)
    }
}

/// The outcome of a report of the bus.
fn outcome_of(event: CallEvent) -> CallOutcome {
    match event {
        CallEvent::Answered(response) => CallOutcome::Answered(response),
        CallEvent::Refused(reason) => CallOutcome::Refused(reason),
        CallEvent::Failed(reason) => CallOutcome::ServerLost(reason),
        CallEvent::Invalid(check) => CallOutcome::ServerLost(format!("invalid response: {check}")),
        CallEvent::Lost => CallOutcome::ServerLost("the server left".to_owned()),
    }
}
