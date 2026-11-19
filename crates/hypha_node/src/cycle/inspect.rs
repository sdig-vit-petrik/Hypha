//! Read-only cycle inspection: the health and the binding views of the last finished step.
//!
//! Both read the snapshots of a cycle alone, so any thread may call them while the
//! `WaitSet` runs.

use super::state::CycleSnapshots;
use super::types::{HyphaCycleBindingView, HyphaCycleHealth};
use crate::status::Error;

/// The health counters of the cycle of `snapshots` now.
#[must_use]
pub fn cycle_health(snapshots: &CycleSnapshots) -> HyphaCycleHealth {
    *snapshots.health.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The view of input `binding_index` of the cycle of `snapshots` after its last step.
///
/// # Errors
/// `InvalidArgument` for an unknown binding.
pub fn cycle_binding(
    snapshots: &CycleSnapshots,
    binding_index: u32,
) -> Result<HyphaCycleBindingView, Error> {
    let index = usize::try_from(binding_index).map_err(|_| Error::InvalidArgument)?;
    let views = snapshots.bindings.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    views.get(index).copied().ok_or(Error::InvalidArgument)
}
