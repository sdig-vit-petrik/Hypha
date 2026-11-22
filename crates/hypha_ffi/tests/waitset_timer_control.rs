//! A plain timer that a callback cancels, resets, reads and creates (#90).
//!
//! Every case that asserts a time runs on simulated time, so the counts and times are
//! exact (#64). A node on its own simulated clock (`use_sim_time`) wakes its timers by
//! the dispatcher's deadline, so a callback may create one; a node on a process clock
//! driven from outside polls its timers natively and refuses it.
#![allow(clippy::expect_used, reason = "a failed test setup should stop the test with its message")]

mod common;

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::ptr;

use hypha_ffi::waitset::{
    HYPHA_ACTION_CONTINUE, HYPHA_ACTION_STOP, HYPHA_RUN_REASON_STOPPED, HyphaCallbackResult,
};
use hypha_ffi::*;

use crate::common::replay::{SimulatedClock, open_subscriber, set_clock};
use crate::common::{attach, unique};

const START: i64 = 1_000_000_000;
const PERIOD: i64 = 100_000_000;
const PERIOD_NANOS: u64 = PERIOD.unsigned_abs();
/// The first id above every id the native set hands out.
const FIRST_DISPATCHER_ID: u64 = 0x8000_0000_0000_0000;
const SAMPLE_MD5: &str = "7c8164229e7d2c17eb95e9231617fdee";
/// A run that one callback ends; the limit only guards against a hang.
const HANG_GUARD_NANOS: u64 = 10_000_000_000;

fn run(waitset: *mut HyphaWaitSet, limit_nanos: u64) -> HyphaWaitSetRunReport {
    let mut report =
        HyphaWaitSetRunReport { reason: 0, status: 0, attachment: 0, slow_callbacks: 0 };
    // SAFETY: every test passes the live `WaitSet` of a node it has not destroyed yet.
    let status = unsafe { hypha_waitset_run(waitset, limit_nanos, &raw mut report) };
    assert_eq!(status, HyphaStatus::Ok);
    report
}

/// One turn at the clock's current time: a run always takes its first turn at once, and
/// the 1 ms limit ends it soon after. The run must not fail.
fn turn(waitset: *mut HyphaWaitSet) {
    let report = run(waitset, 1_000_000);
    assert_eq!(report.status, HyphaStatus::Ok as i32, "{report:?}");
}

fn default_waitset(node: *mut HyphaNode) -> *mut HyphaWaitSet {
    let mut waitset = ptr::null_mut();
    // SAFETY: `node` is a live node a test has just created.
    let status = unsafe { hypha_node_default_waitset(node, &raw mut waitset) };
    assert_eq!(status, HyphaStatus::Ok);
    waitset
}

fn destroy(node: *mut HyphaNode) {
    // SAFETY: each test destroys each node it created once, after its last use.
    unsafe { hypha_node_destroy(node) };
}

/// A node on its own simulated clock at [`START`], and its `WaitSet`. The node goes
/// before the clock.
struct SimNode {
    node: *mut HyphaNode,
    waitset: *mut HyphaWaitSet,
    _clock: SimulatedClock,
}

impl SimNode {
    fn new(label: &str) -> Self {
        let clock = SimulatedClock::start(START);
        let node = common::create_sim_node_with_file(&unique(label), None);
        Self { node, waitset: default_waitset(node), _clock: clock }
    }

    /// A node on host time, on a process clock driven from outside it.
    fn on_external_clock(label: &str) -> Self {
        let clock = SimulatedClock::start(START);
        let node = common::create_node(&unique(label));
        Self { node, waitset: default_waitset(node), _clock: clock }
    }
}

impl Drop for SimNode {
    fn drop(&mut self) {
        destroy(self.node);
    }
}

/// The context of a timer: the time and missed periods of each call, what each call
/// does, and how often the registration released it. Declared before the node, so it
/// outlives the registration.
struct Probe<'a> {
    calls: RefCell<Vec<(i64, u64)>>,
    action: RefCell<Box<dyn FnMut(usize) + 'a>>,
    /// The call that ends the run, counted from one.
    stop_at_call: usize,
    releases: Cell<u32>,
}

impl<'a> Probe<'a> {
    fn new() -> Self {
        Self::with(|_| {})
    }

