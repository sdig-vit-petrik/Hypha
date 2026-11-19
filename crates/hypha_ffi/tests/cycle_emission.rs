//! Single-copy emission: a reserved message is the transport loan, with no staging copy.
#![allow(clippy::expect_used, reason = "a failed test setup should stop the test with its message")]

mod common;
use crate::common::unique;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::ffi::{CString, c_void};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};

use hypha_ffi::waitset::{HYPHA_ACTION_CONTINUE, HYPHA_ACTION_FAIL, HyphaCallbackResult};
use hypha_ffi::*;

/// Counts the allocations made by the thread that has counting switched on.
struct CountingAllocator;

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

fn count_allocation() {
    let _ = COUNTING.try_with(|on| {
        if on.get() {
            let _ = ALLOCATIONS.try_with(|count| count.set(count.get().saturating_add(1)));
        }
    });
}

// SAFETY: every call forwards to the system allocator with the same arguments.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count_allocation();
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        count_allocation();
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn allocations_during(work: impl FnOnce()) -> usize {
    ALLOCATIONS.with(|count| count.set(0));
    COUNTING.with(|on| on.set(true));
    work();
    COUNTING.with(|on| on.set(false));
    ALLOCATIONS.with(Cell::get)
}

struct StepPlan {
    /// The messages to stage, in order: output index and payload.
    messages: Vec<(u32, Vec<u8>)>,
    /// Off makes the step stage nothing, so a tick differs only by its messages.
    active: AtomicBool,
    action: u32,
    status: i32,
}

/// The `(callback_start_nanos, message_stamp_nanos)` of every step `plan_step` ran.
static STEP_TIMES: std::sync::Mutex<Vec<(i64, i64)>> = std::sync::Mutex::new(Vec::new());

extern "C" fn plan_step(
    _image: *const HyphaCycleImage,
    time: *const HyphaCycleTime,
    emission: *mut HyphaCycleEmission,
    context: *mut c_void,
) -> HyphaCallbackResult {
    let plan = unsafe { &*context.cast::<StepPlan>() };
    let (callback_start, stamp) =
        unsafe { ((*time).callback_start_nanos, (*time).message_stamp_nanos) };
    STEP_TIMES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push((callback_start, stamp));
    if plan.active.load(Ordering::SeqCst) {
        for (output, bytes) in &plan.messages {
            assert_eq!(unsafe { common::emit_message(emission, *output, bytes) }, HyphaStatus::Ok);
        }
    }
    HyphaCallbackResult { action: plan.action, status: plan.status }
}

struct Harness {
    node: *mut HyphaNode,
    waitset: *mut HyphaWaitSet,
}

const MD5: &str = "d9d213e8c146065ec958333c617eb1a6";

impl Harness {
    fn new(label: &str) -> Self {
        let node = common::create_node(&unique(label));
        let mut waitset: *mut HyphaWaitSet = ptr::null_mut();
        assert_eq!(unsafe { hypha_node_default_waitset(node, &raw mut waitset) }, HyphaStatus::Ok);
        Self { node, waitset }
    }

    /// A publisher on a unique topic that may hold `loaned_samples` loans at once, and
    /// a subscriber of that topic that counts the sequence gaps it sees.
    fn publisher(&self, loaned_samples: u32) -> (*mut HyphaPublisher, *mut HyphaSubscriber) {
        let topic = CString::new(format!("/{}", unique("emit_out"))).expect("topic");
        let data_type = CString::new("std_msgs/Emit").expect("type");
        let md5 = CString::new(MD5).expect("md5");
        let qos =
            HyphaPublisherOptions { latch: 0, loaned_samples, qos: crate::common::test_qos() };
        let mut publisher = ptr::null_mut();
        assert_eq!(
            unsafe {
                hypha_publish_with_qos(
                    self.node,
                    topic.as_ptr(),
                    topic.as_bytes().len(),
                    data_type.as_ptr(),
                    data_type.as_bytes().len(),
                    md5.as_ptr(),
                    md5.as_bytes().len(),
                    c"".as_ptr(),
                    0,
                    &raw const qos,
                    &raw mut publisher,
                )
            },
            HyphaStatus::Ok
        );
        let sub_qos = HyphaSubscriberOptions {
            depth: 16,
            no_publisher_warning_after_nanos: 0,
            qos: hypha_ffi::HyphaQos::default(),
        };
        let mut subscriber = ptr::null_mut();
        assert_eq!(
            unsafe {
                hypha_subscribe_typed(
                    self.node,
                    topic.as_ptr(),
                    topic.as_bytes().len(),
                    ptr::null(),
                    0,
                    md5.as_ptr(),
                    md5.as_bytes().len(),
                    std::ptr::null(),
                    0,
                    &raw const sub_qos,
                    &raw mut subscriber,
                )
            },
            HyphaStatus::Ok
        );
        (publisher, subscriber)
    }

