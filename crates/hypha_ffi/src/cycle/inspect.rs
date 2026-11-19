//! The C ABI wrappers of `hypha_node::cycle::inspect`.
//! Any thread may call them while the `WaitSet` runs.

use super::{HyphaCycle, snapshots_of};
use crate::boundary::write_out;
use crate::status::try_guard;
use hypha_node::safe::{cycle_binding, cycle_health};
use hypha_node::status::HyphaStatus;
use hypha_node::{HyphaCycleBindingView, HyphaCycleHealth};

/// Read cumulative cycle health as of the last finished step. Counts wrap.
///
/// # Safety
/// `cycle` and `out` must be live pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_cycle_health(
    cycle: *const HyphaCycle,
    out: *mut HyphaCycleHealth,
) -> HyphaStatus {
    try_guard(|| {
        let snapshots = unsafe { snapshots_of(cycle) }?;
        unsafe { write_out(out, cycle_health(snapshots)) }
    })
}

/// Fill `out` with the health, held sample and supervision of binding `binding_index`
/// as of the last finished step; zero before the first step. `InvalidArgument` for a
/// null pointer or an unknown binding.
///
/// # Safety
/// `cycle` and `out` must be live pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_cycle_binding(
    cycle: *const HyphaCycle,
    binding_index: u32,
    out: *mut HyphaCycleBindingView,
) -> HyphaStatus {
    try_guard(|| {
        let snapshots = unsafe { snapshots_of(cycle) }?;
        let snapshot = cycle_binding(snapshots, binding_index)?;
        unsafe { write_out(out, snapshot) }
    })
}
