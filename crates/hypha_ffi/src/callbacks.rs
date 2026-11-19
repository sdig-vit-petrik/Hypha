//! The C callbacks of a registration or a cycle, each with the caller's context and the
//! function that releases it, owned by `hypha_node` through its handler traits.
//!
//! Each drop calls `release` once, after no callback can run again. A refused attach or
//! create builds none, so the caller keeps its context.

use std::os::raw::c_void;
use std::sync::Arc;

use crate::cycle::{HyphaCycle, OwnedHandle};
use hypha_node::safe::{
    CycleId, CycleSnapshots, CycleStepHandler, DeliveryHandler, ForeignCallbacks, HyphaWaitSet,
    IntervalHandler,
};
use hypha_node::waitset::{
    HyphaAttachment, HyphaCallbackResult, HyphaContextRelease, HyphaDelivery,
    HyphaIntervalCallback, HyphaSubscriptionCallback,
};
use hypha_node::{HyphaCycleEmission, HyphaCycleImage, HyphaCycleStep, HyphaCycleTime};

/// What a missing callback answers; decode refuses a registration without one.
const CONTINUE: HyphaCallbackResult = HyphaCallbackResult::continue_run();

/// A caller's context and the function that releases it, once, on drop.
struct CallerContext {
    pointer: *mut c_void,
    release: HyphaContextRelease,
}

// SAFETY: by the caller's contract the context may be used from the thread that runs
// the `WaitSet`, one at a time; it is never dereferenced here.
unsafe impl Send for CallerContext {}

impl Drop for CallerContext {
    fn drop(&mut self) {
        if let Some(release) = self.release {
            release(self.pointer);
        }
    }
}

/// The subscription callback of a C registration.
struct CSubscription {
    function: HyphaSubscriptionCallback,
    context: CallerContext,
}

impl DeliveryHandler for CSubscription {
    fn deliver(&mut self, delivery: &HyphaDelivery) -> HyphaCallbackResult {
        let Some(function) = self.function else {
            return CONTINUE;
        };
        function(delivery, self.context.pointer)
    }
}

/// The timer callback of a C registration.
struct CTimer {
    function: HyphaIntervalCallback,
    context: CallerContext,
}

impl IntervalHandler for CTimer {
    fn tick(&mut self, missed_periods: u64) -> HyphaCallbackResult {
        let Some(function) = self.function else {
            return CONTINUE;
        };
        function(missed_periods, self.context.pointer)
    }
}

/// Takes over the callbacks of `hypha_waitset_attach`.
pub(crate) struct CCallbacks;

impl ForeignCallbacks for CCallbacks {
    fn subscription(attachment: &HyphaAttachment) -> Box<dyn DeliveryHandler> {
        let context = CallerContext { pointer: attachment.context, release: attachment.release };
        Box::new(CSubscription { function: attachment.subscription_callback, context })
    }

    fn timer(attachment: &HyphaAttachment) -> Box<dyn IntervalHandler> {
        let context = CallerContext { pointer: attachment.context, release: attachment.release };
        Box::new(CTimer { function: attachment.periodic_callback, context })
    }
}

/// The step callback of a C cycle, and the cycle's C handle. The fields drop in order:
/// the release of the context first, then the handle.
pub(crate) struct CStep {
    function: extern "C" fn(
        *const HyphaCycleImage,
        *const HyphaCycleTime,
        *mut HyphaCycleEmission,
        *mut c_void,
    ) -> HyphaCallbackResult,
    context: CallerContext,
    /// Freed by its drop, after the release; nothing reads it here.
    #[expect(dead_code, reason = "held for its drop, which frees the cycle's handle")]
    handle: OwnedHandle,
}

impl CStep {
    /// The owner of `step` and `context`, or `None` without a step. Called once the
    /// create can no longer fail; writes the new cycle's handle to `handle`.
    pub(crate) fn adopter(
        step: HyphaCycleStep,
        context: *mut c_void,
        release: HyphaContextRelease,
        waitset: *const HyphaWaitSet,
        handle: &mut *mut HyphaCycle,
    ) -> Option<impl FnOnce(CycleId, &Arc<CycleSnapshots>) -> Box<dyn CycleStepHandler> + '_> {
        let function = step?;
        Some(move |id: CycleId, snapshots: &Arc<CycleSnapshots>| -> Box<dyn CycleStepHandler> {
            let owned = OwnedHandle::new(waitset, id, snapshots);
            *handle = owned.as_ptr();
            let context = CallerContext { pointer: context, release };
            Box::new(Self { function, context, handle: owned })
        })
    }
}

impl CycleStepHandler for CStep {
    fn step(
        &mut self,
        image: *const HyphaCycleImage,
        time: *const HyphaCycleTime,
        emission: *mut HyphaCycleEmission,
    ) -> HyphaCallbackResult {
        (self.function)(image, time, emission, self.context.pointer)
    }
}