    fn create_cycle(&self, plan: &StepPlan) -> *mut HyphaCycle {
        let name = CString::new(unique("emit_cycle")).expect("cycle name");
        let mut cycle = ptr::null_mut();
        assert_eq!(
            unsafe {
                hypha_cycle_create(
                    self.waitset,
                    name.as_ptr(),
                    name.as_bytes().len(),
                    1_000_000,
                    Some(plan_step),
                    ptr::from_ref(plan).cast_mut().cast::<c_void>(),
                    None,
                    &raw mut cycle,
                )
            },
            HyphaStatus::Ok
        );
        cycle
    }

    /// A committed cycle whose outputs are `(publisher, policy, messages per step)`; the
    /// cycle owns the publishers.
    fn cycle(
        &self,
        plan: &StepPlan,
        outputs: &[(*mut HyphaPublisher, HyphaCycleOutputFailure, u32)],
    ) -> *mut HyphaCycle {
        let cycle = self.create_cycle(plan);
        for (publisher, policy, per_step) in outputs {
            let mut index = 0;
            assert_eq!(
                unsafe {
                    hypha_cycle_bind_output(
                        cycle,
                        *publisher,
                        *policy as u32,
                        *per_step,
                        &raw mut index,
                    )
                },
                HyphaStatus::Ok
            );
        }
        assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);
        cycle
    }

    fn destroy(self, cycle: *mut HyphaCycle, subscribers: &[*mut HyphaSubscriber]) {
        assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
        for subscriber in subscribers {
            unsafe { hypha_subscriber_destroy(*subscriber) };
        }
        unsafe {
            hypha_node_destroy(self.node);
        }
    }
}

fn tick(cycle: *mut HyphaCycle) -> HyphaCallbackResult {
    // SAFETY: each test passes a live cycle handle and runs no `WaitSet` meanwhile.
    unsafe { cycle_timer_tick(0, cycle.cast::<c_void>()) }
}

fn health(cycle: *const HyphaCycle) -> HyphaCycleHealth {
    let mut health = HyphaCycleHealth::default();
    assert_eq!(unsafe { hypha_cycle_health(cycle, &raw mut health) }, HyphaStatus::Ok);
    health
}

fn drain(subscriber: *mut HyphaSubscriber) -> usize {
    std::iter::from_fn(|| unsafe { common::replay::receive_payload(subscriber) }).count()
}

fn missing(subscriber: *const HyphaSubscriber) -> u64 {
    let mut missing = 0;
    assert_eq!(
        unsafe { hypha_subscriber_missing_sequences(subscriber, &raw mut missing) },
        HyphaStatus::Ok
    );
    missing
}

/// Hold one loan of `publisher` outside any step; the loan takes a sequence number.
fn hold_loan(publisher: *mut HyphaPublisher) -> *mut HyphaLoan {
    let mut loan = ptr::null_mut();
    let mut bytes = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_publisher_loan_frame(publisher, 8, &raw mut loan, &raw mut bytes) },
        HyphaStatus::Ok
    );
    loan
}