    /// A probe whose call `index`, from zero, runs `action(index)`.
    fn with(action: impl FnMut(usize) + 'a) -> Self {
        Self {
            calls: RefCell::new(Vec::new()),
            action: RefCell::new(Box::new(action)),
            stop_at_call: usize::MAX,
            releases: Cell::new(0),
        }
    }

    fn stopping_at_call(mut self, call: usize) -> Self {
        self.stop_at_call = call;
        self
    }

    fn context(&self) -> *mut c_void {
        ptr::from_ref(self).cast_mut().cast()
    }

    fn calls(&self) -> Vec<(i64, u64)> {
        self.calls.borrow().clone()
    }
}

extern "C" fn tick(missed: u64, context: *mut c_void) -> HyphaCallbackResult {
    // SAFETY: every context of `tick` is a `Probe` that outlives its registration.
    let probe = unsafe { &*context.cast::<Probe<'_>>() };
    let index = {
        let mut calls = probe.calls.borrow_mut();
        let index = calls.len();
        calls.push((hypha_clock_now_nanos(), missed));
        index
    };
    (probe.action.borrow_mut())(index);
    let last = index.saturating_add(1) >= probe.stop_at_call;
    let action = if last { HYPHA_ACTION_STOP } else { HYPHA_ACTION_CONTINUE };
    HyphaCallbackResult { action, status: 0 }
}

extern "C" fn release(context: *mut c_void) {
    // SAFETY: as in `tick`.
    let probe = unsafe { &*context.cast::<Probe<'_>>() };
    probe.releases.set(probe.releases.get().saturating_add(1));
}

extern "C" fn ignore(_missed: u64, _context: *mut c_void) -> HyphaCallbackResult {
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

extern "C" fn ignore_delivery(
    _delivery: *const HyphaDelivery,
    _context: *mut c_void,
) -> HyphaCallbackResult {
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

extern "C" fn ignore_step(
    _image: *const HyphaCycleImage,
    _time: *const HyphaCycleTime,
    _emission: *mut HyphaCycleEmission,
    _context: *mut c_void,
) -> HyphaCallbackResult {
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

fn attach_record(
    waitset: *mut HyphaWaitSet,
    record: &HyphaAttachment,
) -> (HyphaStatus, HyphaAttachmentId) {
    let mut id = 0;
    // SAFETY: the `WaitSet` is live or null; the record and `id` are borrowed for the call.
    let status = unsafe { hypha_waitset_attach(waitset, record, &raw mut id) };
    (status, id)
}

fn attach_timer(
    waitset: *mut HyphaWaitSet,
    period_nanos: u64,
    probe: &Probe<'_>,
) -> (HyphaStatus, HyphaAttachmentId) {
    attach_record(waitset, &attach::timer(period_nanos, Some(tick), probe.context(), Some(release)))
}

fn attached_timer(waitset: *mut HyphaWaitSet, probe: &Probe<'_>) -> HyphaAttachmentId {
    let (status, id) = attach_timer(waitset, PERIOD_NANOS, probe);
    assert_eq!(status, HyphaStatus::Ok);
    id
}

fn detach(waitset: *mut HyphaWaitSet, attachment: HyphaAttachmentId) -> HyphaStatus {
    // SAFETY: the `WaitSet` is live: its node is not destroyed yet.
    unsafe { hypha_waitset_detach(waitset, attachment) }
}

fn cancel(waitset: *mut HyphaWaitSet, timer: HyphaAttachmentId) -> HyphaStatus {
    // SAFETY: the `WaitSet` is null or live: its node is not destroyed yet.
    unsafe { hypha_waitset_timer_cancel(waitset, timer) }
}

fn reset(waitset: *mut HyphaWaitSet, timer: HyphaAttachmentId) -> HyphaStatus {
    // SAFETY: as in `cancel`.
    unsafe { hypha_waitset_timer_reset(waitset, timer) }
}

/// The status of the read, and the state it wrote or [`untouched`].
fn state(waitset: *const HyphaWaitSet, timer: HyphaAttachmentId) -> (HyphaStatus, HyphaTimerState) {
    let mut out = untouched();
    // SAFETY: as in `cancel`; `out` is borrowed for the call.
    let status = unsafe { hypha_waitset_timer_state(waitset, timer, &raw mut out) };
    (status, out)
}

/// A state no read writes: what `out` holds when a read refused.
fn untouched() -> HyphaTimerState {
    HyphaTimerState { canceled: true, period_nanos: -1, until_due_nanos: -1 }
}

fn running(until_due_nanos: i64) -> (HyphaStatus, HyphaTimerState) {
    (HyphaStatus::Ok, HyphaTimerState { canceled: false, period_nanos: PERIOD, until_due_nanos })
}

fn canceled() -> (HyphaStatus, HyphaTimerState) {
    let state = HyphaTimerState { canceled: true, period_nanos: PERIOD, until_due_nanos: i64::MAX };
    (HyphaStatus::Ok, state)
}

fn subscriber(node: *mut HyphaNode, label: &str) -> *mut HyphaSubscriber {
    let topic = format!("/{}", unique(label));
    // SAFETY: `node` is a live node a test has just created.
    unsafe { open_subscriber(node, &topic, SAMPLE_MD5, 4) }
}

fn destroy_subscriber(subscriber: *mut HyphaSubscriber) {
    // SAFETY: each test releases its subscriber handle once.
    unsafe { hypha_subscriber_destroy(subscriber) };
}

#[test]
fn a_canceled_timer_calls_nothing_while_time_advances_and_a_reset_restarts_it() {
    let probe = Probe::new();
    let sim = SimNode::new("timer_cancel");
    let timer = attached_timer(sim.waitset, &probe);
    assert_eq!(cancel(sim.waitset, timer), HyphaStatus::Ok);
    assert_eq!(cancel(sim.waitset, timer), HyphaStatus::Ok, "a cancel is idempotent");
    assert_eq!(state(sim.waitset, timer), canceled());
    for periods in 1..=5 {
        set_clock(START + periods * PERIOD);
        turn(sim.waitset);
    }
    assert_eq!(probe.calls(), [], "a canceled timer calls nothing");

    let reset_at = START + 5 * PERIOD + PERIOD / 2;
    set_clock(reset_at);
    assert_eq!(reset(sim.waitset, timer), HyphaStatus::Ok);
    assert_eq!(state(sim.waitset, timer), running(PERIOD));
    set_clock(reset_at + PERIOD - 1);
    turn(sim.waitset);
    assert_eq!(probe.calls(), []);
    set_clock(reset_at + PERIOD);
    turn(sim.waitset);
    // The canceled periods count as missed in no call.
    assert_eq!(probe.calls(), [(reset_at + PERIOD, 0)]);
}

#[test]
fn a_reset_moves_the_next_call_one_period_past_the_reset() {
    let probe = Probe::new();
    let sim = SimNode::new("timer_reset");
    let timer = attached_timer(sim.waitset, &probe);
    let reset_at = START + PERIOD / 2;
    set_clock(reset_at);
    turn(sim.waitset);
    assert_eq!(reset(sim.waitset, timer), HyphaStatus::Ok);

    // Without the reset the timer would be due here.
    set_clock(START + PERIOD);
    turn(sim.waitset);
    assert_eq!(probe.calls(), []);
    assert_eq!(state(sim.waitset, timer), running(PERIOD / 2));
    set_clock(reset_at + PERIOD);
    turn(sim.waitset);
    assert_eq!(probe.calls(), [(reset_at + PERIOD, 0)]);
    // The periods go on from the reset. A node that follows `/clock` runs each period at
    // its deadline (#72): a turn a period and a half late runs the two it passed.
    let late = reset_at + 3 * PERIOD + PERIOD / 2;
    set_clock(late);
    turn(sim.waitset);
    assert_eq!(
        probe.calls(),
        [(reset_at + PERIOD, 0), (reset_at + 2 * PERIOD, 0), (reset_at + 3 * PERIOD, 0)]
    );
}

#[test]
fn the_time_until_a_call_counts_down_on_the_timer_clock_and_is_zero_once_due() {
    let probe = Probe::new();
    let sim = SimNode::new("timer_until_due");
    let timer = attached_timer(sim.waitset, &probe);
    assert_eq!(state(sim.waitset, timer), running(PERIOD));
    set_clock(START + PERIOD / 4);
    assert_eq!(state(sim.waitset, timer), running(PERIOD - PERIOD / 4), "read at the call");
    set_clock(START + 2 * PERIOD);
    assert_eq!(state(sim.waitset, timer), running(0), "due, and no turn ran");
    turn(sim.waitset);
    // Each period at its deadline, on a node that follows `/clock` (#72).
    assert_eq!(probe.calls(), [(START + PERIOD, 0), (START + 2 * PERIOD, 0)]);
    assert_eq!(state(sim.waitset, timer), running(PERIOD));
}

#[test]
fn a_cancel_from_a_callback_holds_back_a_call_due_later_in_the_same_turn() {
    let waitset = Cell::new(ptr::null_mut());
    let later = Cell::new(0);
    let answers = RefCell::new(Vec::new());
    let canceled_timer = Probe::new();
    let canceler = Probe::with(|index| {
        if index == 0 {
            answers.borrow_mut().push(cancel(waitset.get(), later.get()));
        }
    });
    let sim = SimNode::new("timer_cancel_in_turn");
    waitset.set(sim.waitset);
    // Attached in this order, both due at the same time: the canceler runs first.
    attached_timer(sim.waitset, &canceler);
    later.set(attached_timer(sim.waitset, &canceled_timer));
    set_clock(START + PERIOD);
    turn(sim.waitset);
    assert_eq!(answers.borrow().as_slice(), [HyphaStatus::Ok]);
    assert_eq!(canceler.calls(), [(START + PERIOD, 0)]);
    assert_eq!(canceled_timer.calls(), [], "the cancel held back the call due in its turn");
    set_clock(START + 2 * PERIOD);
    turn(sim.waitset);
    assert_eq!(canceled_timer.calls(), []);
    assert_eq!(state(sim.waitset, later.get()), canceled());
}

#[test]
fn a_reset_from_a_callback_moves_a_call_due_later_in_the_same_turn_a_period_on() {
    let waitset = Cell::new(ptr::null_mut());
    let later = Cell::new(0);
    let answers = RefCell::new(Vec::new());
    let reset_timer = Probe::new();
    let resetter = Probe::with(|index| {
        if index == 0 {
            answers.borrow_mut().push(reset(waitset.get(), later.get()));
        }
    });
    let sim = SimNode::new("timer_reset_in_turn");
    waitset.set(sim.waitset);
    attached_timer(sim.waitset, &resetter);
    later.set(attached_timer(sim.waitset, &reset_timer));
    set_clock(START + PERIOD);
    turn(sim.waitset);
    assert_eq!(answers.borrow().as_slice(), [HyphaStatus::Ok]);
    assert_eq!(resetter.calls().len(), 1);
    assert_eq!(reset_timer.calls(), [], "the reset held back the call due in its turn");
    assert_eq!(state(sim.waitset, later.get()), running(PERIOD));
    set_clock(START + 2 * PERIOD - 1);
    turn(sim.waitset);
    assert_eq!(reset_timer.calls(), []);
    set_clock(START + 2 * PERIOD);
    turn(sim.waitset);
    assert_eq!(reset_timer.calls(), [(START + 2 * PERIOD, 0)]);
}

#[test]
fn a_timer_attached_in_a_callback_fires_a_period_later_and_is_canceled_reset_and_detached() {
    let waitset = Cell::new(ptr::null_mut());
    let attached = Cell::new((HyphaStatus::Backend, 0));
    let seen = RefCell::new(Vec::new());
    let created = Probe::new();
    let creator = Probe::with(|index| {
        if index != 0 {
            return;
        }
        let set = waitset.get();
        let (status, id) = attach_timer(set, PERIOD_NANOS, &created);
        attached.set((status, id));
        let mut seen = seen.borrow_mut();
        seen.push(state(set, id));
        seen.push((cancel(set, id), untouched()));
        seen.push(state(set, id));
        seen.push((reset(set, id), untouched()));
        seen.push(state(set, id));
    });
    let sim = SimNode::new("timer_create_in_callback");
    waitset.set(sim.waitset);
    attached_timer(sim.waitset, &creator);
    set_clock(START + PERIOD);
    turn(sim.waitset);

    let (status, timer) = attached.get();
    assert_eq!(status, HyphaStatus::Ok, "a callback may attach a timer");
    assert!(timer >= FIRST_DISPATCHER_ID, "id {timer:#x} is not above the native range");
    let ok = (HyphaStatus::Ok, untouched());
    assert_eq!(seen.borrow().as_slice(), [running(PERIOD), ok, canceled(), ok, running(PERIOD)]);
    assert_eq!(state(sim.waitset, timer), running(PERIOD), "after the run");
    set_clock(START + 2 * PERIOD - 1);
    turn(sim.waitset);
    assert_eq!(created.calls(), []);
    set_clock(START + 2 * PERIOD);
    turn(sim.waitset);
    assert_eq!(created.calls(), [(START + 2 * PERIOD, 0)], "one period after the attach");

    // Later runs: a cancel and a reset between them, then a detach.
    assert_eq!(cancel(sim.waitset, timer), HyphaStatus::Ok);
    set_clock(START + 3 * PERIOD);
    turn(sim.waitset);
    assert_eq!(created.calls().len(), 1);
    assert_eq!(reset(sim.waitset, timer), HyphaStatus::Ok);
    set_clock(START + 4 * PERIOD);
    turn(sim.waitset);
    assert_eq!(created.calls(), [(START + 2 * PERIOD, 0), (START + 4 * PERIOD, 0)]);
    assert_eq!(detach(sim.waitset, timer), HyphaStatus::Ok);
    assert_eq!(created.releases.get(), 1);
    assert_eq!(state(sim.waitset, timer), (HyphaStatus::InvalidArgument, untouched()));
    assert_eq!(cancel(sim.waitset, timer), HyphaStatus::InvalidArgument);
    assert_eq!(detach(sim.waitset, timer), HyphaStatus::InvalidArgument);
    set_clock(START + 5 * PERIOD);
    turn(sim.waitset);
    assert_eq!(created.calls().len(), 2);
    assert_eq!(created.releases.get(), 1);
}

#[test]
fn a_timer_attached_and_detached_in_one_callback_never_calls_and_is_released_after_the_run() {
    let waitset = Cell::new(ptr::null_mut());
    let answers = RefCell::new(Vec::new());
    let created = Probe::new();
    let creator = Probe::with(|index| {
        if index != 0 {
            return;
        }
        let set = waitset.get();
        let (status, id) = attach_timer(set, PERIOD_NANOS, &created);
        let detached = detach(set, id);
        // Until the run returns, a detached registration is still known.
        answers.borrow_mut().extend([status, detached, state(set, id).0]);
    });
    let sim = SimNode::new("timer_create_detach");
    waitset.set(sim.waitset);
    attached_timer(sim.waitset, &creator);
    set_clock(START + PERIOD);
    turn(sim.waitset);
    assert_eq!(answers.borrow().as_slice(), [HyphaStatus::Ok; 3]);
    assert_eq!(created.releases.get(), 1, "released when the run returned");
    set_clock(START + 3 * PERIOD);
    turn(sim.waitset);
    assert_eq!(created.calls(), []);
    assert_eq!(created.releases.get(), 1);
}

#[test]
fn a_timer_attached_in_a_callback_is_released_by_the_destruction_of_its_node() {
    let waitset = Cell::new(ptr::null_mut());
    let attached = Cell::new(HyphaStatus::Backend);
    let created = Probe::new();
    {
        let creator = Probe::with(|index| {
            if index == 0 {
                attached.set(attach_timer(waitset.get(), PERIOD_NANOS, &created).0);
            }
        });
        let sim = SimNode::new("timer_create_destroy");
        waitset.set(sim.waitset);
        attached_timer(sim.waitset, &creator);
        set_clock(START + PERIOD);
        turn(sim.waitset);
        assert_eq!(attached.get(), HyphaStatus::Ok);
        assert_eq!(created.releases.get(), 0);
    }
    assert_eq!(created.releases.get(), 1);
    assert_eq!(created.calls(), []);
}

#[test]
fn a_canceled_timer_still_ends_the_run_when_time_goes_back() {
    let probe = Probe::new();
    let sim = SimNode::new("timer_cancel_rewind");
    let timer = attached_timer(sim.waitset, &probe);
    set_clock(START + PERIOD);
    turn(sim.waitset);
    assert_eq!(cancel(sim.waitset, timer), HyphaStatus::Ok);
    // Forward while canceled: no call, and the timer follows its clock.
    set_clock(START + 3 * PERIOD);
    turn(sim.waitset);
    set_clock(START + 2 * PERIOD);
    let report = run(sim.waitset, 1_000_000);
    assert_eq!(report.status, HyphaStatus::ClockRewound as i32, "nothing rewinds in place");
    assert_eq!(probe.calls(), [(START + PERIOD, 0)]);
}

#[test]
fn a_reset_after_time_went_back_ends_the_run_instead_of_restarting_in_place() {
    let probe = Probe::new();
    let sim = SimNode::new("timer_reset_rewind");
    let timer = attached_timer(sim.waitset, &probe);
    set_clock(START + PERIOD);
    turn(sim.waitset);
    // Time goes back between runs, and the reset reads the earlier time.
    set_clock(START);
    assert_eq!(reset(sim.waitset, timer), HyphaStatus::Ok);
    let report = run(sim.waitset, 1_000_000);
    assert_eq!(report.status, HyphaStatus::ClockRewound as i32, "nothing rewinds in place");
    assert_eq!(probe.calls(), [(START + PERIOD, 0)]);
}

#[test]
fn a_callback_may_attach_no_other_kind_and_no_timer_on_a_clock_driven_from_outside() {
    let waitset = Cell::new(ptr::null_mut());
    let source = Cell::new(ptr::null_mut());
    let answers = RefCell::new(Vec::new());
    let timer = Probe::new();
    let attacher = Probe::with(|index| {
        if index != 0 {
            return;
        }
        let record =
            attach::subscription(source.get(), Some(ignore_delivery), ptr::null_mut(), None);
        answers.borrow_mut().push(attach_record(waitset.get(), &record));
        answers.borrow_mut().push(attach_timer(waitset.get(), PERIOD_NANOS, &timer));
    });

    // On its own simulated clock a timer is accepted; a subscription is not.
    let sim = SimNode::new("attach_kind_in_callback");
    source.set(subscriber(sim.node, "attach_kind"));
    waitset.set(sim.waitset);
    attached_timer(sim.waitset, &attacher);
    set_clock(START + PERIOD);
    turn(sim.waitset);
    let seen = answers.take();
    assert_eq!(seen.first(), Some(&(HyphaStatus::InvalidState, 0)), "{seen:?}");
    assert_eq!(seen.get(1).map(|answer| answer.0), Some(HyphaStatus::Ok), "{seen:?}");
    destroy_subscriber(source.get());
    drop(sim);
    assert_eq!(timer.releases.get(), 1, "the accepted timer went with its node");

    // A clock driven from outside the node needs a native poll, which the run holds.
    let refused = Probe::new();
    let external = Probe::with(|index| {
        if index == 0 {
            answers.borrow_mut().push(attach_timer(waitset.get(), PERIOD_NANOS, &refused));
        }
    });
    let sim = SimNode::on_external_clock("attach_external_clock");
    waitset.set(sim.waitset);
    attached_timer(sim.waitset, &external);
    set_clock(START + PERIOD);
    turn(sim.waitset);
    assert_eq!(external.calls().len(), 1);
    assert_eq!(answers.take(), [(HyphaStatus::InvalidState, 0)]);
    drop(sim);
    assert_eq!(refused.releases.get(), 0, "a refused attach takes no context");
}

#[test]
fn a_cycle_timer_a_subscription_an_unknown_id_and_a_null_handle_are_refused() {
    let node = common::create_node(&unique("timer_refusals"));
    let waitset = default_waitset(node);
    let attach_plain = || {
        let record = attach::timer(PERIOD_NANOS, Some(ignore), ptr::null_mut(), None);
        let (status, id) = attach_record(waitset, &record);
        assert_eq!(status, HyphaStatus::Ok);
        id
    };
    // A committed rate cycle takes the one id between two plain timers.
    let before = attach_plain();
    let name = "timer_refusals_cycle";
    let mut cycle = ptr::null_mut();
    // SAFETY: the `WaitSet` is live, the name is borrowed for the call and `cycle` writable.
    let created = unsafe {
        hypha_cycle_create(
            waitset,
            name.as_ptr().cast(),
            name.len(),
            PERIOD_NANOS,
            Some(ignore_step),
            ptr::null_mut(),
            None,
            &raw mut cycle,
        )
    };
    assert_eq!(created, HyphaStatus::Ok);
    // SAFETY: `cycle` is the live cycle just created on this `WaitSet`.
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);
    let after = attach_plain();
    assert_eq!(after.checked_sub(before), Some(2), "the cycle's timer lies between");
    let cycle_timer = before.saturating_add(1);

    let source = subscriber(node, "timer_refusals");
    let record = attach::subscription(source, Some(ignore_delivery), ptr::null_mut(), None);
    let (status, subscription) = attach_record(waitset, &record);
    assert_eq!(status, HyphaStatus::Ok);

    for id in [cycle_timer, subscription, 0, after.saturating_add(100), FIRST_DISPATCHER_ID] {
        assert_eq!(cancel(waitset, id), HyphaStatus::InvalidArgument, "cancel {id}");
        assert_eq!(reset(waitset, id), HyphaStatus::InvalidArgument, "reset {id}");
        assert_eq!(state(waitset, id), (HyphaStatus::InvalidArgument, untouched()), "state {id}");
    }
    assert_eq!(cancel(ptr::null_mut(), before), HyphaStatus::InvalidArgument);
    assert_eq!(reset(ptr::null_mut(), before), HyphaStatus::InvalidArgument);
    assert_eq!(state(ptr::null(), before), (HyphaStatus::InvalidArgument, untouched()));
    // SAFETY: the `WaitSet` is live; a null `out` is the case under test.
    let no_out = unsafe { hypha_waitset_timer_state(waitset, before, ptr::null_mut()) };
    assert_eq!(no_out, HyphaStatus::InvalidArgument);
    assert_eq!(state(waitset, before).0, HyphaStatus::Ok);

    destroy_subscriber(source);
    destroy(node);
}

#[test]
fn another_thread_is_refused_while_the_waitset_runs_and_changes_nothing() {
    let waitset = Cell::new(ptr::null_mut());
    let own = Cell::new(0);
    let answers = RefCell::new(Vec::new());
    let probe = Probe::with(|index| {
        if index != 0 {
            return;
        }
        let address = waitset.get() as usize;
        let timer = own.get();
        // The run is in this callback until the other thread is joined.
        let other = std::thread::spawn(move || {
            let set = address as *mut HyphaWaitSet;
            let record = attach::timer(PERIOD_NANOS, Some(ignore), ptr::null_mut(), None);
            let attached = attach_record(set, &record).0;
            [cancel(set, timer), reset(set, timer), state(set, timer).0, attached]
        });
        answers.borrow_mut().extend(other.join().unwrap_or([HyphaStatus::Backend; 4]));
        // The run's own thread is answered.
        answers.borrow_mut().push(state(waitset.get(), timer).0);
    })
    .stopping_at_call(1);
    let node = common::create_node(&unique("timer_other_thread"));
    let set = default_waitset(node);
    waitset.set(set);
    own.set(attach_timer(set, 1_000_000, &probe).1);
    let report = run(set, HANG_GUARD_NANOS);
    assert_eq!((report.reason, report.status), (HYPHA_RUN_REASON_STOPPED, 0), "{report:?}");
    let refused = HyphaStatus::InvalidState;
    assert_eq!(answers.borrow().as_slice(), [refused, refused, refused, refused, HyphaStatus::Ok]);
    let (status, after) = state(set, own.get());
    assert_eq!(
        (status, after.canceled),
        (HyphaStatus::Ok, false),
        "the refused cancel did nothing"
    );
    destroy(node);
}

#[test]
fn a_host_timer_attached_in_a_callback_fires_and_is_due_within_its_period() {
    const HOST_PERIOD: u64 = 1_000_000;
    let waitset = Cell::new(ptr::null_mut());
    let attached = Cell::new((HyphaStatus::Backend, 0));
    let until_due = Cell::new(-1);
    let created = Probe::new().stopping_at_call(1);
    let creator = Probe::with(|index| {
        if index == 0 {
            let answer = attach_timer(waitset.get(), HOST_PERIOD, &created);
            attached.set(answer);
            until_due.set(state(waitset.get(), answer.1).1.until_due_nanos);
        }
    });
    let node = common::create_node(&unique("timer_host_create"));
    let set = default_waitset(node);
    waitset.set(set);
    assert_eq!(attach_timer(set, HOST_PERIOD, &creator).0, HyphaStatus::Ok);
    // The created timer's first call ends the run.
    let report = run(set, HANG_GUARD_NANOS);
    assert_eq!((report.reason, report.status), (HYPHA_RUN_REASON_STOPPED, 0), "{report:?}");
    let (status, timer) = attached.get();
    assert_eq!(status, HyphaStatus::Ok);
    assert!(timer >= FIRST_DISPATCHER_ID, "id {timer:#x}");
    assert_eq!(created.calls().len(), 1);
    let host_period = i64::try_from(HOST_PERIOD).expect("period");
    assert!((0..=host_period).contains(&until_due.get()), "until due {}", until_due.get());
    destroy(node);
    assert_eq!(created.releases.get(), 1);
}
