//! Caller-owned execution at the C ABI.
//!
//! The application takes its node's `WaitSet` (`hypha_node_default_waitset`),
//! attaches work to it, and calls [`hypha_waitset_run`] on a thread it owns.
//! This module starts no thread.

pub mod clock_driver;
pub mod dispatch;
pub mod replay;
pub mod sim_clock;
pub mod triggered;
pub mod upstream;
use dispatch::CallbackDispatcher;
pub(crate) use dispatch::status_from_code;
pub mod callbacks;
pub(crate) use callbacks::apply_sim_clock_sample;
use callbacks::{invoke_cycle, invoke_data, invoke_interval};
pub mod configuration;
pub(crate) use configuration::WaitSetSlot;
pub use configuration::{WaitSetLease, node_default_waitset};
pub mod registrations;
pub(crate) use clock_driver::attach_driver_cycle;
pub use registrations::{
    HYPHA_ATTACH_SUBSCRIPTION, HYPHA_ATTACH_TIMER, HyphaAttachment, waitset_attach_subscription,
    waitset_detach,
};
pub(crate) use registrations::{attach_cycle_timer, attach_triggered_cycle};
pub mod run;
pub use run::{waitset_request_stop, waitset_run};
mod service_edges;
mod services;
pub(crate) use services::{ServiceTarget, attach_service, detach_service};
pub mod timer_control;
use timer_control::TimerControl;
pub use timer_control::{
    HyphaTimerState, waitset_timer_cancel, waitset_timer_reset, waitset_timer_state,
};

use std::cell::{Ref, RefCell, RefMut};
use std::os::raw::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use hypha_core::dispatch::{BatchStep, BatchTurn, DispatchLimits};
use hypha_core::port::dispatch::{AttachmentId, FIRST_DISPATCHER_IDENTIFIER, StopReason};
use hypha_iceoryx2::waitset::{Dispatcher, EventOutcome, Iox2WaitSet, Progression, Trigger};

use crate::node::HyphaNode;
use crate::node_health::ExecutionHealth;
use crate::status::{Error, HyphaStatus};
use crate::subscriber::{
    FrameIdentity, HyphaSampleIdentity, HyphaSubscriber, ReceiveFailure, host_now_nanos,
};

/// Identifies one registration. Never zero, and never the identifier of
/// another live registration.
pub type HyphaAttachmentId = u64;

/// One taken sample, handed to a subscription callback.
///
/// A frame that did not parse is still delivered, with `payload` null and
/// `payload_len` zero; `identity.topic` is filled in regardless.
#[repr(C)]
pub struct HyphaDelivery {
    /// The message payload, or null when the frame did not parse.
    pub payload: *const u8,
    /// Payload byte count. Zero when `payload` is null.
    pub payload_len: usize,
    /// Topic, and, when the frame parsed, schema hash and send time.
    pub identity: HyphaSampleIdentity,
    /// Nanoseconds between the send and this delivery, on the host monotonic
    /// clock. Zero when `payload` is null.
    pub transport_age_nanos: u64,
    /// The sample, which `hypha_delivery_retain` keeps after the call. Valid only for the
    /// call; null when the delivery cannot be kept.
    pub holder: *const crate::sample::HyphaDeliveryHolder,
}

