//! A managed cycle with idle inputs wakes its `WaitSet` on its own schedule, not by polling.
//! The simulated clock is process-wide, so every case holds [`SERIAL`].
#![allow(clippy::expect_used, reason = "a failed test setup should stop the test with its message")]

mod common;

use std::ffi::{CString, c_void};
use std::ptr;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use hypha_ffi::waitset::{
    HYPHA_ACTION_CONTINUE, HYPHA_RUN_REASON_DEADLINE, HyphaCallbackResult, HyphaWaitSetRunReport,
    hypha_waitset_run,
};
use hypha_ffi::*;

static SERIAL: Mutex<()> = Mutex::new(());
static NEXT: AtomicUsize = AtomicUsize::new(0);

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

extern "C" fn count_step(
    _image: *const HyphaCycleImage,
    _time: *const HyphaCycleTime,
    _emission: *mut HyphaCycleEmission,
    context: *mut c_void,
) -> HyphaCallbackResult {
    let steps = unsafe { &*context.cast::<AtomicU32>() };
    steps.fetch_add(1, Ordering::SeqCst);
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

fn create_node(parameters: Option<&str>) -> *mut HyphaNode {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let name = format!("wakeups_{}_{id}", std::process::id());
    let path = parameters.map(|text| {
        let path = std::env::temp_dir().join(format!("wakeups_{}_{id}.toml", std::process::id()));
        std::fs::write(&path, text).expect("parameter file");
        path
    });
    let node = common::create_node_with_file(&name, path.as_deref());
    if let Some(path) = path {
        std::fs::remove_file(path).unwrap_or_default();
    }
    node
}

fn sweeps(node: *const HyphaNode) -> u64 {
    common::node_metric(node, "node.sweeps").expect("the node emits node.sweeps")
}

/// A `WaitSet` on `node` with one 50 Hz cycle that has one idle input, with no publisher.
fn idle_50_hz_cycle(
    node: *mut HyphaNode,
    steps: &AtomicU32,
) -> (*mut HyphaWaitSet, *mut HyphaInputSource, *mut HyphaCycle) {
    let context = (&raw const *steps).cast_mut().cast();
    idle_50_hz_cycle_with(node, Some(count_step), context)
}

/// The idle 50 Hz cycle of [`idle_50_hz_cycle`], stepped by `step` with `context`.
fn idle_50_hz_cycle_with(
    node: *mut HyphaNode,
    step: HyphaCycleStep,
    context: *mut c_void,
) -> (*mut HyphaWaitSet, *mut HyphaInputSource, *mut HyphaCycle) {
    let mut waitset: *mut HyphaWaitSet = ptr::null_mut();
    assert_eq!(unsafe { hypha_node_default_waitset(node, &raw mut waitset) }, HyphaStatus::Ok);

    let topic = CString::new(format!(
        "/wakeups_in_{}_{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
    .expect("topic");
    let md5 = CString::new("d9d213e8c146065ec958333c617eb1a6").expect("md5");
    let sub_qos = HyphaSubscriberOptions {
        depth: 4,
        no_publisher_warning_after_nanos: 0,
        qos: crate::common::test_qos(),
    };
    let mut subscriber = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_subscribe_typed(
                node,
                topic.as_ptr(),
                topic.as_bytes().len(),
                c"std_msgs/Data".as_ptr(),
                "std_msgs/Data".len(),
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
    let mut source = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_input_source_create(waitset, subscriber, &raw mut source) },
        HyphaStatus::Ok
    );

    let name = CString::new("idle_50_hz").expect("cycle name");
    let mut cycle = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                waitset,
                name.as_ptr(),
                name.as_bytes().len(),
                20_000_000,
                step,
                context,
                None,
                &raw mut cycle,
            )
        },
        HyphaStatus::Ok
    );
    let binding = HyphaCycleBindingOptions {
        mode: HyphaCycleInputMode::Latest as u32,
        max_samples_per_iteration: 1,
        optional: 0,
    };
    let mut index = 0;
    assert_eq!(
        unsafe { hypha_cycle_bind_input(cycle, source, &raw const binding, &raw mut index) },
        HyphaStatus::Ok
    );
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);
    (waitset, source, cycle)
}

