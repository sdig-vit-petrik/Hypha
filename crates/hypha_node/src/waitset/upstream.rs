//! The node side of the upstream hold of a replay cycle. The hold rules are in
//! `hypha_runtime::upstream_hold`.

use std::sync::Arc;
use std::time::Instant;

use hypha_iceoryx2::node_names;
use hypha_runtime::upstream_hold::{self as runtime, HoldError, Limit};

use crate::cycle::CycleInputSource;
use crate::node::HyphaNode;
use crate::node_progress::{self, valid_clock};
use crate::service::CycleServices;
use crate::status::Error;
use crate::subscriber::HyphaSubscriber;

impl From<HoldError> for Error {
    /// Every refusal of the hold is an invalid state of the cycle.
    fn from(_: HoldError) -> Self {
        Error::InvalidState
    }
}

/// The held deadlines of one replay cycle.
#[derive(Default)]
pub(crate) struct UpstreamHold {
    /// The hold rules and the state they keep.
    pub(crate) hold: runtime::UpstreamHold,
    /// Registry scratch retained across turns, including while a progress input is busy.
    input_ids: Vec<u128>,
    service_ids: Vec<u128>,
    progress_ids: Vec<u128>,
    /// The service peers that report progress, as last found.
    reporters: Vec<u128>,
}

/// The limits the upstream nodes of a replay cycle give now: those of its topic edges, and
/// those of its request and response edges, which are cut strictly before a step's time.
pub(crate) struct Limits {
    pub(crate) topic: Limit,
    pub(crate) service: Limit,
    /// The node ids of the service peers that report progress, sorted: the cut of a step
    /// binds their requests and answers only.
    pub(crate) reporters: Vec<u128>,
}

impl UpstreamHold {
    /// Resolve the upstream names of the cycle, and return the limits they give now: the
    /// publishers of its inputs, and the service peers of its `Requests` and `Calls` ports
    /// (the clients of its servers, the servers of its clients).
    ///
    /// The limit is read before the inputs of the turn are drained, so every output that
    /// a reported clock covers is already queued when the drain runs. Before any
    /// resolution it holds every deadline for this turn.
    pub(crate) fn settle(
        &mut self,
        inputs: &[Arc<CycleInputSource>],
        services: &CycleServices,
        node: &HyphaNode,
    ) -> Result<Limits, Error> {
        let mut subscribers = Vec::with_capacity(inputs.len());
        for source in inputs {
            if let Some(subscriber) = input_subscriber(source)? {
                subscribers.push(subscriber);
            }
        }
        self.input_ids.clear();
        for subscriber in subscribers {
            subscriber.port.publisher_node_ids(&mut self.input_ids);
        }
        self.input_ids.sort_unstable();
        self.input_ids.dedup();
        self.service_ids.clear();
        services.peer_ids(&mut self.service_ids);
        self.service_ids.sort_unstable();
        self.service_ids.dedup();
        self.progress_ids.clear();
        {
            let Ok(progress) = node.progress_subscriber.try_lock() else {
                let topic = self.hold.limit_while_busy(clocks(node));
                let service = self.hold.service_limit(clocks(node));
                return Ok(Limits { topic, service, reporters: self.reporters.clone() });
            };
            let Some(progress) = progress.as_deref() else {
                self.hold.clear_resolution();
                self.reporters.clear();
                return Ok(self.limits(node));
            };
            progress.port.publisher_node_ids(&mut self.progress_ids);
        }
        self.progress_ids.sort_unstable();
        self.progress_ids.dedup();
        self.reporters.clear();
        let progress_ids = &self.progress_ids;
        let reporting = self.service_ids.iter().filter(|id| progress_ids.binary_search(id).is_ok());
        self.reporters.extend(reporting);
        self.hold.resolve(
            &self.input_ids,
            &self.service_ids,
            &self.progress_ids,
            &node.name,
            Instant::now(),
            |ids| node_names(ids).map_err(Error::from),
        )?;
        Ok(self.limits(node))
    }

    fn limits(&self, node: &HyphaNode) -> Limits {
        Limits {
            topic: self.hold.limit(clocks(node)),
            service: self.hold.service_limit(clocks(node)),
            reporters: self.reporters.clone(),
        }
    }
}

/// The valid clock and the last clock of each upstream name.
fn clocks(node: &HyphaNode) -> impl Fn(&str) -> (Option<u64>, Option<u64>) + '_ {
    let epoch = node_progress::epoch(node);
    move |name| (valid_clock(node, name, epoch), valid_clock(node, name, 0))
}

/// The subscriber an input reads through. `None` for an input that has no subscriber.
fn input_subscriber(source: &CycleInputSource) -> Result<Option<&HyphaSubscriber>, Error> {
    match source.subscriber.as_deref() {
        Some(subscriber) => Ok(Some(subscriber)),
        None if source.history.is_none() => Ok(None),
        None => Err(Error::InvalidState),
    }
}
