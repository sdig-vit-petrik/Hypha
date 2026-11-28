//! The frozen input image and the emission collection of one iteration.

use std::cell::RefCell;
use std::sync::Arc;

use hypha_cycle::{EmissionBudget, StagedMessage, Supervision};

use super::registration::CycleOutput;
use super::source::CycleInputSource;
use super::types::{
    HYPHA_CYCLE_VERDICT_NEVER_ARRIVED, HYPHA_CYCLE_VERDICT_STALE, HYPHA_CYCLE_VERDICT_USABLE,
    HyphaCycleBindingReport, HyphaCycleBindingView, HyphaCycleSampleView,
};
use crate::loan::HyphaLoan;
use crate::status::Error;

/// One callback-scoped frozen input image. Opaque to callers.
pub struct HyphaCycleImage {
    _private: [u8; 0],
}

/// One callback-scoped output emission collection. Opaque to callers.
///
/// Each message is one reservation (`hypha_cycle_emission_reserve`), written once
/// straight into shared memory, and one commit; the library sends or releases the bytes
/// after the step, and the caller never frees them. The first error of a step latches:
/// the step sends nothing and the run fails. A later send failure cannot undo an earlier
/// send. A `STOP` result, or `CONTINUE` with a status, sends nothing.
pub struct HyphaCycleEmission {
    _private: [u8; 0],
}

/// What one step reads. `HyphaCycleImage` is this view behind an opaque pointer.
pub struct ImageView<'a> {
    pub(super) captures: &'a [hypha_cycle::BindingCapture],
    /// The cycle's inputs, in binding order, for the binding health.
    pub(super) inputs: &'a [Arc<CycleInputSource>],
    /// The options of each binding, in binding order: whether it counts for health.
    pub(super) input_options: &'a [super::HyphaCycleBindingOptions],
    /// One supervision copy per binding, in binding order.
    pub(super) supervision: &'a [RefCell<SupervisionCopy>],
    /// The cycle's TF inputs, ingested for this step.
    pub(super) transforms: &'a [super::transforms::TfInput],
    pub(super) is_replay: bool,
}

/// The callback-scoped state of one binding. The runtime applies the updates after the step.
#[derive(Default)]
pub(super) struct SupervisionCopy {
    state: Option<Supervision>,
    pub(super) update: SupervisionUpdate,
    /// The capture as the step sees it after a rejection; `None` without one.
    rejudged: Option<hypha_cycle::BindingCapture>,
    /// The sequences the step rejected, in order.
    rejected: Vec<u64>,
    /// How many of them no other port of the source had rejected before.
    pub(super) newly_rejected: u64,
}

/// The updates one step made to a binding's supervision.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct SupervisionUpdate {
    pub(super) alive: Option<u64>,
    pub(super) ignored: Option<bool>,
}

impl SupervisionCopy {
    /// The sequences the step rejected and how many were new to the source, taken.
    pub(super) fn take_rejected(&mut self) -> (Vec<u64>, u64) {
        let newly = std::mem::take(&mut self.newly_rejected);
        (std::mem::take(&mut self.rejected), newly)
    }
}

impl<'a> ImageView<'a> {
    pub(super) fn new(
        captures: &'a [hypha_cycle::BindingCapture],
        inputs: &'a [Arc<CycleInputSource>],
        input_options: &'a [super::HyphaCycleBindingOptions],
        supervision: &'a mut [RefCell<SupervisionCopy>],
        transforms: &'a [super::transforms::TfInput],
        is_replay: bool,
    ) -> Self {
        for (copy, capture) in supervision.iter_mut().zip(captures) {
            let copy = copy.get_mut();
            copy.state = capture.supervision().cloned();
            copy.update = SupervisionUpdate::default();
            copy.rejudged = None;
            copy.rejected.clear();
            copy.newly_rejected = 0;
        }
        Self { captures, inputs, input_options, supervision, transforms, is_replay }
    }
}

fn supervision_copy<'view>(
    view: &'view ImageView<'_>,
    binding_index: u32,
) -> Option<&'view RefCell<SupervisionCopy>> {
    let index = usize::try_from(binding_index).ok()?;
    view.supervision.get(index)
}

/// The messages one step has staged, and the budget that admits them.
pub struct EmissionView<'a, 'b> {
    /// Messages in registration order. `None` is a dropped message: no loan was free.
    pub(super) emissions: &'a mut Vec<StagedMessage<Option<HyphaLoan>>>,
    /// The cycle's outputs.
    pub(super) outputs: &'a [CycleOutput],
    /// The output limits, owned and checked by `hypha_cycle`.
    pub(super) budget: &'a mut EmissionBudget<'b>,
    /// True between a reserve and its commit.
    pub(super) open: bool,
    pub(super) error: Option<Error>,
    /// The cycle's servers and clients, for the step's requests, outcomes and calls.
    pub(super) services: &'a mut crate::service::CycleServices,
}