/// The `WaitSet` of one node, which owns it. Opaque to the caller.
///
/// One thread at a time uses it: between runs any thread, during a run the thread that
/// called `hypha_waitset_run`. During a run another thread may call
/// `hypha_waitset_request_stop`, `hypha_waitset_request_failure`, `hypha_cycle_health` and
/// `hypha_cycle_binding`; any other call that names this `WaitSet`, one of its cycles or
/// input sources returns `InvalidState` on that thread and changes nothing. On the run's
/// own thread a callback may also detach a registration, applied when the run returns;
/// attach a timer whose wake needs no native attachment, which joins the dispatch before
/// the next wait; and cancel, reset or read a timer. Any other attach, a second run, and
/// a call that creates, configures or destroys a cycle or an input source return
/// `InvalidState`. Each call and each run claims the `WaitSet` for the calling thread,
/// so a run or a call that another thread starts while one is inside a call returns
/// `InvalidState` too (#83).
pub struct HyphaWaitSet {
    /// The node this `WaitSet` serves and co-owns: its destruction drops the `WaitSet` first.
    node: Arc<HyphaNode>,
    /// The native listeners. Borrowed mutably for the length of a run.
    native: RefCell<Iox2WaitSet>,
    /// The state another thread may touch during a run, reached by a place projection
    /// of the raw handle (`&(*waitset).shared`) so no call forms a reference to this
    /// `!Sync` `WaitSet` on a thread other than the run's. Not part of the Rust API.
    #[doc(hidden)]
    pub shared: Arc<WaitSetShared>,
    /// Borrowed mutably for the length of a run.
    registrations: RefCell<Vec<Registration>>,
    /// The cycles of this `WaitSet`, which owns them. Registrations and C handles name a
    /// cycle by its id and look it up in this list.
    cycles: RefCell<Cycles>,
    /// The id the next cycle takes. Starts at one and only grows.
    next_cycle: std::cell::Cell<crate::cycle::CycleId>,
    limits: DispatchLimits,
    warn_interval_nanos: u64,
    /// Registrations detached by a callback during the current run, removed and released
    /// once the run returns.
    pending_detach: RefCell<Vec<AttachmentId>>,
    /// The timers a callback attached during the current run, not borrowed by the run.
    run_attaches: RefCell<RunAttaches>,
    /// The live registrations that `attach` recorded, with the control of each plain
    /// timer. Not borrowed by a run, so a callback's detach, cancel, reset or state read
    /// can tell a known registration from an unknown one.
    live: RefCell<Vec<LiveRegistration>>,
    input_sources: RefCell<Vec<Weak<crate::cycle::CycleInputSource>>>,
    /// The node's execution counters, shared from this `WaitSet`'s creation.
    pub(crate) health: Arc<ExecutionHealth>,
}

/// The cycles a `WaitSet` owns; nothing holds a cycle's address, so it may move in the list.
pub(crate) type Cycles = Vec<crate::cycle::Cycle>;

/// The timers a callback attached during a run, which the dispatcher alone holds.
struct RunAttaches {
    /// Attached since the dispatcher last looked. It takes them before its next wait,
    /// and the run's end takes those it did not.
    pending: Vec<Registration>,
    /// How many registrations the running dispatcher holds, for the registration limit.
    held: usize,
    /// The id the next one takes. Starts at [`FIRST_DISPATCHER_IDENTIFIER`], above every
    /// id of the native set, and only grows.
    next_id: AttachmentId,
}

impl RunAttaches {
    fn new() -> Self {
        Self { pending: Vec::new(), held: 0, next_id: FIRST_DISPATCHER_IDENTIFIER }
    }

    /// A new id, which the native set never hands out. `ResourceLimit` once the ids ran
    /// out, which no process reaches.
    fn take_id(&mut self) -> Result<AttachmentId, Error> {
        let id = self.next_id;
        self.next_id = id.checked_add(1).ok_or(Error::ResourceLimit)?;
        Ok(id)
    }
}

/// One entry of the registry of live registrations, which no run borrows.
struct LiveRegistration {
    id: AttachmentId,
    /// The control of a plain timer; `None` for any other registration.
    timer: Option<Arc<TimerControl>>,
}

// `Send` by its fields: the C ABI lets its caller run it, and destroy its node, on any
// thread, one thread at a time. It stays `!Sync`: its configuration lives in `RefCell`s.
const _: () = {
    const fn moves_between_threads<T: Send>() {}
    moves_between_threads::<HyphaWaitSet>();
};

/// The part of a `WaitSet` that another thread may touch while it runs: whether it runs,
/// the native stop, and the failure the application requested.
pub struct WaitSetShared {
    pub(crate) running: hypha_runtime::RunState,
    /// The only native state another thread accesses during a run.
    stop: hypha_iceoryx2::waitset::StopHandle,
    /// Set by [`WaitSetShared::request_failure`] from any thread: the current or next run
    /// ends as failed with [`HyphaStatus::ApplicationFailed`]. The run that reports it clears it.
    failure: std::sync::atomic::AtomicBool,
    /// The claim token of the thread inside a call or a run on this `WaitSet`, 0 when
    /// none is ([`WaitSetShared::claim`]).
    owner: AtomicU64,
}

