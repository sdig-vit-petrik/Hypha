//! A step's reset of an input (`clear()`, #213): what a capture forgets, and how the
//! next captures judge only what it kept.

use std::sync::Arc;

use super::{BindingCapture, CaptureClock, CaptureMode, CycleHistory, HistorySample, before};

/// What one capture could take: the samples below `high_water` that `clock` admits and,
/// with `sent_through`, those within [`BindingCapture::reach`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct LastTake {
    pub(super) clock: CaptureClock,
    pub(super) sent_through: Option<u64>,
    pub(super) high_water: u64,
}

/// The samples one clear forgot: those before `through` that `clock` admits, or every one
/// before it without a clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Cleared {
    pub(super) through: u64,
    pub(super) clock: Option<CaptureClock>,
    /// The newest sample the binding had before the clear: its silence counts on.
    pub(super) newest: Option<Arc<HistorySample>>,
}

impl BindingCapture {
    /// The newest retained sample `clock` admits that no reader rejected and no clear
    /// forgot.
    pub(super) fn newest_judged<'h>(
        &self,
        history: &'h CycleHistory,
        clock: CaptureClock,
    ) -> Option<&'h Arc<HistorySample>> {
        let end = clock.admitted_end(history).unwrap_or(history.samples().len());
        history.samples().range(..end).rev().find(|sample| {
            clock.admits(sample) && !history.is_rejected(sample.sequence) && !self.forgets(sample)
        })
    }

    /// Whether the last clear forgot `sample`.
    pub(super) fn forgets(&self, sample: &HistorySample) -> bool {
        self.cleared.as_ref().is_some_and(|cleared| {
            before(sample.sequence, cleared.through)
                && cleared.clock.is_none_or(|clock| clock.admits(sample))
        })
    }

    /// Drop the record of the last clear once no forgotten sample is retained and a later
    /// one was admitted (`admitted_since`).
    pub(super) fn end_spent_clear(&mut self, history: &CycleHistory, admitted_since: bool) {
        let spent = self.cleared.as_ref().is_some_and(|cleared| {
            let oldest =
                history.samples().front().map_or(history.high_water_next(), |s| s.sequence);
            admitted_since && !before(oldest, cleared.through)
        });
        if spent {
            self.cleared = None;
        }
    }

    /// Forget, after the step of the last capture, what that capture could take: the held
    /// sample, and the samples it admitted that are still pending. The cursor moves past
    /// them as a capture with no limit would, and they are counted in `cleared_total`; the
    /// count is returned. A sample the capture did not admit (sent at or after the cut of a
    /// replay step, after a host trigger sample, or not yet in the history) stays, so a
    /// replay forgets exactly what was pending at the step's time.
    ///
    /// Later captures judge only the samples the clear did not forget: until one is
    /// admitted the verdict is `NeverArrived` and no value is held, and the silence counts
    /// from the newest sample before the clear (`Stale` past the limit). The received and
    /// delivered counts and the supervision state are not reset.
    pub fn clear(&mut self, history: Option<&CycleHistory>) -> u64 {
        let newest = self.last_take.and_then(|take| {
            history
                .and_then(|history| self.newest_judged(history, take.clock))
                .or(self.held.as_ref())
                .map(Arc::clone)
                .or_else(|| self.cleared.as_ref().and_then(|cleared| cleared.newest.clone()))
        });
        self.held = None;
        self.previous_held = None;
        self.passed_from = None;
        let (Some(take), Some(history)) = (self.last_take, history) else {
            return 0;
        };
        let dropped = self.skip_admitted(history, take);
        self.counters.cleared_total = self.counters.cleared_total.wrapping_add(dropped);
        let (through, clock) = match self.mode {
            CaptureMode::Queued => (self.cursor, None),
            CaptureMode::Latest => (take.high_water, Some(take.clock)),
        };
        self.cleared = Some(Cleared { through, clock, newest });
        dropped
    }

    /// Move the cursor past the pending samples `take` admitted and return how many of
    /// them no reader rejected. A queued binding first reports the samples the history
    /// evicted before it read them, as a capture would.
    pub(super) fn skip_admitted(&mut self, history: &CycleHistory, take: LastTake) -> u64 {
        let Some(oldest) = history.samples().front().map(|sample| sample.sequence) else {
            return 0;
        };
        if self.mode == CaptureMode::Queued && before(self.cursor, oldest) {
            let lost = oldest.wrapping_sub(self.cursor);
            self.carried_lost = self.carried_lost.wrapping_add(lost);
            self.counters.overrun_total = self.counters.overrun_total.wrapping_add(lost);
            self.cursor = oldest;
        }
        let start = history.cursor_offset(self.cursor);
        let end =
            self.reach(history, take.clock, take.sent_through).unwrap_or(history.samples().len());
        let mut dropped = 0_u64;
        for sample in history.samples().range(start..end.max(start)) {
            let admitted = before(sample.sequence, take.high_water) && take.clock.admits(sample);
            if !admitted {
                break;
            }
            self.cursor = sample.sequence.wrapping_add(1);
            if !history.is_rejected(sample.sequence) {
                dropped = dropped.wrapping_add(1);
            }
        }
        if self.mode == CaptureMode::Latest {
            return 0;
        }
        dropped
    }
}