#[test]
#[allow(clippy::too_many_lines)]
fn a_drop_pool_exhausted_output_drops_only_its_own_message() {
    let harness = Harness::new("emit_pool");
    let (drop_publisher, drop_subscriber) = harness.publisher(1);
    let (fail_publisher, fail_subscriber) = harness.publisher(1);
    let plan = StepPlan {
        messages: vec![(0, vec![1]), (1, vec![2, 2])],
        active: AtomicBool::new(true),
        action: HYPHA_ACTION_CONTINUE,
        status: 0,
    };
    let cycle = harness.cycle(
        &plan,
        &[
            (drop_publisher, HyphaCycleOutputFailure::DropPoolExhausted, 1),
            (fail_publisher, HyphaCycleOutputFailure::FailRun, 1),
        ],
    );

    // A first tick gives each subscriber the sequence it counts gaps from.
    assert_eq!(tick(cycle).action, HYPHA_ACTION_CONTINUE);
    assert_eq!(drain(drop_subscriber), 1);
    assert_eq!(drain(fail_subscriber), 1);

    // Hold the only loan of the first output. Its reserve then finds no free loan.
    let held = hold_loan(drop_publisher);
    let result = tick(cycle);
    assert_eq!(result.action, HYPHA_ACTION_CONTINUE);
    assert_eq!(result.status, 0);
    let health = health(cycle);
    assert_eq!(health.iterations, 2);
    assert_eq!(health.output_attempts, 4);
    assert_eq!(health.output_pool_drops, 1);
    assert_eq!(health.output_failures, 1);
    assert_eq!(health.output_successes, 3);
    assert_eq!(drain(drop_subscriber), 0, "the dropped message was not sent");
    assert_eq!(drain(fail_subscriber), 1);

    // The dropped message took a sequence number, a sample lost to a full pool, so the next
    // frame of that output arrives one value on. The held loan, given up unsent, took none.
    unsafe { hypha_loan_drop(held) };
    assert_eq!(tick(cycle).action, HYPHA_ACTION_CONTINUE);
    assert_eq!(drain(drop_subscriber), 1);
    assert_eq!(drain(fail_subscriber), 1);
    assert_eq!(missing(drop_subscriber), 1);
    assert_eq!(missing(fail_subscriber), 0);

    harness.destroy(cycle, &[drop_subscriber, fail_subscriber]);
}

#[test]
fn a_failed_step_releases_its_loans_and_sends_nothing() {
    let harness = Harness::new("emit_fail");
    let (publisher, subscriber) = harness.publisher(2);
    let plan = StepPlan {
        messages: vec![(0, vec![1]), (0, vec![2])],
        active: AtomicBool::new(true),
        action: HYPHA_ACTION_FAIL,
        status: 9,
    };
    let cycle = harness.cycle(&plan, &[(publisher, HyphaCycleOutputFailure::FailRun, 2)]);

    let result = tick(cycle);
    assert_eq!(result.action, HYPHA_ACTION_FAIL);
    assert_eq!(result.status, 9);
    let health = health(cycle);
    assert_eq!(health.iterations, 0);
    assert_eq!(health.output_attempts, 0);
    assert_eq!(drain(subscriber), 0);

    // Both loans came back: the publisher can lend its full limit again.
    let first = hold_loan(publisher);
    let second = hold_loan(publisher);
    unsafe {
        hypha_loan_drop(first);
        hypha_loan_drop(second);
    }

    harness.destroy(cycle, &[subscriber]);
}

#[test]
fn a_fail_without_a_status_is_reported_as_backend() {
    let harness = Harness::new("emit_fail_zero");
    let (publisher, subscriber) = harness.publisher(2);
    let plan = StepPlan {
        messages: vec![(0, vec![1])],
        active: AtomicBool::new(true),
        action: HYPHA_ACTION_FAIL,
        status: 0,
    };
    let cycle = harness.cycle(&plan, &[(publisher, HyphaCycleOutputFailure::FailRun, 2)]);

    let result = tick(cycle);
    assert_eq!(result.action, HYPHA_ACTION_FAIL);
    assert_eq!(result.status, HyphaStatus::Backend as i32);
    assert_eq!(health(cycle).iterations, 0);
    assert_eq!(drain(subscriber), 0);

    harness.destroy(cycle, &[subscriber]);
}