impl<'a, 'b> EmissionView<'a, 'b> {
    /// The view of a step that has staged nothing yet.
    pub(super) fn new(
        emissions: &'a mut Vec<StagedMessage<Option<HyphaLoan>>>,
        outputs: &'a [CycleOutput],
        budget: &'a mut EmissionBudget<'b>,
        services: &'a mut crate::service::CycleServices,
    ) -> Self {
        Self { emissions, outputs, budget, open: false, error: None, services }
    }
}

/// Run `body` with the emission of a step that has services and no output: a step test's
/// (`hypha::testing::StepHarness`), whose outputs the SDK keeps itself. The service
/// functions of the C ABI take it as any step's emission.
pub fn with_service_emission<R>(
    services: &mut crate::service::CycleServices,
    body: impl FnOnce(&mut EmissionView<'_, '_>) -> R,
) -> R {
    let mut emissions = Vec::new();
    let mut budget = EmissionBudget::without_outputs();
    let mut view = EmissionView::new(&mut emissions, &[], &mut budget, services);
    body(&mut view)
}

/// The requests the step reads from its server `server` (`docs/design/native-services.md`
/// §2): on the first call of the step, those due; the same ones on a later call.
///
/// # Errors
/// `InvalidArgument` for an unknown server.
pub fn emission_requests<'view>(
    view: &'view mut EmissionView<'_, '_>,
    server: u32,
) -> Result<&'view mut [crate::service::StepRequest], Error> {
    view.services.requests(server)
}

/// The next outcome of its client `client` that the step reads; `None` when none is left.
///
/// # Errors
/// `InvalidArgument` for an unknown client.
pub fn emission_next_result(
    view: &mut EmissionView<'_, '_>,
    client: u32,
) -> Result<Option<crate::service::CallResult>, Error> {
    view.services.next_result(client)
}

/// The outcomes of its client `client` that successful steps did not read and dropped.
///
/// # Errors
/// `InvalidArgument` for an unknown client.
pub fn emission_unread_results(view: &mut EmissionView<'_, '_>, client: u32) -> Result<u64, Error> {
    view.services.unread_results(client)
}

/// Stage a call of its client `client` with `request`, sent after the step succeeds.
///
/// # Errors
/// `InvalidArgument` for an unknown client.
pub fn emission_call(
    view: &mut EmissionView<'_, '_>,
    client: u32,
    request: &[u8],
) -> Result<crate::service::CallStatus, Error> {
    view.services.call(client, request)
}

fn sample_view(sample: &hypha_cycle::HistorySample) -> HyphaCycleSampleView {
    HyphaCycleSampleView {
        payload: sample.payload.as_ptr(),
        payload_len: sample.payload.len(),
        transport_sent_nanos: sample.transport_sent_nanos,
        sequence: sample.sequence,
        publisher_id: sample.origin.publisher_id,
        publisher_sequence: sample.origin.sequence,
    }
}

/// Run `read` on the capture of binding `index` as the step sees it.
fn with_capture<T>(
    view: &ImageView<'_>,
    index: usize,
    read: impl FnOnce(&hypha_cycle::BindingCapture) -> T,
) -> Result<T, Error> {
    let capture = view.captures.get(index).ok_or(Error::InvalidArgument)?;
    let copy = match view.supervision.get(index) {
        Some(cell) => Some(cell.try_borrow().map_err(|_| Error::InvalidState)?),
        None => None,
    };
    let rejudged = copy.as_ref().and_then(|copy| copy.rejudged.as_ref());
    Ok(read(rejudged.unwrap_or(capture)))
}

/// Latch `status` as the step's first emission error and return it.
fn latch(view: &mut EmissionView<'_, '_>, error: Error) -> Error {
    view.error = Some(error);
    error
}

/// Sample `sample_index` of input `binding_index` in the step's image.
///
/// # Errors
/// `InvalidArgument` for an unknown binding; `NotAvailable` for an index past the
/// samples of the step.
pub fn image_sample(
    view: &ImageView<'_>,
    binding_index: u32,
    sample_index: usize,
) -> Result<HyphaCycleSampleView, Error> {
    let index = usize::try_from(binding_index).map_err(|_| Error::InvalidArgument)?;
    let capture = view.captures.get(index).ok_or(Error::InvalidArgument)?;
    let sample = capture.samples.get(sample_index).ok_or(Error::NotAvailable)?;
    Ok(sample_view(sample))
}

