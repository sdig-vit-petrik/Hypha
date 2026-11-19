//! The node side of the upstream hold of a replay cycle. The hold rules are in
//! `hypha_runtime::upstream_hold`.

use std::sync::Arc;
use std::time::Instant;

use hypha_iceoryx2::node_names;
use hypha_runtime::upstream_hold::{self as runtime, HoldError, Limit};

use crate::cycle::CycleInputSource;
use crate::node::HyphaNode;
use crate::node_progress::{self, valid_clock};
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
    progress_ids: Vec<u128>,
}

impl UpstreamHold {
    /// Resolve the upstream names of the cycle, and return the limit they give now.
    ///
    /// The limit is read before the inputs of the turn are drained, so every output that
    /// a reported clock covers is already queued when the drain runs. Before any
    /// resolution it holds every deadline for this turn.
    pub(crate) fn settle(
        &mut self,
        inputs: &[Arc<CycleInputSource>],
        node: &HyphaNode,
    ) -> Result<Limit, Error> {
        self.input_ids.clear();
        for source in inputs {
            if let Some(subscriber) = input_subscriber(source)? {
                subscriber.port.publisher_node_ids(&mut self.input_ids);
            }
        }
        self.input_ids.sort_unstable();
        self.input_ids.dedup();
        self.progress_ids.clear();
        {
            let Ok(progress) = node.progress_subscriber.try_lock() else {
                return Ok(self.hold.limit_while_busy(clocks(node)));
            };
            let Some(progress) = progress.as_deref() else {
                self.hold.clear_resolution();
                return Ok(self.hold.limit(clocks(node)));
            };
            progress.port.publisher_node_ids(&mut self.progress_ids);
        }
        self.progress_ids.sort_unstable();
        self.progress_ids.dedup();
        self.hold.resolve(
            &self.input_ids,
            &self.progress_ids,
            &node.name,
            Instant::now(),
            |ids| node_names(ids).map_err(Error::from),
        )?;
        Ok(self.hold.limit(clocks(node)))
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
