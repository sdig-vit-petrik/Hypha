//! The C ABI wrappers of `hypha_node::cycle::registration`.

use super::{CycleTarget, HyphaCycle};
use crate::boundary::{borrow_str, handle_ref, read_in, require_non_null, take_box, write_out};
use crate::callbacks::CStep;
use crate::status::try_guard;
use crate::waitset::access::waitset_here;
use hypha_node::safe::{
    HyphaInputSource, HyphaPublisher, HyphaWaitSet, cycle_bind_input, cycle_bind_output,
    cycle_commit, cycle_create, cycle_destroy, cycle_set_class_based, cycle_set_host_clock,
    cycle_set_trigger,
};
use hypha_node::status::HyphaStatus;
use hypha_node::{HyphaContextRelease, HyphaCycleBindingOptions, HyphaCycleStep};
use std::os::raw::{c_char, c_void};

/// Create an uncommitted cycle registration, owned by `waitset`.
///
/// `*out` is borrowed: the `WaitSet` destroys the cycle, its callback context and handle
/// when it is destroyed; `hypha_cycle_destroy` releases it earlier. A refused create
/// leaves `context` to the caller. `period_nanos` is at most `i64::MAX`; zero makes a
/// triggered cycle, which needs `hypha_cycle_set_trigger` before `hypha_cycle_commit`.
/// `InvalidArgument`: a name another cycle of the `WaitSet` holds. `InvalidState`: a
/// running `WaitSet`.
///
/// # Safety
/// `waitset`, name bytes, callback context and `out` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_cycle_create(
    waitset: *mut HyphaWaitSet,
    name: *const c_char,
    name_len: usize,
    period_nanos: u64,
    callback: HyphaCycleStep,
    context: *mut c_void,
    release: HyphaContextRelease,
    out: *mut *mut HyphaCycle,
) -> HyphaStatus {
    try_guard(|| {
        require_non_null(waitset)?;
        if callback.is_none() {
            return Err(HyphaStatus::InvalidArgument);
        }
        let name = unsafe { borrow_str(name, name_len) }?;
        unsafe { write_out(out, std::ptr::null_mut()) }?;
        // Mirror `cycle_create`: clear the last error and refuse arguments before the run check.
        hypha_node::last_error::clear();
        let refused_argument = i64::try_from(period_nanos).is_err() || name.is_empty();
        if refused_argument {
            return Err(HyphaStatus::InvalidArgument);
        }
        let set = unsafe { waitset_here(waitset) }?;
        let mut handle = std::ptr::null_mut();
        let adopt = CStep::adopter(callback, context, release, waitset, &mut handle);
        cycle_create(&set, name, period_nanos, adopt)?;
        unsafe { write_out(out, handle) }
    })
}

/// Bind one source to an uncommitted cycle with `options`, read for the call, and write
/// its input index to `out_index` (from zero in declaration order, apart from outputs).
/// The cycle keeps the source's subscriber and history until it is destroyed; the caller
/// keeps and destroys the handle. `InvalidArgument`: a source of another `WaitSet` or an
/// unknown mode. `InvalidState`: a committed cycle or a running `WaitSet`.
///
/// # Safety
/// `cycle`, `source`, `options`, and `out_index` must be live pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_cycle_bind_input(
    cycle: *mut HyphaCycle,
    source: *mut HyphaInputSource,
    options: *const HyphaCycleBindingOptions,
    out_index: *mut u32,
) -> HyphaStatus {
    try_guard(|| {
        let cycle = unsafe { CycleTarget::of(cycle) }?;
        let input = unsafe { handle_ref(source) }?;
        let selected = unsafe { read_in(options) }?;
        unsafe { write_out(out_index, 0) }?;
        let (set, cycle) = unsafe { cycle.waitset() }?;
        let index = cycle_bind_input(&set, cycle, input, &selected)?;
        unsafe { write_out(out_index, index) }
    })
}

/// Bind and take exclusive ownership of one publisher, as an output that sends at most
/// `max_messages_per_step` messages a step under `policy` (`HyphaCycleOutputFailure`).
/// `out_index` counts outputs from zero, apart from inputs. On `Ok` the cycle owns
/// `publisher` until `hypha_cycle_destroy`: the caller must not destroy it, but may read
/// it with `hypha_publisher_subscriber_count`. A refused bind leaves it to the caller.
/// `InvalidArgument`: a zero count, an unknown policy, a publisher of another
/// node. `ResourceLimit`: fewer loans than the count. `InvalidState`: a publisher another
/// cycle owns, a committed cycle or a running `WaitSet`.
///
/// # Safety
/// `cycle`, `publisher`, and `out_index` must be live pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_cycle_bind_output(
    cycle: *mut HyphaCycle,
    publisher: *mut HyphaPublisher,
    policy: u32,
    max_messages_per_step: u32,
    out_index: *mut u32,
) -> HyphaStatus {
    try_guard(|| {
        let cycle = unsafe { CycleTarget::of(cycle) }?;
        unsafe { handle_ref(publisher) }?;
        unsafe { write_out(out_index, 0) }?;
        let (set, cycle) = unsafe { cycle.waitset() }?;
        let owned = unsafe { take_box(publisher) }.ok_or(HyphaStatus::InvalidArgument)?;
        match cycle_bind_output(&set, cycle, owned, policy, max_messages_per_step) {
            Ok(index) => unsafe { write_out(out_index, index) },
            Err((error, refused)) => {
                let _caller_keeps = Box::into_raw(refused);
                Err(HyphaStatus::from(error))
            }
        }
    })
}