#[test]
fn messages_per_step_of_three_hold_three_loans_and_all_are_sent() {
    let harness = Harness::new("emit_three");
    let (publisher, subscriber) = harness.publisher(3);
    let plan = StepPlan {
        messages: vec![(0, vec![1]), (0, vec![2, 2]), (0, vec![3, 3, 3])],
        active: AtomicBool::new(true),
        action: HYPHA_ACTION_CONTINUE,
        status: 0,
    };
    let cycle = harness.cycle(&plan, &[(publisher, HyphaCycleOutputFailure::FailRun, 3)]);

    let result = tick(cycle);
    assert_eq!(result.action, HYPHA_ACTION_CONTINUE);
    let health = health(cycle);
    assert_eq!(health.output_successes, 3);
    assert_eq!(health.output_pool_drops, 0);
    assert_eq!(drain(subscriber), 3);
    assert_eq!(missing(subscriber), 0);
    harness.destroy(cycle, &[subscriber]);
}

#[test]
fn an_output_refuses_a_publisher_that_holds_fewer_loans_than_its_messages() {
    let harness = Harness::new("emit_short");
    let (publisher, subscriber) = harness.publisher(2);
    let plan = StepPlan {
        messages: Vec::new(),
        active: AtomicBool::new(false),
        action: HYPHA_ACTION_CONTINUE,
        status: 0,
    };
    let cycle = harness.create_cycle(&plan);
    let mut index = 0;
    assert_eq!(
        unsafe {
            hypha_cycle_bind_output(
                cycle,
                publisher,
                HyphaCycleOutputFailure::FailRun as u32,
                3,
                &raw mut index,
            )
        },
        HyphaStatus::ResourceLimit
    );
    // A refused bind leaves the publisher with the caller.
    unsafe { hypha_publisher_destroy(publisher) };
    harness.destroy(cycle, &[subscriber]);
}

#[test]
fn a_step_sending_messages_allocates_no_more_than_a_step_sending_none() {
    let harness = Harness::new("emit_alloc");
    let (publisher, subscriber) = harness.publisher(2);
    let plan = StepPlan {
        messages: vec![(0, vec![7; 64]), (0, vec![8; 64])],
        active: AtomicBool::new(true),
        action: HYPHA_ACTION_CONTINUE,
        status: 0,
    };
    let cycle = harness.cycle(&plan, &[(publisher, HyphaCycleOutputFailure::FailRun, 2)]);

    // Warm-up grows every retained buffer to its steady-state size.
    for _ in 0..4 {
        assert_eq!(tick(cycle).action, HYPHA_ACTION_CONTINUE);
    }

    plan.active.store(false, Ordering::SeqCst);
    let without_messages = allocations_during(|| {
        assert_eq!(tick(cycle).action, HYPHA_ACTION_CONTINUE);
    });
    plan.active.store(true, Ordering::SeqCst);
    let with_messages = allocations_during(|| {
        assert_eq!(tick(cycle).action, HYPHA_ACTION_CONTINUE);
    });
    // iceoryx2 allocates one shared state per loan (`SampleMutUninit::new`); hypha adds
    // none, so two messages add at most two.
    let added = with_messages.saturating_sub(without_messages);
    assert!(added <= 2, "two messages added {added} allocations; at most one per loan is allowed");
    // Warm-up sent 4 ticks of 2 messages, and the measured pair sent 0 and 2.
    assert_eq!(health(cycle).output_successes, 4 * 2 + 2);
    harness.destroy(cycle, &[subscriber]);
}

