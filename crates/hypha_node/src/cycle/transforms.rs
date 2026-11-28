//! The TF input of a cycle: two queued bindings on `/tf` and `/tf_static`, ingested
//! into a cycle-owned [`TfState`] before each step, and the step's lookup. A replay step
//! sees the transforms sent strictly before its deadline.

use std::time::Duration;

use hypha_cycle::BindingCapture;

use super::image::ImageView;
use super::registration::Cycle;
use super::source::HyphaInputSource;
use super::state::CycleId;
use super::types::{HyphaCycleBindingOptions, HyphaCycleInputMode};
use crate::status::Error;
use crate::transforms::{IngestPass, TfOwner, TfState, TransformLookup};
use crate::waitset::HyphaWaitSet;

/// One TF input of a cycle.
pub(crate) struct TfInput {
    /// The binding indices of the `/tf` and the `/tf_static` source.
    dynamic: usize,
    fixed: usize,
    state: TfState,
    /// When the last warning of dropped samples outside the lockstep was written.
    drops_warned: Option<std::time::Instant>,
    /// The step asked to forget the dynamic transforms when it returns.
    clear_requested: std::cell::Cell<bool>,
}

/// The shortest interval between two warnings of dropped TF samples.
const DROP_WARNING_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

impl TfInput {
    /// Insert every transform the step captured on both bindings. A sample that does not
    /// decode fails the step with `InvalidFrame`. A replay step (`losses` is `Some`) that
    /// had to drop samples sent before its time fails with `SequenceGap` in the lockstep,
    /// and warns of them outside it.
    pub(super) fn ingest(
        &mut self,
        cycle: &str,
        captures: &[BindingCapture],
        losses: Option<super::tick::Losses>,
    ) -> Result<(), Error> {
        let owner = TfOwner::Cycle(cycle);
        let mut pass = IngestPass::default();
        for (index, is_static) in [(self.fixed, true), (self.dynamic, false)] {
            let Some(capture) = captures.get(index) else {
                return Err(Error::InvalidState);
            };
            let topic = if is_static { "/tf_static" } else { "/tf" };
            let (drops, pending) = (capture.work_limit_drops, capture.samples.len());
            match losses {
                Some(super::tick::Losses::Fail) if drops > 0 => {
                    log::error!(target: "hypha", "hypha: cycle '{cycle}' TF input dropped {drops} {topic} sample(s) sent before the step: more than {pending} are pending");
                    return Err(Error::SequenceGap);
                }
                Some(super::tick::Losses::Warn) if drops > 0 && self.drop_warning_due() => {
                    log::warn!(target: "hypha", "hypha: cycle '{cycle}' TF input dropped {drops} {topic} sample(s) sent before the step: more than {pending} are pending; outside the replay lockstep the run goes on");
                }
                _ => {}
            }
            for sample in &capture.samples {
                self.state.ingest(owner, &sample.payload, is_static, &mut pass)?;
            }
        }
        self.state.finish(owner, pass);
        Ok(())
    }

    /// Forget the dynamic transforms when the step returns ([`Self::apply_clear`]).
    pub(super) fn request_clear(&self) {
        self.clear_requested.set(true);
    }

    /// Forget the dynamic transforms if the finished step of cycle `cycle` asked to, and
    /// log it once; the static transforms stay.
    pub(super) fn apply_clear(&mut self, cycle: &str) {
        if !self.clear_requested.replace(false) {
            return;
        }
        let frames = self.state.clear_dynamic();
        log::info!(target: "hypha", "hypha: cycle '{cycle}' TF input cleared: {frames} dynamic frame(s) forgotten, static transforms kept");
    }

    /// True when no warning of dropped samples went out in the last interval; starts one.
    fn drop_warning_due(&mut self) -> bool {
        let now = std::time::Instant::now();
        let due =
            self.drops_warned.is_none_or(|last| now.duration_since(last) >= DROP_WARNING_INTERVAL);
        if due {
            self.drops_warned = Some(now);
        }
        due
    }
}

/// Binds a TF input over the two sources `tf` and `tf_static` of the uncommitted cycle
/// `cycle` of `set` with `cache_nanos` of history per frame; returns the TF input's index.
///
/// # Errors
/// `InvalidArgument` for a zero cache, a cycle `set` does not hold, the same source
/// twice, an unwired or foreign source; `InvalidState` for a committed cycle or a running `WaitSet`; a refusal of
/// the input binds.
pub fn cycle_bind_transforms(
    set: &HyphaWaitSet,
    cycle: CycleId,
    tf: &HyphaInputSource,
    tf_static: &HyphaInputSource,
    cache_nanos: u64,
) -> Result<u32, Error> {
    let sources = [tf, tf_static];
    if cache_nanos == 0 {
        return Err(Error::InvalidArgument);
    }
    let cycle = set.cycle(cycle)?;
    let cycle: &Cycle = &cycle;
    let index = u32::try_from(cycle.configurable(set)?.transforms.len())
        .map_err(|_| Error::ResourceLimit)?;
    if std::sync::Arc::ptr_eq(&tf.inner, &tf_static.inner) {
        return Err(Error::InvalidArgument);
    }
    let mut depths = [0_usize; 2];
    for (source, depth) in sources.iter().zip(&mut depths) {
        let inner = &source.inner;
        let Some(subscriber) = inner.subscriber.as_deref() else {
            return Err(Error::InvalidArgument);
        };
        let usable = Cycle::owns_source(set, inner) && inner.history.is_some();
        if !usable {
            return Err(Error::InvalidArgument);
        }
        *depth = subscriber.port.buffer_size().max(1);
    }
    // Only the cycle's own binding limits can refuse the second binding.
    let mut bindings = [0_usize; 2];
    for ((source, depth), binding) in sources.into_iter().zip(depths).zip(&mut bindings) {
        let options = HyphaCycleBindingOptions {
            mode: HyphaCycleInputMode::Queued as u32,
            max_samples_per_iteration: depth,
            optional: 1,
        };
        let bound = super::registration::bind_input(set, cycle, source, &options)?;
        *binding = usize::try_from(bound).map_err(|_| Error::InvalidState)?;
    }
    let mut registration = cycle.step_mut()?;
    for binding in bindings {
        registration.runtime.exempt_from_budget(binding).map_err(Error::from_code)?;
    }
    let [dynamic, fixed] = bindings;
    registration.transforms.push(TfInput {
        dynamic,
        fixed,
        state: TfState::new(
            Duration::from_nanos(cache_nanos),
            std::sync::Arc::clone(&set.node().health),
        ),
        drops_warned: None,
        clear_requested: std::cell::Cell::new(false),
    });
    Ok(index)
}

/// Looks up, in the step's TF input `index`, `source` in `target` at `at` nanoseconds,
/// or at the latest time common to the chain for `None`.
///
/// # Errors
/// `InvalidArgument` for an unknown index; `InvalidState` for an overflow composing
/// the chain.
pub fn image_lookup_transform(
    view: &ImageView<'_>,
    index: u32,
    target: &str,
    source: &str,
    at: Option<i64>,
) -> Result<TransformLookup, Error> {
    let input = usize::try_from(index)
        .ok()
        .and_then(|i| view.transforms.get(i))
        .ok_or(Error::InvalidArgument)?;
    input.state.lookup(target, source, at)
}