/// Reserves `len` bytes of output `output_index` for the next message of the step and
/// returns them, or `None` when the output's pool holds no free loan. An error latches:
/// every later reserve and commit of the step returns it.
///
/// # Errors
/// `InvalidState` for a reserve before the previous commit; a refusal of the step's
/// emission budget; `ResourceLimit` when the message list cannot grow; a loan error.
pub fn emission_reserve<'view>(
    view: &'view mut EmissionView<'_, '_>,
    output_index: u32,
    len: usize,
) -> Result<Option<&'view mut [u8]>, Error> {
    if let Some(error) = view.error {
        return Err(error);
    }
    if view.open {
        return Err(latch(view, Error::InvalidState));
    }
    if let Err(status) = view.budget.admit(output_index, len) {
        return Err(latch(view, Error::from_code(status)));
    }
    if view.emissions.try_reserve(1).is_err() {
        return Err(latch(view, Error::ResourceLimit));
    }
    // `admit` checked the index, so the output exists.
    let Some(output) = usize::try_from(output_index).ok().and_then(|i| view.outputs.get(i)) else {
        return Err(latch(view, Error::InvalidArgument));
    };
    match crate::loan::lend_frame(&output.publisher, len) {
        Ok(mut loan) => {
            if loan.payload_mut().is_none() {
                return Err(latch(view, Error::InvalidState));
            }
            view.emissions.push(StagedMessage { output: output_index, len, value: Some(loan) });
            view.open = true;
            let staged = view.emissions.last_mut().and_then(|message| message.value.as_mut());
            Ok(staged.and_then(HyphaLoan::payload_mut))
        }
        Err(Error::PoolExhausted | Error::LoanLimit) => {
            // No loan is free: the output's policy decides at dispatch.
            view.emissions.push(StagedMessage { output: output_index, len, value: None });
            Ok(None)
        }
        Err(error) => Err(latch(view, error)),
    }
}

/// Closes the message the last [`emission_reserve`] opened.
///
/// # Errors
/// The latched error of the step; `InvalidState` with no open message.
pub fn emission_commit(view: &mut EmissionView<'_, '_>) -> Result<(), Error> {
    if let Some(error) = view.error {
        return Err(error);
    }
    if !view.open {
        return Err(latch(view, Error::InvalidState));
    }
    view.open = false;
    Ok(())
}

/// The sample input `binding_index` holds in the step: the newest one before the cut,
/// after any rejection of the step.
///
/// # Errors
/// `InvalidArgument` for an unknown binding; `NotAvailable` when it holds none;
/// `InvalidState` while its supervision is borrowed.
pub fn image_held(view: &ImageView<'_>, binding_index: u32) -> Result<HyphaCycleSampleView, Error> {
    let index = usize::try_from(binding_index).map_err(|_| Error::InvalidArgument)?;
    let held = with_capture(view, index, |capture| capture.held().map(|s| sample_view(s)))?;
    held.ok_or(Error::NotAvailable)
}

/// Reports the application's liveness `value` of input `binding_index` for the step.
///
/// # Errors
/// `InvalidArgument` for an unknown binding; `InvalidState` while it is borrowed.
pub fn image_alive(view: &ImageView<'_>, binding_index: u32, value: u64) -> Result<(), Error> {
    let cell = supervision_copy(view, binding_index).ok_or(Error::InvalidArgument)?;
    let mut copy = cell.try_borrow_mut().map_err(|_| Error::InvalidState)?;
    copy.update.alive = Some(value);
    if let Some(state) = copy.state.as_mut() {
        state.on_alive(value);
    }
    Ok(())
}

/// Makes the step ignore (or no longer ignore) the supervision of input `binding_index`.
///
/// # Errors
/// `InvalidArgument` for an unknown binding; `InvalidState` while it is borrowed.
pub fn image_set_ignored(
    view: &ImageView<'_>,
    binding_index: u32,
    ignored: bool,
) -> Result<(), Error> {
    let cell = supervision_copy(view, binding_index).ok_or(Error::InvalidArgument)?;
    let mut copy = cell.try_borrow_mut().map_err(|_| Error::InvalidState)?;
    copy.update.ignored = Some(ignored);
    if let Some(state) = copy.state.as_mut() {
        state.set_ignored(ignored);
    }
    Ok(())
}