#[test]
fn a_steady_state_run_iteration_allocates_nothing() {
    use hypha_cycle::{
        BindingOptions, CaptureMode, CycleHistory, CycleRuntime, CycleTime, QueueOverflowPolicy,
    };
    use std::sync::{Arc, Mutex};

    let options = |mode| BindingOptions {
        mode,
        silence_nanos: 0,
        max_samples_per_iteration: 4,
        overflow_policy: QueueOverflowPolicy::Carry,
    };
    let shared = Arc::new(Mutex::new(CycleHistory::new(16).expect("history")));
    let other = Arc::new(Mutex::new(CycleHistory::new(16).expect("history")));
    let mut runtime = CycleRuntime::new();
    runtime.add_binding(Arc::clone(&shared), options(CaptureMode::Queued)).expect("binding");
    runtime.add_binding(Arc::clone(&shared), options(CaptureMode::Latest)).expect("binding");
    runtime.add_binding(Arc::clone(&other), options(CaptureMode::Queued)).expect("binding");
    runtime.add_disabled_binding(options(CaptureMode::Latest)).expect("binding");
    runtime.set_arrival_budget(Some(5)).expect("budget");

    let mut emissions: Vec<hypha_cycle::StagedMessage<()>> = Vec::new();
    let mut stamp = 0_u64;
    let mut tick = |runtime: &mut CycleRuntime, measure: bool| {
        let mut run = |runtime: &mut CycleRuntime| {
            // Once the ring buffer reaches capacity, pushing recycles evicted
            // samples and their payload storage without allocating.
            for history in [&shared, &other] {
                for _ in 0..3 {
                    stamp += 1_000_000;
                    history.lock().expect("history").push(&[1, 2, 3], stamp, stamp);
                }
            }
            let time = CycleTime {
                scheduled_nanos: i64::try_from(stamp).expect("stamp"),
                capture_steady_nanos: i64::try_from(stamp).expect("stamp"),
                ..CycleTime::default()
            };
            runtime
                .run_iteration_with_emissions(
                    time,
                    || 0,
                    Some(0),
                    &mut emissions,
                    |_, _, _, _| Ok(()),
                    |_, ()| Ok(()),
                )
                .expect("iteration");
        };
        if measure {
            allocations_during(|| run(runtime))
        } else {
            run(runtime);
            0
        }
    };
    for _ in 0..8 {
        tick(&mut runtime, false);
    }
    let allocations = tick(&mut runtime, true);
    assert_eq!(allocations, 0, "a steady-state tick allocated {allocations} times");
}

#[test]
fn a_committed_tick_with_an_input_allocates_nothing() {
    let harness = Harness::new("image_alloc");
    let plan = StepPlan {
        messages: Vec::new(),
        active: AtomicBool::new(false),
        action: HYPHA_ACTION_CONTINUE,
        status: 0,
    };
    let cycle = harness.create_cycle(&plan);
    let mut source = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_input_source_create(harness.waitset, ptr::null_mut(), &raw mut source) },
        HyphaStatus::Ok
    );
    let options = HyphaCycleBindingOptions {
        mode: HyphaCycleInputMode::Latest as u32,
        max_samples_per_iteration: 1,
        optional: 1,
    };
    let mut index = 0;
    assert_eq!(
        unsafe { hypha_cycle_bind_input(cycle, source, &raw const options, &raw mut index) },
        HyphaStatus::Ok
    );
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);
    assert_eq!(tick(cycle).action, HYPHA_ACTION_CONTINUE);
    let allocations = allocations_during(|| {
        assert_eq!(tick(cycle).action, HYPHA_ACTION_CONTINUE);
    });
    assert_eq!(allocations, 0, "a frozen image allocated {allocations} times");
    assert_eq!(unsafe { hypha_input_source_destroy(source) }, HyphaStatus::Ok);
    harness.destroy(cycle, &[]);
}

#[test]
fn a_loan_adds_one_allocation_and_a_pool_refusal_takes_a_sequence_but_an_unsent_loan_does_not() {
    let harness = Harness::new("encode_loan");
    let (publisher, subscriber) = harness.publisher(1);
    let send = |bytes: &[u8]| unsafe { common::replay::send_payload(publisher, bytes) };
    send(&42_u64.to_le_bytes());
    let allocations = allocations_during(|| send(&42_u64.to_le_bytes()));
    assert!(
        allocations <= 2,
        "a send added {allocations} allocations; only the loan handle and the transport may allocate"
    );
    // A loan given up unsent is released, so the only loan can be taken again.
    unsafe { hypha_loan_drop(hold_loan(publisher)) };
    send(&[1; 8]);
    let held = hold_loan(publisher);
    let mut loan = ptr::null_mut();
    let mut bytes = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_publisher_loan_frame(publisher, 8, &raw mut loan, &raw mut bytes) },
        HyphaStatus::PoolExhausted
    );
    unsafe { hypha_loan_drop(held) };
    send(&[2; 8]);
    // Sent 1, 2, 3, 5. The loan given up unsent takes no number, and the refused loan
    // (4) is a dropped sample, so the subscriber counts that one missing value.
    assert_eq!(drain(subscriber), 4);
    assert_eq!(missing(subscriber), 1);
    unsafe {
        hypha_publisher_destroy(publisher);
        hypha_subscriber_destroy(subscriber);
        hypha_node_destroy(harness.node);
    }
}

