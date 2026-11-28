//! The services of a step test (`hypha::testing::StepHarness`, `docs/cpp-api.md` §9): the
//! servers and clients of one cycle that was not started, whose peers are the test.
//!
//! A test delivers requests to the servers and gives the clients' calls their outcomes; a
//! step reads, answers and calls through the cycle's services as a started cycle's step
//! does, and the test reads what the step answered and called. Which requests a step at T
//! reads (those stamped before T, in step order, at most `per_step`), what an unanswered or
//! a failed step's request becomes, which outcomes a step reads, which it carries after a
//! failure and which it counts unread: every rule is the cycle's (`cycle.rs`), run on fed
//! ends (`port.rs`) in place of the bus.

use crate::config::HyphaConfig;
use crate::cycle::{EmissionView, with_service_emission};
use crate::status::Error;

use super::cycle::CycleServices;
use super::outcome::CallOutcome;
use super::port::{FED_PEER, FedAnswer, FedCall, Slot};
use super::spec::ServiceDecl;

/// The servers and clients of a cycle under a step test. Opaque to C callers.
pub struct HyphaStepServices {
    services: CycleServices,
    config: Option<HyphaConfig>,
    cycle: String,
    /// The calls each client sent after the last step, in call order.
    sent: Vec<Vec<FedCall>>,
}

/// What became of a request a test delivered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestState {
    /// No step read it yet.
    Queued,
    /// A step read it and deferred it; no answer was given yet.
    Deferred,
    /// The answer, stamped with the time of the step it left after.
    Answered(FedAnswer, i64),
}

/// Make the services of the cycle `cycle` for a step test; `config` holds the tables of its
/// ports, if any.
#[must_use]
pub fn step_services_create(config: Option<&HyphaConfig>, cycle: &str) -> HyphaStepServices {
    let mut services = CycleServices::default();
    services.set_reporters(vec![FED_PEER]);
    // A cycle that was not started may have no name yet; the logs name the test then.
    let cycle = if cycle.is_empty() { "(step test)" } else { cycle };
    HyphaStepServices {
        services,
        config: config.cloned(),
        cycle: cycle.to_owned(),
        sent: Vec::new(),
    }
}

impl HyphaStepServices {
    /// Bind the server of `decl`; returns its index, as `cycle_bind_server` returns one.
    ///
    /// # Errors
    /// `InvalidArgument` for a malformed table or an invalid policy.
    pub fn bind_server(&mut self, decl: &ServiceDecl<'_>) -> Result<u32, Error> {
        crate::last_error::clear();
        self.services.add_fed_server(self.config.as_ref(), decl)
    }

    /// Bind a client of `decl`; returns its index, as `cycle_bind_client` returns one.
    ///
    /// # Errors
    /// As [`Self::bind_server`].
    pub fn bind_client(&mut self, decl: &ServiceDecl<'_>) -> Result<u32, Error> {
        crate::last_error::clear();
        let index = self.services.add_fed_client(self.config.as_ref(), decl)?;
        self.sent.push(Vec::new());
        Ok(index)
    }

    /// Deliver the encoded `request` of the client node `client` to server `server`, sent at
    /// `stamp`, or just before the next step for `None`. Returns the request's id.
    ///
    /// # Errors
    /// `InvalidArgument` for an unknown server.
    pub fn deliver(
        &mut self,
        server: u32,
        request: &[u8],
        client: &str,
        stamp: Option<i64>,
    ) -> Result<u64, Error> {
        self.services.with_fed_server(server, |fed| fed.deliver(request.to_vec(), client, stamp))
    }

    /// End the oldest call in flight of client `client` with `outcome`, its answer stamped
    /// `stamp`, or read by the next step for `None`.
    ///
    /// # Errors
    /// `InvalidArgument` for an unknown client; `InvalidState` when no call is in flight.
    pub fn feed(
        &mut self,
        client: u32,
        outcome: CallOutcome,
        stamp: Option<i64>,
    ) -> Result<(), Error> {
        let fed = self.services.with_fed_client(client, |fed| fed.finish_oldest(outcome, stamp))?;
        if fed { Ok(()) } else { Err(Error::InvalidState) }
    }

    /// What became of request `id` of server `server`.
    ///
    /// # Errors
    /// `InvalidArgument` for an unknown server or request.
    pub fn request_state(&self, server: u32, id: u64) -> Result<RequestState, Error> {
        let queued = self.services.queues(server, Slot::Fed(id));
        self.services.with_fed_server(server, |fed| {
            if queued || fed.holds(id) {
                return Ok(RequestState::Queued);
            }
            match fed.answer_of(id) {
                Ok(Some((answer, stamp))) => Ok(RequestState::Answered(answer.clone(), *stamp)),
                Ok(None) => Ok(RequestState::Deferred),
                Err(_) => Err(Error::InvalidArgument),
            }
        })?
    }

    /// The calls client `client` sent after the last step, in call order.
    ///
    /// # Errors
    /// `InvalidArgument` for an unknown client.
    pub fn sent(&self, client: u32) -> Result<&[FedCall], Error> {
        let index = usize::try_from(client).map_err(|_| Error::InvalidArgument)?;
        self.sent.get(index).map(Vec::as_slice).ok_or(Error::InvalidArgument)
    }

    /// Run one step at framework time `now`, as a started cycle on simulated time runs it:
    /// `body` gets the step's emission and returns whether the step succeeded. After it,
    /// the answers and calls of a successful step leave; a failed step's requests end as
    /// failed and its calls `NotSent`.
    pub fn step(&mut self, now: i64, body: impl FnOnce(&mut EmissionView<'_, '_>) -> bool) {
        let before = now.saturating_sub(1);
        for server in 0..self.services.server_count() {
            let _ = self.services.with_fed_server(server, |fed| fed.stamp_unstamped(before));
        }
        self.services.begin(now, now, Some(now), None);
        let succeeded = with_service_emission(&mut self.services, body);
        self.services.finish(succeeded, &self.cycle);
        for client in 0..self.services.client_count() {
            let sent = self.services.with_fed_client(client, super::port::FedClient::take_sent);
            let index = usize::try_from(client).unwrap_or(usize::MAX);
            if let (Ok(sent), Some(slot)) = (sent, self.sent.get_mut(index)) {
                *slot = sent;
            }
        }
    }
}