/// The source of the claim tokens; each thread takes one, never 0, on its first claim.
static NEXT_CLAIM_TOKEN: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static CLAIM_TOKEN: u64 = NEXT_CLAIM_TOKEN.fetch_add(1, Ordering::Relaxed);
}

/// The calling thread's claim token.
fn claim_token() -> u64 {
    CLAIM_TOKEN.with(|token| *token)
}

/// The claim of a `WaitSet` by the calling thread for one call or one run, released when
/// it drops. A claim inside a claim of the same thread, such as a callback's call during
/// the run, holds nothing more and releases nothing.
#[must_use = "the claim holds the WaitSet until it drops"]
pub struct WaitSetClaim<'a> {
    shared: &'a WaitSetShared,
    /// Whether this claim took the `WaitSet`, so its drop gives it back.
    took: bool,
}

impl Drop for WaitSetClaim<'_> {
    fn drop(&mut self) {
        if self.took {
            self.shared.owner.store(0, Ordering::Release);
        }
    }
}

// `hypha_waitset_request_stop` and `hypha_waitset_request_failure` reach this from
// another thread.
const _: () = {
    const fn shared_across_threads<T: Send + Sync>() {}
    shared_across_threads::<WaitSetShared>();
};

impl WaitSetShared {
    /// The state of a new `WaitSet` over the native `stop`: not running, no failure.
    pub(crate) fn new(stop: hypha_iceoryx2::waitset::StopHandle) -> Self {
        Self {
            running: hypha_runtime::RunState::default(),
            stop,
            failure: std::sync::atomic::AtomicBool::new(false),
            owner: AtomicU64::new(0),
        }
    }

    /// Claim the `WaitSet` for the calling thread until the claim drops: one
    /// compare-and-swap of the owner, so of two threads that start a call or a run at once
    /// exactly one gets it. A C call claims before it forms a reference to the `WaitSet`,
    /// and a run claims for its whole length. Any thread may call it.
    ///
    /// # Errors
    /// `InvalidState` while another thread is inside a call or a run on this `WaitSet`;
    /// nothing is claimed then.
    pub fn claim(&self) -> Result<WaitSetClaim<'_>, Error> {
        let token = claim_token();
        match self.owner.compare_exchange(0, token, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => Ok(WaitSetClaim { shared: self, took: true }),
            Err(held) if held == token => Ok(WaitSetClaim { shared: self, took: false }),
            Err(_) => Err(Error::InvalidState),
        }
    }

    /// Whether a run is in progress on the calling thread: the run holds the claim for its
    /// whole length, so a call from one of its callbacks is the only one that sees it.
    pub(crate) fn runs_on_this_thread(&self) -> bool {
        self.running.is_running() && self.owner.load(Ordering::Acquire) == claim_token()
    }

    /// Whether the application asked for the run to fail. Any thread may read it.
    pub(crate) fn failure_requested(&self) -> bool {
        self.failure.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Clear the failure request and return it: the run that reports it consumes it.
    pub(crate) fn take_failure(&self) -> bool {
        self.failure.swap(false, std::sync::atomic::Ordering::AcqRel)
    }

    /// Ask the run to stop. Any thread may call it. Returns whether a run was in progress.
    ///
    /// # Errors
    /// The transport's error when the stop cannot be signalled.
    pub fn request_stop(&self) -> Result<bool, Error> {
        let was_running = self.running.is_running();
        self.stop.request_stop().map_err(Error::from)?;
        Ok(was_running)
    }

    /// Ask the run to end as failed. Any thread may call it. Returns what
    /// [`WaitSetShared::request_stop`] returns.
    ///
    /// # Errors
    /// The transport's error when the stop cannot be signalled.
    pub fn request_failure(&self) -> Result<bool, Error> {
        self.failure.store(true, std::sync::atomic::Ordering::Release);
        self.request_stop()
    }
}

impl HyphaWaitSet {
    pub(crate) fn failure_requested(&self) -> bool {
        self.shared.failure_requested()
    }

    pub(crate) fn is_running(&self) -> bool {
        self.shared.running.is_running()
    }