/// What a step that reads its own cycle sees.
struct Reentrant {
    cycle: std::sync::atomic::AtomicPtr<HyphaCycle>,
    statuses: std::sync::Mutex<Vec<HyphaStatus>>,
}

extern "C" fn reentrant_step(
    _image: *const HyphaCycleImage,
    _time: *const HyphaCycleTime,
    _emission: *mut HyphaCycleEmission,
    context: *mut c_void,
) -> HyphaCallbackResult {
    let plan = unsafe { &*context.cast::<Reentrant>() };
    let cycle = plan.cycle.load(Ordering::SeqCst);
    let mut health = HyphaCycleHealth::default();
    let mut binding = HyphaCycleBindingView::default();
    let statuses = unsafe {
        [
            hypha_cycle_health(cycle, &raw mut health),
            hypha_cycle_binding(cycle, 0, &raw mut binding),
            hypha_cycle_set_class_based(cycle, 1),
        ]
    };
    plan.statuses.lock().expect("statuses").extend(statuses);
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

#[test]
fn a_step_may_read_its_own_health_and_bindings_but_not_its_configuration() {
    let harness = Harness::new("reentrant");
    let plan = Reentrant {
        cycle: std::sync::atomic::AtomicPtr::new(ptr::null_mut()),
        statuses: std::sync::Mutex::new(Vec::new()),
    };
    let name = CString::new(unique("reentrant_cycle")).expect("cycle name");
    let mut cycle = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                harness.waitset,
                name.as_ptr(),
                name.as_bytes().len(),
                1_000_000,
                Some(reentrant_step),
                ptr::from_ref(&plan).cast_mut().cast::<c_void>(),
                None,
                &raw mut cycle,
            )
        },
        HyphaStatus::Ok
    );
    plan.cycle.store(cycle, Ordering::SeqCst);
    let mut source = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_input_source_create(harness.waitset, ptr::null_mut(), &raw mut source) },
        HyphaStatus::Ok
    );
    let options = HyphaCycleBindingOptions {
        mode: HyphaCycleInputMode::Latest as u32,
        max_samples_per_iteration: 1,
        optional: 1,
    };
    let mut index = 0;
    assert_eq!(
        unsafe { hypha_cycle_bind_input(cycle, source, &raw const options, &raw mut index) },
        HyphaStatus::Ok
    );
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);

    assert_eq!(tick(cycle).action, HYPHA_ACTION_CONTINUE);
    assert_eq!(
        *plan.statuses.lock().expect("statuses"),
        [HyphaStatus::Ok, HyphaStatus::Ok, HyphaStatus::InvalidState]
    );
    assert_eq!(health(cycle).iterations, 1);
    assert_eq!(unsafe { hypha_input_source_destroy(source) }, HyphaStatus::Ok);
    harness.destroy(cycle, &[]);
}

#[test]
fn a_step_stamp_is_the_same_reading_as_its_callback_time_and_its_frames_carry_it() {
    let harness = Harness::new("emit_one_reading");
    let (publisher, subscriber) = harness.publisher(2);
    let plan = StepPlan {
        messages: vec![(0, vec![1]), (0, vec![2])],
        active: AtomicBool::new(true),
        action: HYPHA_ACTION_CONTINUE,
        status: 0,
    };
    let cycle = harness.cycle(&plan, &[(publisher, HyphaCycleOutputFailure::FailRun, 2)]);
    let result = tick(cycle);
    assert_eq!(result.action, HYPHA_ACTION_CONTINUE);
    let times = STEP_TIMES.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
    let &(callback_start, stamp) = times.last().expect("the step ran");
    assert_eq!(stamp, callback_start, "one reading gives the step time and the stamp");
    let mut framework_stamps = Vec::new();
    while let Some((_, identity)) = unsafe { common::replay::receive_frame(subscriber) } {
        framework_stamps.push(identity.framework_sent_nanos);
    }
    assert_eq!(framework_stamps.len(), 2);
    let all_at_the_step_time = framework_stamps
        .iter()
        .all(|sent| i64::try_from(*sent).is_ok_and(|sent| sent == callback_start));
    assert!(all_at_the_step_time, "frames {framework_stamps:?}, step {callback_start}");
    harness.destroy(cycle, &[subscriber]);
}
