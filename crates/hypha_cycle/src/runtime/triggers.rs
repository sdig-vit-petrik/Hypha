//! The trigger ports of a cycle's runtime: which bindings step a triggered cycle, their
//! pending stamps for the schedule, and the samples the schedule passes without a step.

use super::helpers::shares_history;
use super::{CycleRuntime, STATUS_INVALID_ARGUMENT, STATUS_RESOURCE_LIMIT};
use crate::{Passed, TriggerPorts};

impl CycleRuntime {
    /// Add binding `index` to the triggers of a triggered cycle; a repeat adds nothing.
    /// Each step takes the samples its [`CycleTime::trigger_step`] names, outside the
    /// binding's limits and the arrival budget.
    ///
    /// # Errors
    /// Returns `STATUS_INVALID_ARGUMENT` unless the binding exists, is queued and has a
    /// source, and no other trigger reads its history, where each sample would step
    /// twice; `STATUS_RESOURCE_LIMIT` past [`TriggerPorts::CAPACITY`] triggers.
    pub fn set_trigger(&mut self, index: usize) -> Result<(), i32> {
        let slot = self.bindings.get(index).ok_or(STATUS_INVALID_ARGUMENT)?;
        if slot.options.mode != crate::CaptureMode::Queued || slot.history.is_none() {
            return Err(STATUS_INVALID_ARGUMENT);
        }
        let Err(position) = self.triggers.binary_search(&index) else {
            return Ok(());
        };
        let shared = self
            .triggers
            .iter()
            .filter_map(|&trigger| self.bindings.get(trigger))
            .any(|trigger| shares_history(trigger, slot));
        if shared {
            return Err(STATUS_INVALID_ARGUMENT);
        }
        if self.triggers.len() >= TriggerPorts::CAPACITY {
            return Err(STATUS_RESOURCE_LIMIT);
        }
        self.triggers.insert(position, index);
        if let Some(capture) = self.captures.get_mut(index) {
            capture.set_trigger();
        }
        Ok(())
    }

    /// The trigger bindings, in binding order; empty for a rate cycle. Trigger port `p`
    /// is binding `triggers()[p]`.
    #[must_use]
    pub fn triggers(&self) -> &[usize] {
        &self.triggers
    }

    /// Replace `out` with one list per trigger port: the framework send stamps of the
    /// samples it has not taken, in history order. Empty for a rate cycle.
    ///
    /// # Errors
    /// Returns `STATUS_INVALID_ARGUMENT` for a stamp beyond `i64::MAX`.
    pub fn pending_trigger_stamps(&self, out: &mut Vec<Vec<i64>>) -> Result<(), i32> {
        out.resize_with(self.triggers.len(), Vec::new);
        for (&index, stamps) in self.triggers.iter().zip(out.iter_mut()) {
            stamps.clear();
            let history = self.bindings.get(index).and_then(|slot| slot.history.as_ref());
            let (Some(history), Some(capture)) = (history, self.captures.get(index)) else {
                return Err(STATUS_INVALID_ARGUMENT);
            };
            let history = crate::locked(history);
            for stamp in capture.pending_stamps(&history) {
                stamps.push(i64::try_from(stamp).map_err(|_| STATUS_INVALID_ARGUMENT)?);
            }
        }
        Ok(())
    }

    /// Start the silence count of every trigger port at `clock_nanos` on its capture
    /// clock, unless a capture started it: the first evaluation of a triggered cycle.
    pub fn start_trigger_silence(&mut self, clock_nanos: u64) {
        for &index in &self.triggers {
            if let Some(capture) = self.captures.get_mut(index) {
                capture.start_at(clock_nanos);
            }
        }
    }

    /// Consume the oldest pending sample of trigger port `port` without a step, counted as
    /// `why` says on that port.
    ///
    /// # Errors
    ///
    /// Returns `STATUS_INVALID_ARGUMENT` without such a port or a pending sample.
    pub fn pass_trigger(&mut self, port: usize, why: Passed) -> Result<(), i32> {
        let index = *self.triggers.get(port).ok_or(STATUS_INVALID_ARGUMENT)?;
        let history = self.bindings.get(index).and_then(|slot| slot.history.as_ref());
        let (Some(history), Some(capture)) = (history, self.captures.get_mut(index)) else {
            return Err(STATUS_INVALID_ARGUMENT);
        };
        let history = crate::locked(history);
        if capture.pass_pending(&history, why) { Ok(()) } else { Err(STATUS_INVALID_ARGUMENT) }
    }
}
