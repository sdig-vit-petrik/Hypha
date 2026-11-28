//! One cycle: the snapshots any thread may read, around the step state one tick borrows.

use std::cell::{Ref, RefCell, RefMut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use hypha_cycle::StagedMessage;

use super::image::{HyphaCycleEmission, HyphaCycleImage};
use super::source::CycleInputSource;
use super::types::{
    HyphaCycleBindingOptions, HyphaCycleBindingView, HyphaCycleHealth, HyphaCycleTime,
};
use crate::introspection::IntrospectionRow;
use crate::loan::HyphaLoan;
use crate::publisher::HyphaPublisher;
use crate::status::Error;
use crate::waitset::{HyphaAttachmentId, HyphaCallbackResult, HyphaWaitSet};

/// Names one cycle of a `WaitSet`: never zero, and never the id of another cycle of
/// that `WaitSet`, live or destroyed.
pub type CycleId = u64;

/// The C step callback of a cycle and its context, owned by the cycle until its
/// destruction. Its drop releases the context, once, after no step can run again.
pub trait CycleStepHandler: Send {
    /// Run one step.
    fn step(
        &mut self,
        image: *const HyphaCycleImage,
        time: *const HyphaCycleTime,
        emission: *mut HyphaCycleEmission,
    ) -> HyphaCallbackResult;
}

/// What any thread may read of one cycle, while a step runs too. `hypha_cycle_health`
/// and `hypha_cycle_binding` read it without the `WaitSet`.
#[derive(Default)]
pub struct CycleSnapshots {
    pub(crate) name: String,
    pub(crate) period_nanos: i64,
    pub(crate) committed: AtomicBool,
    pub(crate) health: Mutex<HyphaCycleHealth>,
    /// The distribution of the step durations, which `~/state` reports.
    pub(crate) step_times: Mutex<hypha_cycle::StepTimes>,
    pub(crate) bindings: Mutex<Vec<HyphaCycleBindingView>>,
    /// The source counters of each input, in binding order, at the last step.
    pub(crate) input_counts: Mutex<Vec<InputCounts>>,
    /// What each input reads about the other streams of its topic, in binding order; `None`
    /// for an input without a subscriber of one stream.
    pub(crate) watches: Mutex<Vec<Option<crate::topic_streams::Watch>>>,
}

// Another thread reads it during a run (`hypha_cycle_health`, `hypha_cycle_binding`).
const _: () = {
    const fn shared_across_threads<T: Send + Sync>() {}
    shared_across_threads::<CycleSnapshots>();
};

/// One cycle of a `WaitSet`, which owns it and names it by [`CycleId`].
///
/// Everything a step mutates is in `step`, borrowed for the step: a call that lands
/// inside a step finds it borrowed and returns `InvalidState`. What another thread may
/// read is in `snapshots`.
pub(crate) struct Cycle {
    /// Names this cycle in its `WaitSet`'s registrations.
    pub(crate) id: CycleId,
    pub(crate) snapshots: Arc<CycleSnapshots>,
    /// The cycle row of the node introspection, removed with this cycle.
    pub(crate) introspection: IntrospectionRow,
    pub(crate) step: RefCell<CycleStep>,
}

/// The counters of one input's source that `~/state` reports.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct InputCounts {
    /// Frames its subscriber refused.
    pub(crate) rejected: u64,
    /// Samples a port could not decode.
    pub(crate) undecodable: u64,
}

/// The state one step reads and writes.
#[derive(Default)]
pub(crate) struct CycleStep {
    pub(crate) attachment: HyphaAttachmentId,
    /// The listener registrations that wake a triggered cycle, one per trigger topic that
    /// has a listener (a wildcard has none). Empty for a rate cycle.
    pub(crate) trigger_attachments: Vec<HyphaAttachmentId>,
    /// The step callback with its context. `None` only while the cycle drops.
    pub(crate) callback: Option<Box<dyn CycleStepHandler>>,
    pub(crate) runtime: hypha_cycle::CycleRuntime,
    pub(crate) inputs: Vec<Arc<CycleInputSource>>,
    pub(crate) input_options: Vec<HyphaCycleBindingOptions>,
    /// One role row per declaration, including disabled sources.
    pub(crate) input_rows: Vec<IntrospectionRow>,
    /// The port name of each input, in binding order, empty for a port without a table.
    pub(crate) port_names: Vec<String>,
    pub(crate) outputs: Vec<CycleOutput>,
    /// The TF inputs, each over two of the input bindings, in bind order.
    pub(crate) transforms: Vec<super::transforms::TfInput>,
    /// Per-tick emission staging, retained between timer firings. `None` is a dropped message.
    pub(crate) emissions: Vec<StagedMessage<Option<HyphaLoan>>>,
    /// The loans of the step's outputs, by token, retained between steps and empty there.
    pub(crate) lent: Vec<Option<super::image::LentMessage>>,
    /// Callback supervision scratch, allocated once at commit.
    pub(super) supervision: Vec<RefCell<super::image::SupervisionCopy>>,
    pub(crate) class_based: bool,
    pub(crate) scheduled_nanos: i64,
    /// The data-time cut of the replay step that runs next, set by the dispatcher. `None`
    /// for a step in host time, which drains its own inputs.
    pub(crate) input_cut: Option<i64>,
    /// What the trigger ports hold in the next step of a triggered cycle, set by the
    /// dispatcher.
    pub(crate) trigger_step: Option<hypha_cycle::TriggerStep>,
    /// The `/clock` driver settings of a simulator cycle. `None` for any other cycle.
    pub(crate) driver: Option<super::DriverSetup>,
    /// The cycle's servers (`Requests` ports) and clients (`Calls` ports).
    pub(crate) services: crate::service::CycleServices,
}

impl Cycle {
    pub(crate) fn name(&self) -> &str {
        &self.snapshots.name
    }

    /// The period of this cycle in nanoseconds, zero for a triggered cycle.
    pub(crate) fn period_nanos(&self) -> i64 {
        self.snapshots.period_nanos
    }

    pub(crate) fn is_committed(&self) -> bool {
        self.snapshots.committed.load(Ordering::Acquire)
    }

    /// Whether `source` was created on `set`, this cycle's `WaitSet`.
    pub(crate) fn owns_source(set: &HyphaWaitSet, source: &CycleInputSource) -> bool {
        source.created_on(set) && source.node == set.node_address()
    }

    /// The step state, for a call that may change it: `InvalidState` once committed,
    /// while `set` runs, or inside a step.
    pub(crate) fn configurable(&self, set: &HyphaWaitSet) -> Result<RefMut<'_, CycleStep>, Error> {
        if set.is_running() || self.is_committed() {
            return Err(Error::InvalidState);
        }
        self.step_mut()
    }

    /// The step state, mutable. `InvalidState` inside a step.
    pub(crate) fn step_mut(&self) -> Result<RefMut<'_, CycleStep>, Error> {
        self.step.try_borrow_mut().map_err(|_| Error::InvalidState)
    }

    /// The step state, shared. `InvalidState` inside a step.
    pub(crate) fn step_ref(&self) -> Result<Ref<'_, CycleStep>, Error> {
        self.step.try_borrow().map_err(|_| Error::InvalidState)
    }
}

/// A publisher exclusively owned by one cycle output binding.
pub(crate) struct CycleOutput {
    pub(crate) publisher: Box<HyphaPublisher>,
}

impl Drop for Cycle {
    /// Releases the callback context once, before the rest of the cycle goes.
    fn drop(&mut self) {
        drop(self.step.get_mut().callback.take());
    }
}