    pub(crate) fn node(&self) -> &HyphaNode {
        &self.node
    }

    /// The address of the node, as its endpoints record it; compared, never dereferenced.
    pub(crate) fn node_address(&self) -> usize {
        Arc::as_ptr(&self.node) as usize
    }

    /// The cycles, shared. `InvalidState` while a configuration call holds them.
    pub(crate) fn cycles(&self) -> Result<Ref<'_, Cycles>, Error> {
        self.cycles.try_borrow().map_err(|_| Error::InvalidState)
    }

    /// The cycle `id`, shared. `InvalidState` while a configuration call holds the
    /// cycles; `InvalidArgument` for an id this `WaitSet` does not hold.
    pub(crate) fn cycle(
        &self,
        id: crate::cycle::CycleId,
    ) -> Result<Ref<'_, crate::cycle::Cycle>, Error> {
        Ref::filter_map(self.cycles()?, |cycles| cycles.iter().find(|cycle| cycle.id == id))
            .map_err(|_| Error::InvalidArgument)
    }

    /// A new cycle id. `ResourceLimit` once the ids ran out, which no process reaches.
    pub(crate) fn take_cycle_id(&self) -> Result<crate::cycle::CycleId, Error> {
        let id = self.next_cycle.get();
        let next = id.checked_add(1).ok_or(Error::ResourceLimit)?;
        self.next_cycle.set(next);
        Ok(id)
    }

    /// The cycles, for a configuration call. `InvalidState` during a run.
    pub(crate) fn cycles_mut(&self) -> Result<RefMut<'_, Cycles>, Error> {
        if self.is_running() {
            return Err(Error::InvalidState);
        }
        self.cycles.try_borrow_mut().map_err(|_| Error::InvalidState)
    }

    /// The input sources created on this `WaitSet`, for a configuration call.
    /// `InvalidState` during a run.
    pub(crate) fn input_sources_mut(
        &self,
    ) -> Result<RefMut<'_, Vec<Weak<crate::cycle::CycleInputSource>>>, Error> {
        if self.is_running() {
            return Err(Error::InvalidState);
        }
        self.input_sources.try_borrow_mut().map_err(|_| Error::InvalidState)
    }

    /// `InvalidState` during a run, which holds the native set.
    fn native_attach(
        &self,
        listeners: Vec<hypha_iceoryx2::Iox2Listener>,
        trigger: Trigger,
    ) -> Result<AttachmentId, Error> {
        let mut native = self.native.try_borrow_mut().map_err(|_| Error::InvalidState)?;
        native.attach(listeners, trigger).map_err(Error::from)
    }
}

/// True while this thread is inside a `WaitSet` run.
/// A call that would wait for a round trip is refused here: the dispatcher
/// thread is the one that has to deliver that round trip's answer.
pub(crate) fn in_dispatch() -> bool {
    IN_DISPATCH.with(std::cell::Cell::get)
}

thread_local! {
    static IN_DISPATCH: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// What the callback asks the run loop to do next.
pub const HYPHA_ACTION_CONTINUE: u32 = 0;
/// Stop the run and report a requested stop.
pub const HYPHA_ACTION_STOP: u32 = 1;
/// Stop the run and report a failure.
pub const HYPHA_ACTION_FAIL: u32 = 2;

/// Why a run returned.
pub const HYPHA_RUN_REASON_STOPPED: u32 = 0;
/// Building the native attachments failed. No callback ran.
pub const HYPHA_RUN_REASON_SETUP_FAILED: u32 = 1;
/// The run failed after it started.
pub const HYPHA_RUN_REASON_RUNTIME_FAILED: u32 = 2;
/// A bounded run reached its time limit. The application asked for no stop.
pub const HYPHA_RUN_REASON_DEADLINE: u32 = 3;

/// What a callback asks the run loop to do next.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct HyphaCallbackResult {
    /// One of the `HYPHA_ACTION_*` values.
    pub action: u32,
    /// An [`HyphaStatus`] value. Zero reports success. A nonzero status with
    /// the continue action reports a failure that does not stop the run.
    pub status: i32,
}

