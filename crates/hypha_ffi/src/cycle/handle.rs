//! The C handle of a cycle, and how a C call reaches the cycle from it.

use std::ptr::NonNull;
use std::sync::Arc;

use crate::waitset::access::{ClaimedWaitSet, waitset_here};
use hypha_node::safe::{CycleId, CycleSnapshots, HyphaWaitSet};
use hypha_node::status::HyphaStatus;

/// One cycle registration. Opaque to callers.
///
/// The cycle owns its handle and frees it with itself, at `hypha_cycle_destroy` or the
/// destruction of the `WaitSet`.
pub struct HyphaCycle {
    /// The `WaitSet` that owns the cycle. It outlives the handle, which the cycle frees.
    waitset: *const HyphaWaitSet,
    /// The cycle in that `WaitSet`.
    id: CycleId,
    /// What any thread may read of the cycle.
    snapshots: Arc<CycleSnapshots>,
}

/// The handle of one cycle, held by the owner of the cycle's step callback. Freed on drop.
pub(crate) struct OwnedHandle(NonNull<HyphaCycle>);

impl OwnedHandle {
    /// A new handle of cycle `id` of `waitset`, sharing `snapshots`.
    pub(crate) fn new(
        waitset: *const HyphaWaitSet,
        id: CycleId,
        snapshots: &Arc<CycleSnapshots>,
    ) -> Self {
        let handle = Box::new(HyphaCycle { waitset, id, snapshots: Arc::clone(snapshots) });
        Self(NonNull::from(Box::leak(handle)))
    }

    /// The address the C caller receives.
    pub(crate) fn as_ptr(&self) -> *mut HyphaCycle {
        self.0.as_ptr()
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: from `Box::leak` in `OwnedHandle::new`; only this drop takes it back, once.
        drop(unsafe { Box::from_raw(self.0.as_ptr()) });
    }
}

// SAFETY: the owner dereferences the handle only to free it; its contents are written
// once, when it is built.
unsafe impl Send for OwnedHandle {}

/// The snapshots of the cycle behind `cycle`, reached without a reference to the
/// handle (not `Sync`). Any thread may call it while the `WaitSet` runs.
/// `InvalidArgument` for null.
///
/// # Safety
/// `cycle` must be null or a live cycle handle, live for `'a`.
pub(crate) unsafe fn snapshots_of<'a>(
    cycle: *const HyphaCycle,
) -> Result<&'a CycleSnapshots, HyphaStatus> {
    if cycle.is_null() {
        return Err(HyphaStatus::InvalidArgument);
    }
    // SAFETY: non-null by the check and live by the caller; a place projection borrows
    // only this field, written only when the handle is built, and the `Arc` is `Sync`.
    let snapshots: &Arc<CycleSnapshots> = unsafe { &(*cycle).snapshots };
    Ok(snapshots)
}

/// The `WaitSet` and the id that a cycle handle names, read from the handle.
#[derive(Clone, Copy)]
pub(crate) struct CycleTarget {
    waitset: *const HyphaWaitSet,
    id: CycleId,
}

impl CycleTarget {
    /// What the handle `cycle` names. `InvalidArgument` for null.
    ///
    /// # Safety
    /// `cycle` must be null or a live cycle handle.
    pub(crate) unsafe fn of(cycle: *const HyphaCycle) -> Result<Self, HyphaStatus> {
        if cycle.is_null() {
            return Err(HyphaStatus::InvalidArgument);
        }
        // SAFETY: non-null by the check and live by the caller; a place read of a field
        // written only when the handle is built.
        let waitset = unsafe { (*cycle).waitset };
        // SAFETY: as above, for the other field.
        let id = unsafe { (*cycle).id };
        Ok(Self { waitset, id })
    }

    /// The `WaitSet` of the cycle, claimed for the call, and its id. `InvalidState` while
    /// another thread is inside a call or a run on it.
    ///
    /// # Safety
    /// The handle this target was read from must still be live for `'a`.
    pub(crate) unsafe fn waitset<'a>(self) -> Result<(ClaimedWaitSet<'a>, CycleId), HyphaStatus> {
        // SAFETY: the `WaitSet` outlives its cycles' handles; the rest is the caller's.
        let set = unsafe { waitset_here(self.waitset) }?;
        Ok((set, self.id))
    }
}

#[cfg(test)]
mod tests {
    //! The handle is freed with its cycle, and only then.
    #![allow(clippy::expect_used, reason = "a failed test setup should stop the test")]

