//! The replay accounting of evicted samples: which evictions a later selection supersedes
//! and which are losses (`docs/replay-flow-control.md`).

use super::{BindingCapture, CaptureClock, CycleHistory, before, before_or_equal};

impl BindingCapture {
    /// Move the cursor up to `oldest` past evicted samples, and count them as lost.
    pub(super) fn skip_evicted(&mut self, oldest: u64) {
        self.count_evicted_from(self.cursor, oldest);
    }

    /// Count the evicted samples from `from` up to `oldest` as lost, and move the cursor
    /// up to `oldest`.
    pub(super) fn count_evicted_from(&mut self, from: u64, oldest: u64) {
        if before(from, oldest) {
            let lost = oldest.wrapping_sub(from);
            self.lost_since_capture = self.lost_since_capture.wrapping_add(lost);
            self.counters.overrun_total = self.counters.overrun_total.wrapping_add(lost);
        }
        if before(self.cursor, oldest) {
            self.cursor = oldest;
        }
    }

    /// The replay loss rule of a latest binding, at a capture that selects `selected`,
    /// with `oldest` the oldest retained sequence.
    ///
    /// Invariant: a sample the binding passed over without holding it, evicted or not, is
    /// uncounted only while a newer sample that no reader rejected is retained for it or
    /// held by it. Every later cut admits that sample, so every later step takes it or a
    /// newer one, and the passed sample is superseded in data time. The passed samples run
    /// from `passed_from` up to the newest selection, the sample before the cursor, which
    /// never moves back:
    /// - a newer selection supersedes them as well, and the run extends to it; an older
    ///   one, a fallback past a rejected sample, supersedes none of them;
    /// - the newest selection evicted with no rejection is a value for good, so they are
    ///   superseded for good, and the run ends;
    /// - a capture that selects nothing has no such retained sample, since it would
    ///   select it: every one of them that is evicted counts, with those evicted unread
    ///   since, from past the sample the binding holds for good. One of them may be the
    ///   value this step or a later one is due, and the capture knows no later deadline.
    ///
    /// A sample that may have been rejected before its eviction counts as rejected.
    pub(super) fn account_evictions(
        &mut self,
        history: &CycleHistory,
        oldest: u64,
        selected: Option<u64>,
        clock: CaptureClock,
    ) {
        let newest_selected = self.cursor.wrapping_sub(1);
        let superseded_for_good =
            before(newest_selected, oldest) && !may_have_been_rejected(history, newest_selected);
        if superseded_for_good {
            self.passed_from = None;
        }
        if let Some(selected) = selected {
            if self.passed_from.is_none() && before(self.cursor, selected) {
                self.passed_from = Some(self.cursor);
            }
        } else {
            let passed = self.passed_from.take().unwrap_or(self.cursor);
            let after_held =
                self.held_for_good(history, oldest, clock).map(|held| held.wrapping_add(1));
            let from = after_held.filter(|after| before(passed, *after)).unwrap_or(passed);
            self.count_evicted_from(from, oldest);
        }
    }

    /// The newest sample this binding holds, or held before, that was evicted with no
    /// rejection and that the replay cut of this capture, `clock`, admits, with `oldest`
    /// the oldest retained sequence: every later cut admits it too, so no step takes a
    /// sample older than it again. A host-clock capture may have held a sample stamped
    /// ahead of every cut so far, which is no such value.
    pub(super) fn held_for_good(
        &self,
        history: &CycleHistory,
        oldest: u64,
        clock: CaptureClock,
    ) -> Option<u64> {
        [self.held.as_ref(), self.previous_held.as_ref()]
            .into_iter()
            .flatten()
            .filter(|sample| clock.admits(sample))
            .map(|sample| sample.sequence)
            .find(|sequence| {
                before(*sequence, oldest) && !may_have_been_rejected(history, *sequence)
            })
    }
}

/// Whether a reader rejected the sample `sequence`, or may have before it was evicted:
/// eviction forgets a rejection, so any rejected sample evicted at or after it counts.
fn may_have_been_rejected(history: &CycleHistory, sequence: u64) -> bool {
    history.is_rejected(sequence)
        || history.newest_evicted_rejected().is_some_and(|newest| before_or_equal(sequence, newest))
}