fn run_report() -> HyphaWaitSetRunReport {
    HyphaWaitSetRunReport { reason: 0, status: 0, attachment: 0, slow_callbacks: 0 }
}

#[test]
fn an_idle_50_hz_cycle_wakes_its_waitset_about_fifty_times_a_second() {
    let _serial = serial();
    let node = create_node(None);
    let steps = AtomicU32::new(0);
    let (waitset, source, cycle) = idle_50_hz_cycle(node, &steps);

    let sweeps_before = sweeps(node);
    let mut report = run_report();
    assert_eq!(
        unsafe { hypha_waitset_run(waitset, 1_000_000_000, &raw mut report) },
        HyphaStatus::Ok
    );
    assert_eq!(report.reason, HYPHA_RUN_REASON_DEADLINE);
    let turns = sweeps(node) - sweeps_before;

    let ticks = steps.load(Ordering::SeqCst);
    assert!((45..=55).contains(&ticks), "a 50 Hz cycle ran {ticks} steps in one second");
    assert!(
        (40..=70).contains(&turns),
        "an idle 50 Hz cycle woke its WaitSet {turns} times in one second; expected about 50"
    );

    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_input_source_destroy(source) }, HyphaStatus::Ok);
    unsafe {
        hypha_node_destroy(node);
    }
}

/// What the host cycle test records: the step count, and each step's `elapsed_nanos`.
#[derive(Default)]
struct StepLog {
    steps: AtomicU32,
    elapsed: Mutex<Vec<i64>>,
}

extern "C" fn record_step(
    _image: *const HyphaCycleImage,
    time: *const HyphaCycleTime,
    _emission: *mut HyphaCycleEmission,
    context: *mut c_void,
) -> HyphaCallbackResult {
    let log = unsafe { &*context.cast::<StepLog>() };
    log.steps.fetch_add(1, Ordering::SeqCst);
    let time = unsafe { &*time };
    if time.has_elapsed != 0 {
        log.elapsed.lock().unwrap_or_else(PoisonError::into_inner).push(time.elapsed_nanos);
    }
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

#[test]
fn a_host_cycle_steps_at_its_period_over_one_second() {
    let _serial = serial();
    let node = create_node(None);
    let log = StepLog::default();
    let context: *mut c_void = (&raw const log).cast_mut().cast();
    let (waitset, source, cycle) = idle_50_hz_cycle_with(node, Some(record_step), context);

    let mut report = run_report();
    assert_eq!(
        unsafe { hypha_waitset_run(waitset, 1_000_000_000, &raw mut report) },
        HyphaStatus::Ok
    );
    assert_eq!(report.reason, HYPHA_RUN_REASON_DEADLINE);

    let steps = log.steps.load(Ordering::SeqCst);
    assert!((45..=55).contains(&steps), "a 50 Hz cycle ran {steps} steps in one second");
    let elapsed = log.elapsed.lock().unwrap_or_else(PoisonError::into_inner).clone();
    assert!(!elapsed.is_empty(), "no step reported an elapsed time");
    // Every gap is the period, within a wide window for a loaded host. macOS timer slack
    // wakes a step 10 ms late now and then, on an idle host too (1 in 15 runs alone,
    // 2026-11-21), and the step after it comes that much early.
    let window =
        if cfg!(target_os = "macos") { 5_000_000..=35_000_000 } else { 10_000_000..=30_000_000 };
    for gap in elapsed {
        assert!(
            window.contains(&gap),
            "a 50 Hz host step came {} ms after the previous one",
            gap / 1_000_000
        );
    }

    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_input_source_destroy(source) }, HyphaStatus::Ok);
    unsafe {
        hypha_node_destroy(node);
    }
}