/// Add the queued, wired input `binding_index` to the triggers of an uncommitted cycle
/// created with `period_nanos == 0`: the cycle runs one step per sample of any trigger,
/// in send-stamp order, a tie in binding order. Each call adds one trigger, a repeat adds
/// nothing, and the order of the calls has no meaning; a trigger drops no arrival.
/// Commit requires each trigger's silence limit to be positive; after the shortest limit
/// without a step a fallback step runs. `InvalidArgument`: null cycle, unknown, latest or
/// unwired binding, or a binding whose source another trigger reads. `ResourceLimit`:
/// more than 64 triggers. `InvalidState`: committed cycle or running `WaitSet`.
///
/// # Safety
/// `cycle` is null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_cycle_set_trigger(
    cycle: *mut HyphaCycle,
    binding_index: u32,
) -> HyphaStatus {
    try_guard(|| {
        let (set, cycle) = unsafe { CycleTarget::of(cycle)?.waitset() }?;
        Ok(cycle_set_trigger(&set, cycle, binding_index)?)
    })
}

/// Capture the latest input `binding_index` of an uncommitted cycle on the host clock,
/// for data stamped on another clock. Under simulated time a step takes the newest
/// sample whatever its stamp and judges its silence limit by host time.
/// `InvalidArgument`: null cycle, unknown or queued binding.
/// `InvalidState`: committed cycle or running `WaitSet`.
///
/// # Safety
/// `cycle` is null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_cycle_set_host_clock(
    cycle: *mut HyphaCycle,
    binding_index: u32,
) -> HyphaStatus {
    try_guard(|| {
        let (set, cycle) = unsafe { CycleTarget::of(cycle)?.waitset() }?;
        Ok(cycle_set_host_clock(&set, cycle, binding_index)?)
    })
}

/// Mark a class-based cycle so its `[cycle]` and input tables may override code
/// defaults. The commit reads `[cycle]` keys `arrival_budget` (optional positive limit
/// on queued arrivals examined in one step) and `rate_hz`; any other key refuses the
/// commit. `InvalidState`: a committed cycle or a running `WaitSet`.
///
/// # Safety
/// `cycle` must be a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_cycle_set_class_based(
    cycle: *mut HyphaCycle,
    class_based: u8,
) -> HyphaStatus {
    try_guard(|| {
        let cycle = unsafe { CycleTarget::of(cycle) }?;
        let class_based = match class_based {
            0 => false,
            1 => true,
            _ => return Err(HyphaStatus::InvalidArgument),
        };
        let (set, cycle) = unsafe { cycle.waitset() }?;
        Ok(cycle_set_class_based(&set, cycle, class_based)?)
    })
}

/// Atomically attach the cycle timer, triggers or clock driver after all bindings; an
/// uncommitted cycle never runs. `InvalidArgument`: neither a period nor a trigger, both,
/// or a refused `[cycle]` key. A trigger binding without a silence limit is untimed: it
/// runs no fallback of its own, and a cycle of untimed triggers runs no fallback.
/// `InvalidState`: a committed cycle, a running `WaitSet`, any cycle on a CLI tool node
/// or on a node with a clock driver, and a driver in a process already on simulated
/// time, on a node with another cycle, or while another process publishes `/clock`. A
/// driver commit enables simulated time. A refusal reason goes to `hypha_last_error`.
/// # Safety
/// `cycle` must be a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_cycle_commit(cycle: *mut HyphaCycle) -> HyphaStatus {
    try_guard(|| {
        let cycle = unsafe { CycleTarget::of(cycle) }?;
        // Mirror `cycle_commit`, which clears the last error first.
        hypha_node::last_error::clear();
        let (set, cycle) = unsafe { cycle.waitset() }?;
        Ok(cycle_commit(&set, cycle)?)
    })
}

/// Destroy a cycle before its `WaitSet`: release its callback context, publishers and
/// source references and free the handle. A running `WaitSet` refuses with
/// `InvalidState` and changes nothing. A destroyed `WaitSet` destroys its cycles.
/// Destroying a clock driver, or its `WaitSet`, turns process simulated time off.
///
/// # Safety
/// `cycle` must be a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_cycle_destroy(cycle: *mut HyphaCycle) -> HyphaStatus {
    try_guard(|| {
        let (set, cycle) = unsafe { CycleTarget::of(cycle)?.waitset() }?;
        Ok(cycle_destroy(&set, cycle)?)
    })
}