/// The view of input `binding_index` in the step, after any rejection of the step.
///
/// # Errors
/// `InvalidArgument` for an unknown binding; `InvalidState` while it is borrowed.
pub fn image_binding(
    view: &ImageView<'_>,
    binding_index: u32,
) -> Result<HyphaCycleBindingView, Error> {
    let index = usize::try_from(binding_index).map_err(|_| Error::InvalidArgument)?;
    image_binding_view(view, index)
}

/// The view of binding `index` in this step.
fn image_binding_view(view: &ImageView<'_>, index: usize) -> Result<HyphaCycleBindingView, Error> {
    let capture = view.captures.get(index).ok_or(Error::InvalidArgument)?;
    let input = view.inputs.get(index).map(Arc::as_ref);
    let copy = match view.supervision.get(index) {
        Some(cell) => Some(cell.try_borrow().map_err(|_| Error::InvalidState)?),
        None => None,
    };
    let supervision = copy.as_ref().and_then(|copy| copy.state.as_ref());
    let judged = copy.as_ref().and_then(|copy| copy.rejudged.as_ref()).unwrap_or(capture);
    let options = view.input_options.get(index);
    Ok(super::tick::binding_view(judged, input, options, view.is_replay, supervision))
}

/// Rejects the sample `sequence` of input `binding_index` for this and every later
/// step, and returns the binding's view after the rejection.
///
/// # Errors
/// `InvalidArgument` for an unknown binding; `NotAvailable` for a sequence the step
/// does not hold, or an unwired input; `InvalidState` while it is borrowed.
pub fn image_reject_sample(
    view: &ImageView<'_>,
    binding_index: u32,
    sequence: u64,
) -> Result<HyphaCycleBindingView, Error> {
    let index = usize::try_from(binding_index).map_err(|_| Error::InvalidArgument)?;
    let capture = view.captures.get(index).ok_or(Error::InvalidArgument)?;
    let in_samples = capture.samples.iter().any(|sample| sample.sequence == sequence);
    // After an earlier rejection the step's capture holds the sample held before.
    let is_held = with_capture(view, index, |judged| {
        judged.held().is_some_and(|held| held.sequence == sequence)
    })?;
    let selected = in_samples || is_held;
    if !selected {
        return Err(Error::NotAvailable);
    }
    let input = view.inputs.get(index).ok_or(Error::InvalidArgument)?;
    let history = input.history.as_ref().ok_or(Error::NotAvailable)?;
    let cell = view.supervision.get(index).ok_or(Error::InvalidArgument)?;
    let mut copy = cell.try_borrow_mut().map_err(|_| Error::InvalidState)?;
    let history = crate::locked(history);
    let newly = history_reject(history, sequence, capture, &mut copy);
    if newly {
        copy.newly_rejected = copy.newly_rejected.wrapping_add(1);
    }
    drop(copy);
    image_binding_view(view, index)
}

/// Mark `sequence` in `history` and re-judge the step's copy of `capture` without it.
/// Returns whether the source had not rejected the sample before.
fn history_reject(
    mut history: std::sync::MutexGuard<'_, hypha_cycle::CycleHistory>,
    sequence: u64,
    capture: &hypha_cycle::BindingCapture,
    copy: &mut SupervisionCopy,
) -> bool {
    let newly = history.reject(sequence);
    let mut rejudged = copy.rejudged.take().unwrap_or_else(|| capture.clone());
    rejudged.reject(&history, sequence);
    drop(history);
    if let Some(state) = copy.state.as_mut() {
        state.set_stale(rejudged.stale());
    }
    copy.rejudged = Some(rejudged);
    copy.rejected.push(sequence);
    newly
}

pub(super) fn binding_report(capture: &hypha_cycle::BindingCapture) -> HyphaCycleBindingReport {
    let verdict = match capture.verdict {
        hypha_cycle::InputVerdict::NeverArrived => HYPHA_CYCLE_VERDICT_NEVER_ARRIVED,
        hypha_cycle::InputVerdict::Usable => HYPHA_CYCLE_VERDICT_USABLE,
        hypha_cycle::InputVerdict::Stale => HYPHA_CYCLE_VERDICT_STALE,
    };
    HyphaCycleBindingReport {
        verdict,
        sample_count: capture.samples.len(),
        backlog_samples: capture.backlog_samples,
        work_limit_drops: capture.work_limit_drops,
        lost_since_capture: capture.lost_since_capture,
        lost_total: capture.counters.overrun_total,
        stale_since_capture: capture.stale_since_capture,
        stale_total: capture.counters.age_rejections_total,
    }
}