/// Publish one `/clock` sample, with `nanos` split into the ROS `secs`/`nsecs` pair.
unsafe fn publish_clock(publisher: *mut HyphaPublisher, nanos: u64) {
    let secs = u32::try_from(nanos / 1_000_000_000).expect("secs");
    let nsecs = u32::try_from(nanos % 1_000_000_000).expect("nsecs");
    let mut payload = [0_u8; 8];
    payload[..4].copy_from_slice(&secs.to_le_bytes());
    payload[4..].copy_from_slice(&nsecs.to_le_bytes());
    let mut loan = ptr::null_mut();
    let mut bytes = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_publisher_loan_frame(publisher, payload.len(), &raw mut loan, &raw mut bytes)
        },
        HyphaStatus::Ok
    );
    unsafe { ptr::copy_nonoverlapping(payload.as_ptr(), bytes, payload.len()) };
    assert_eq!(unsafe { hypha_loan_send(loan, -1) }, HyphaStatus::Ok);
}

fn clock_publisher(node: *mut HyphaNode) -> *mut HyphaPublisher {
    let topic = CString::new("/clock").expect("topic");
    let data_type = CString::new("rosgraph_msgs/Clock").expect("type");
    let md5 = CString::new("a9c97c1d230cfc112e270351a944ee47").expect("md5");
    let qos = HyphaPublisherOptions {
        latch: 0,
        loaned_samples: 0,
        qos: crate::common::qos_for_topic("/clock"),
    };
    let mut publisher = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_publish_with_qos(
                node,
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
    publisher
}

#[test]
#[allow(clippy::too_many_lines)]
fn an_idle_50_hz_sim_time_cycle_wakes_per_clock_event_not_per_millisecond() {
    const CLOCK_EVENTS: u64 = 50;
    const PERIOD_NANOS: u64 = 20_000_000;
    const BASE_NANOS: u64 = 1_000_000_000;
    // The clock needs 1.02 s; the rest is slack for a loaded host.
    const RUN_NANOS: u64 = 1_800_000_000;
    let _serial = serial();
    let node =
        common::create_sim_node_with_file(&format!("wakeups_sim_{}", std::process::id()), None);
    let steps = AtomicU32::new(0);
    let (waitset, source, cycle) = idle_50_hz_cycle(node, &steps);
    let publisher = clock_publisher(node);

    // The first value takes the phase, then each of the next fifty passes one deadline.
    let publisher_address = publisher as usize;
    // Each event waits for its own instant, so the sleeps' overshoot does not add up.
    let clock = std::thread::spawn(move || {
        let publisher = publisher_address as *mut HyphaPublisher;
        let started = Instant::now();
        for event in 0..=CLOCK_EVENTS {
            let due = started + Duration::from_nanos(PERIOD_NANOS * (event + 1));
            std::thread::sleep(due.saturating_duration_since(Instant::now()));
            let nanos = BASE_NANOS + event * PERIOD_NANOS;
            unsafe { publish_clock(publisher, nanos) };
        }
    });

    let sweeps_before = sweeps(node);
    let mut report = run_report();
    assert_eq!(unsafe { hypha_waitset_run(waitset, RUN_NANOS, &raw mut report) }, HyphaStatus::Ok);
    assert!(clock.join().is_ok(), "the clock publisher panicked");
    let turns = sweeps(node) - sweeps_before;

    let ticks = steps.load(Ordering::SeqCst);
    assert!(
        (45..=52).contains(&ticks),
        "a 50 Hz sim-time cycle ran {ticks} steps for one clock second"
    );
    // Each clock event wakes the WaitSet once, and the audit adds about 40 a second.
    assert!(
        turns <= 4 * (CLOCK_EVENTS + 1),
        "an idle sim-time cycle woke its WaitSet {turns} times for {} clock events",
        CLOCK_EVENTS + 1
    );

    unsafe { hypha_publisher_destroy(publisher) };
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_input_source_destroy(source) }, HyphaStatus::Ok);
    unsafe {
        hypha_node_destroy(node);
    }
}