impl HyphaCallbackResult {
    /// Go on with the run. A function, not an associated constant, so that cbindgen
    /// leaves the header alone.
    #[must_use]
    pub const fn continue_run() -> Self {
        Self { action: HYPHA_ACTION_CONTINUE, status: 0 }
    }

    /// Stop the run and report its failure with `status`.
    #[must_use]
    pub const fn fail(status: i32) -> Self {
        Self { action: HYPHA_ACTION_FAIL, status }
    }
}

/// The outcome of one run.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct HyphaWaitSetRunReport {
    /// One of the `HYPHA_RUN_REASON_*` values.
    pub reason: u32,
    /// An [`HyphaStatus`] value. Zero when the run stopped as requested.
    pub status: i32,
    /// The registration the failure belongs to. Zero when it belongs to none.
    pub attachment: HyphaAttachmentId,
    /// Callbacks that ran longer than 10 ms of host monotonic time. Each is logged at
    /// most once per 1 s warning interval.
    pub slow_callbacks: u64,
}

/// Releases a callback context. A successful attach transfers ownership of the
/// context to the registration when this function is supplied.
///
/// # Safety
/// Must not unwind. Runs exactly once, after no callback can run again.
pub type HyphaContextRelease = Option<extern "C" fn(context: *mut c_void)>;

/// Called for each sample a subscription registration accepts.
///
/// # Safety
/// Must not unwind. `delivery` is valid only for the call.
pub type HyphaSubscriptionCallback = Option<
    extern "C" fn(delivery: *const HyphaDelivery, context: *mut c_void) -> HyphaCallbackResult,
>;

/// Called once for each period a timer registration is due.
///
/// # Safety
/// Must not unwind.
pub type HyphaIntervalCallback =
    Option<extern "C" fn(missed_periods: u64, context: *mut c_void) -> HyphaCallbackResult>;

/// One sample a Rust subscription callback receives. It borrows the sample and is
/// valid only for the call.
#[derive(Debug, Clone, Copy)]
pub struct SampleDelivery<'a> {
    /// The message payload.
    pub payload: &'a [u8],
    /// Topic, schema hash and send times of the frame.
    pub identity: FrameIdentity<'a>,
    /// Nanoseconds between the send and this delivery, on the host monotonic clock.
    pub transport_age_nanos: u64,
}

/// What a Rust subscription callback asks the run loop to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallbackAction {
    /// Go on with the run.
    Continue,
    /// Stop the run and report a requested stop.
    Stop,
    /// Stop the run and report its failure with this error.
    Fail(Error),
}

impl CallbackAction {
    /// The answer of a C callback that asks the same.
    fn as_result(self) -> HyphaCallbackResult {
        match self {
            Self::Continue => HyphaCallbackResult::continue_run(),
            Self::Stop => HyphaCallbackResult { action: HYPHA_ACTION_STOP, status: 0 },
            Self::Fail(error) => HyphaCallbackResult::fail(HyphaStatus::from(error) as i32),
        }
    }
}