    use std::os::raw::c_void;
    use std::ptr;
    use std::sync::{Arc, Weak};

    use super::{CycleSnapshots, HyphaCycle};
    use crate::waitset::HyphaCallbackResult;
    use crate::{
        HyphaCycleEmission, HyphaCycleImage, HyphaCycleTime, HyphaNode, HyphaStatus, HyphaWaitSet,
    };

    extern "C" fn step(
        _image: *const HyphaCycleImage,
        _time: *const HyphaCycleTime,
        _emission: *mut HyphaCycleEmission,
        _context: *mut c_void,
    ) -> HyphaCallbackResult {
        HyphaCallbackResult::continue_run()
    }

    fn node_and_waitset(label: &str) -> (*mut HyphaNode, *mut HyphaWaitSet) {
        let name = hypha_testkit::unique_name(label);
        let mut node = ptr::null_mut();
        // SAFETY: the name comes with its length; no parameter file; `node` is a local.
        let created = unsafe {
            crate::hypha_node_create(
                name.as_ptr().cast(),
                name.len(),
                ptr::null(),
                0,
                ptr::null(),
                &raw mut node,
            )
        };
        assert_eq!(created, HyphaStatus::Ok);
        let mut waitset = ptr::null_mut();
        // SAFETY: `node` is live; `waitset` is a local.
        let lent = unsafe { crate::hypha_node_default_waitset(node, &raw mut waitset) };
        assert_eq!(lent, HyphaStatus::Ok);
        (node, waitset)
    }

    /// A cycle `name` of `waitset` at `period_nanos`, with the snapshots its handle shares.
    fn cycle(
        waitset: *mut HyphaWaitSet,
        name: &str,
        period_nanos: u64,
    ) -> (*mut HyphaCycle, Weak<CycleSnapshots>) {
        let mut cycle = ptr::null_mut();
        // SAFETY: `waitset` is live; the name comes with its length; `cycle` is a local.
        let created = unsafe {
            crate::hypha_cycle_create(
                waitset,
                name.as_ptr().cast(),
                name.len(),
                period_nanos,
                Some(step),
                ptr::null_mut(),
                None,
                &raw mut cycle,
            )
        };
        assert_eq!(created, HyphaStatus::Ok);
        // SAFETY: the handle was just created and is live.
        let snapshots = unsafe { super::snapshots_of(cycle) }.expect("a live handle");
        // SAFETY: as above; the field is the `Arc` the handle holds.
        let shared: &Arc<CycleSnapshots> = unsafe { &(*cycle).snapshots };
        assert!(ptr::eq(snapshots, Arc::as_ptr(shared)));
        (cycle, Arc::downgrade(shared))
    }

    #[test]
    fn a_destroyed_cycle_frees_its_handle() {
        let (node, waitset) = node_and_waitset("handle_destroy");
        let (handle, snapshots) = cycle(waitset, "destroyed", 1_000_000);
        assert_eq!(snapshots.strong_count(), 2, "the cycle and its handle");
        // SAFETY: the handle is live and not used afterwards.
        assert_eq!(unsafe { crate::hypha_cycle_destroy(handle) }, HyphaStatus::Ok);
        assert_eq!(snapshots.strong_count(), 0, "the cycle and its handle are gone");
        // SAFETY: `node` is live and destroyed once.
        unsafe { crate::hypha_node_destroy(node) };
    }

    #[test]
    fn the_waitset_frees_the_handles_of_its_cycles() {
        let (node, waitset) = node_and_waitset("handle_waitset");
        let (accepted, committed) = cycle(waitset, "committed", 1_000_000);
        let (refused, uncommitted) = cycle(waitset, "uncommitted", 0);
        // SAFETY: the handle is live.
        assert_eq!(unsafe { crate::hypha_cycle_commit(refused) }, HyphaStatus::InvalidArgument);
        assert_eq!(uncommitted.strong_count(), 2);
        // SAFETY: the handle is live.
        assert_eq!(unsafe { crate::hypha_cycle_commit(accepted) }, HyphaStatus::Ok);
        // SAFETY: `node` is live and destroyed once; it destroys its `WaitSet` first.
        unsafe { crate::hypha_node_destroy(node) };
        assert_eq!(committed.strong_count(), 0);
        assert_eq!(uncommitted.strong_count(), 0);
    }
}
