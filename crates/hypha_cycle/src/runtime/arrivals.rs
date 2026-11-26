//! The cycle-wide queued-arrival budget.

use super::helpers::rotate_index;
use super::{CaptureClock, CycleRuntime};
use crate::CaptureMode;

const STATUS_INVALID_STATE: i32 = 20;
const STATUS_RESOURCE_LIMIT: i32 = 13;

impl CycleRuntime {
    /// Fill `self.assignments` with each binding's share of the arrival budget.
    pub(super) fn arrival_assignments(&mut self, clock: CaptureClock) -> Result<(), i32> {
        self.assignments.clear();
        let Some(mut remaining) = self.arrival_budget else {
            // Without a shared budget, capture applies each binding's limit directly.
            self.assignments.extend(self.bindings.iter().map(|slot| {
                if slot.options.mode == CaptureMode::Queued {
                    slot.options.max_samples_per_iteration
                } else {
                    0
                }
            }));
            return Ok(());
        };
        let binding_count = self.bindings.len();
        self.capacities.clear();
        for (index, (slot, capture)) in self.bindings.iter().zip(&self.captures).enumerate() {
            let capacity = match (&slot.history, slot.options.mode) {
                // The trigger and exempt bindings take their samples outside the budget.
                _ if self.triggers.contains(&index) || slot.exempt => 0,
                (Some(history), CaptureMode::Queued) => {
                    let history = crate::locked(history);
                    capture
                        .pending_samples_on(&history, clock)
                        .min(slot.options.max_samples_per_iteration)
                }
                _ => 0,
            };
            self.capacities.push(capacity);
        }

        self.assignments.resize(binding_count, 0);
        if binding_count == 0 {
            return Ok(());
        }

        let start = self.arrival_rotation;
        while remaining > 0 {
            self.active.clear();
            for offset in 0..binding_count {
                let index = rotate_index(start, offset, binding_count);
                let capacity = self.capacities.get(index).copied().unwrap_or(0);
                if capacity > self.assignments.get(index).copied().unwrap_or(0) {
                    self.active.push(index);
                }
            }
            if self.active.is_empty() {
                break;
            }
            let Some(share) = remaining.checked_div(self.active.len()) else {
                return Err(STATUS_INVALID_STATE);
            };
            if share == 0 {
                for &index in &self.active {
                    if remaining == 0 {
                        break;
                    }
                    if let Some(assigned) = self.assignments.get_mut(index) {
                        let Some(next) = assigned.checked_add(1) else {
                            return Err(STATUS_RESOURCE_LIMIT);
                        };
                        *assigned = next;
                        remaining = remaining.saturating_sub(1);
                    }
                }
                break;
            }
            let mut allocated = 0usize;
            for &index in &self.active {
                let Some(capacity) = self.capacities.get(index).copied() else {
                    continue;
                };
                let Some(assigned) = self.assignments.get_mut(index) else {
                    continue;
                };
                let available = capacity.saturating_sub(*assigned);
                let amount = available.min(share);
                let Some(next_assigned) = assigned.checked_add(amount) else {
                    return Err(STATUS_RESOURCE_LIMIT);
                };
                let Some(next_allocated) = allocated.checked_add(amount) else {
                    return Err(STATUS_RESOURCE_LIMIT);
                };
                *assigned = next_assigned;
                allocated = next_allocated;
            }
            if allocated == 0 {
                break;
            }
            remaining = remaining.saturating_sub(allocated);
        }
        for (slot, assigned) in self.bindings.iter().zip(&mut self.assignments) {
            if slot.exempt {
                *assigned = slot.options.max_samples_per_iteration;
            }
        }
        self.arrival_rotation =
            start.checked_add(1).filter(|next| *next < binding_count).unwrap_or(0);
        Ok(())
    }
}