/// A Rust subscription callback, owned by the registration until the detach or the
/// destruction of the `WaitSet`. It runs on the thread that runs the `WaitSet`.
pub trait SubscriptionHandler: Send {
    /// Handle one sample.
    fn deliver(&mut self, delivery: &SampleDelivery<'_>, waitset: &HyphaWaitSet) -> CallbackAction;
}

impl<F> SubscriptionHandler for F
where
    F: FnMut(&SampleDelivery<'_>, &HyphaWaitSet) -> CallbackAction + Send,
{
    fn deliver(&mut self, delivery: &SampleDelivery<'_>, waitset: &HyphaWaitSet) -> CallbackAction {
        self(delivery, waitset)
    }
}

/// A C subscription callback and its context, owned by its registration until the
/// detach or the destruction of the `WaitSet`. Its drop releases the context, once.
pub trait DeliveryHandler: Send {
    /// Handle one sample, borrowed for the call.
    fn deliver(&mut self, delivery: &HyphaDelivery) -> HyphaCallbackResult;
}

/// A C timer callback and its context, owned by its registration as a
/// [`DeliveryHandler`] is.
pub trait IntervalHandler: Send {
    /// Handle one due period, `missed_periods` after the previous call.
    fn tick(&mut self, missed_periods: u64) -> HyphaCallbackResult;
}

/// How `hypha_ffi` takes over the callback and context of a C registration. The
/// attach calls one of these once nothing can refuse it, so a refused attach leaves
/// the context to the caller.
pub trait ForeignCallbacks {
    /// The owner of the subscription callback and context of `attachment`.
    fn subscription(attachment: &HyphaAttachment) -> Box<dyn DeliveryHandler>;
    /// The owner of the timer callback and context of `attachment`.
    fn timer(attachment: &HyphaAttachment) -> Box<dyn IntervalHandler>;
}

pub(crate) enum DataCallback {
    /// A C callback with its context. `None` once the registration released it.
    Foreign(Option<Box<dyn DeliveryHandler>>),
    /// A Rust callback. The registration is its owner.
    Native(Box<dyn SubscriptionHandler>),
}

enum Target {
    /// The registration shares the subscriber until the detach or the destruction of the
    /// `WaitSet`.
    Subscription { subscriber: Arc<HyphaSubscriber>, callback: DataCallback },
    /// Replays latched topics to subscribers that joined late.
    LatchReplay,
    /// Wakes the `WaitSet` periodically to drain `/clock` on simulated time.
    SimClock,
    /// A clock-driven timer the registration owns, due against framework time. On host
    /// time `run_anchored` is false until the first run turn re-anchors the timer, so the
    /// time before the first run is not counted as missed. A plain timer has a `control`,
    /// shared with the registry, which its cancel and reset change during a run.
    Timer {
        timer: hypha_core::timer::Timer,
        callback: Option<Box<dyn IntervalHandler>>,
        cycle: Option<Box<replay::CycleTimer>>,
        run_anchored: bool,
        control: Option<Arc<TimerControl>>,
    },
    /// A triggered cycle. Its periodic wake serves fallbacks and a wildcard trigger.
    Triggered(Box<triggered::TriggeredCycle>),
    /// The listener on a triggered cycle's trigger topic. It only wakes the `WaitSet`.
    TriggerWake,
    /// A cycle that drives `/clock` for a simulator. Its 1 ms wake polls the acknowledgements.
    Driver(Box<clock_driver::DriverCycle>),
    /// A server or a client, woken by its request or response event.
    Service(ServiceTarget),
}

/// The slow-callback diagnostics of one registration, apart from its target.
#[derive(Default)]
struct CallbackWarnings {
    slow_callbacks: u64,
    suppressed_warnings: u64,
    last_warning: Option<Instant>,
}

struct Registration {
    id: AttachmentId,
    target: Target,
    warnings: CallbackWarnings,
    /// Whether the last turn of this subscription failed in its transport.
    transport_failing: bool,
}

impl Registration {
    fn new(id: AttachmentId, target: Target) -> Self {
        Self { id, target, warnings: CallbackWarnings::default(), transport_failing: false }
    }

    /// The entry of this registration in the registry of live registrations.
    fn live_entry(&self) -> LiveRegistration {
        let timer = match self.target {
            Target::Timer { ref control, .. } => control.clone(),
            _ => None,
        };
        LiveRegistration { id: self.id, timer }
    }

    /// Tell a dispatched subscriber that this registration no longer drains it. Runs
    /// before the context release, which can free the last owner of the subscriber.
    fn end_dispatch(&self) {
        if let Target::Subscription { ref subscriber, .. } = self.target {
            subscriber.end_dispatch();
        }
    }

    /// Release the C context of this registration, once: its owner drops here.
    fn release_context(&mut self) {
        match self.target {
            Target::Subscription { callback: DataCallback::Foreign(ref mut owner), .. } => {
                drop(owner.take());
            }
            Target::Timer { callback: ref mut owner, .. } => drop(owner.take()),
            _ => {}
        }
    }
}

impl Drop for HyphaWaitSet {
    fn drop(&mut self) {
        crate::node_progress::withdraw_progress(self.node(), std::ptr::from_ref(self) as usize);
        // A run's end takes every timer attached during it; any left here goes the same way.
        let attached = self.run_attaches.get_mut().pending.iter_mut();
        for registration in self.registrations.get_mut().iter_mut().chain(attached) {
            registration.end_dispatch();
            registration.release_context();
        }
    }
}
