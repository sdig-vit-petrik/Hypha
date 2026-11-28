//! The servers and clients a `WaitSet` drives (`docs/design/native-services.md` §2, §3).
//!
//! A server's request event wakes the run, which hands each request to its handler. A
//! client's response event wakes the run too; each turn every client collects its answers,
//! sends the calls it held for a server that appeared (the graph event wakes the run), and
//! ends the calls whose deadline passed. Outcome callbacks run here, on the run's thread,
//! never inside `call()`.

use std::sync::Arc;

use super::dispatch::{CallbackDispatcher, Subject, is_detached_in, timed};
use super::registrations::{Request, attach};
use super::{AttachmentId, EventOutcome, HyphaCallbackResult, Progression, Registration, Target};
use crate::node::HyphaNode;
use crate::service::{ClientCore, ServeHandler, ServerCore};
use crate::status::Error;
use hypha_core::dispatch::{CallbackBudget, callback_budget};

/// What a service registration drives.
pub(crate) enum ServiceTarget {
    /// A server, with the handler of its requests.
    Server { core: Arc<ServerCore>, handler: Box<dyn ServeHandler> },
    /// A client that delivers its outcomes on this `WaitSet`.
    Client(Arc<ClientCore>),
}

/// Attach `target`, woken by `listener`, to the default `WaitSet` of `node`.
///
/// # Errors
/// `InvalidState` during a run or while another thread is inside a call on the `WaitSet`;
/// `ResourceLimit` at the registration limit; the transport's error otherwise.
pub(crate) fn attach_service(
    node: &Arc<HyphaNode>,
    listener: hypha_iceoryx2::Iox2Listener,
    target: ServiceTarget,
) -> Result<AttachmentId, Error> {
    super::configuration::with_default_waitset(node, |set| {
        let _claim = set.shared.claim()?;
        attach(set, Request::Service { listener: Box::new(listener), target })
    })?
}

/// Detach the service registration `attachment` from the default `WaitSet` of `node`. A
/// detach that is refused (another thread during a run) leaves a registration whose
/// endpoint is gone, which drives nothing.
pub(crate) fn detach_service(node: &Arc<HyphaNode>, attachment: AttachmentId) {
    let detached = super::configuration::with_default_waitset(node, |set| {
        let _claim = set.shared.claim()?;
        super::waitset_detach(set, attachment)
    });
    if let Err(error) = detached.and_then(|inner| inner) {
        log::debug!(target: "hypha", "hypha: a service endpoint stays attached until its WaitSet goes: {error:?}");
    }
}

impl CallbackDispatcher<'_> {
    /// Serve the requests of a server, or collect the outcomes of a client, that `attachment`
    /// woke. `None` when `attachment` is no service registration.
    pub(super) fn on_service_event(&mut self, attachment: AttachmentId) -> Option<EventOutcome> {
        let budget = callback_budget(self.node.slow_callback_ms, None);
        let (warn_interval, batch) = (self.warn_interval_nanos, self.batch_size);
        let registration =
            self.registrations.iter_mut().find(|registration| registration.id == attachment)?;
        let Registration { ref mut target, ref mut warnings, .. } = *registration;
        let Target::Service(service) = target else {
            return None;
        };
        let (answer, more, slow) = match service {
            ServiceTarget::Server { core, handler } => {
                core.drop_abandoned();
                let mut more = false;
                let name = core.name().to_owned();
                let subject = Subject::Service(&name);
                let (result, was_slow) = timed(warnings, subject, budget, warn_interval, || {
                    let (answer, left) = core.serve_turn(handler.as_mut(), batch);
                    more = left;
                    answer.unwrap_or_else(HyphaCallbackResult::continue_run)
                });
                (stops(result).then_some(result), more, u64::from(was_slow))
            }
            ServiceTarget::Client(core) => {
                let finished =
                    core.drive(hypha_time::scheduling_now().nanos(), &self.node.transport);
                let name = core.name();
                let (answer, slow) =
                    deliver(finished, warnings, Subject::Service(&name), budget, warn_interval);
                (answer, false, slow)
            }
        };
        self.count_slow(slow);
        let outcome = if more { EventOutcome::MoreWork } else { EventOutcome::Drained };
        let Some(result) = answer else {
            return Some(outcome);
        };
        Some(match self.act(attachment, result) {
            Progression::Continue => outcome,
            Progression::Stop => EventOutcome::Stop,
            Progression::Fail(cause) => EventOutcome::Fail(cause),
        })
    }

    /// Each turn: every client collects its outcomes, sends what it held for a server, and
    /// ends the calls past their deadline; every server drops the requests of clients that
    /// left.
    pub(super) fn drive_services(&mut self) -> Progression {
        let budget = callback_budget(self.node.slow_callback_ms, None);
        let warn_interval = self.warn_interval_nanos;
        let now = hypha_time::scheduling_now().nanos();
        let mut answers = Vec::new();
        let mut slow = 0_u64;
        for registration in
            self.registrations.iter_mut().filter(|r| !is_detached_in(self.detached, r.id))
        {
            let Registration { id, ref mut target, ref mut warnings, .. } = *registration;
            match target {
                Target::Service(ServiceTarget::Client(core)) => {
                    let finished = core.drive(now, &self.node.transport);
                    let name = core.name();
                    let subject = Subject::Service(&name);
                    let (answer, was_slow) =
                        deliver(finished, warnings, subject, budget, warn_interval);
                    slow = slow.saturating_add(was_slow);
                    answers.extend(answer.map(|answer| (id, answer)));
                }
                Target::Service(ServiceTarget::Server { core, .. }) => core.drop_abandoned(),
                _ => {}
            }
        }
        self.count_slow(slow);
        for (id, answer) in answers {
            let decision = self.act(id, answer);
            if decision != Progression::Continue {
                return decision;
            }
        }
        Progression::Continue
    }

    /// The earliest deadline of a client's call, on the host clock; `None` on simulated
    /// time, whose clock wakes the run.
    pub(super) fn service_deadline(&self) -> Option<i64> {
        if self.node.on_sim_time() {
            return None;
        }
        let live = self.registrations.iter().filter(|r| !is_detached_in(self.detached, r.id));
        live.filter_map(|registration| match registration.target {
            Target::Service(ServiceTarget::Client(ref core)) => core.next_deadline(),
            _ => None,
        })
        .min()
        .map(|deadline| deadline.max(hypha_time::steady_now().nanos()))
    }
}

/// Run each outcome's callback, timed as one callback of the registration. Returns the
/// first answer that does not continue the run, and the slow callbacks.
fn deliver(
    finished: Vec<crate::service::Finished>,
    warnings: &mut super::CallbackWarnings,
    subject: Subject<'_>,
    budget: CallbackBudget,
    warn_interval: u64,
) -> (Option<HyphaCallbackResult>, u64) {
    let mut answer = None;
    let mut slow = 0_u64;
    for (done, result, _) in finished {
        let Some(done) = done else {
            continue;
        };
        let (outcome, was_slow) =
            timed(warnings, subject, budget, warn_interval, || done.done(&result));
        slow = slow.saturating_add(u64::from(was_slow));
        if stops(outcome) && answer.is_none() {
            answer = Some(outcome);
        }
    }
    (answer, slow)
}

/// Whether a callback's answer ends or fails the run, rather than continue it.
fn stops(result: HyphaCallbackResult) -> bool {
    result.action != super::HYPHA_ACTION_CONTINUE || result.status != 0
}
