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

/// The steps the simulated-time test expects: one per period of its clock second.
const SIM_STEPS: u32 = 50;

/// Counts the steps and ends the run at the last one the test expects.
extern "C" fn count_step_to_the_last(
    _image: *const HyphaCycleImage,
    _time: *const HyphaCycleTime,
    _emission: *mut HyphaCycleEmission,
    context: *mut c_void,
) -> HyphaCallbackResult {
    let steps = unsafe { &*context.cast::<AtomicU32>() };
    let ran = steps.fetch_add(1, Ordering::SeqCst).saturating_add(1);
    let action = if ran >= SIM_STEPS { HYPHA_ACTION_STOP } else { HYPHA_ACTION_CONTINUE };
    HyphaCallbackResult { action, status: 0 }
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
fn an_idle_50_hz_cycle_wakes_its_waitset_once_per_step() {
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

    // A loaded host runs fewer steps; the wakes per step are the claim, not the count.
    let ticks = u64::from(steps.load(Ordering::SeqCst));
    assert!(ticks >= 1, "a 50 Hz cycle ran no step in one second");
    assert!(
        turns <= 2 * ticks + 2,
        "an idle 50 Hz cycle woke its WaitSet {turns} times for {ticks} steps: it polls"
    );

    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_input_source_destroy(source) }, HyphaStatus::Ok);
    unsafe {
        hypha_node_destroy(node);
    }
}

/// What the host cycle test records: each step's scheduled time and missed periods.
#[derive(Default)]
struct StepLog {
    steps: Mutex<Vec<(i64, u64)>>,
}

extern "C" fn record_step(
    _image: *const HyphaCycleImage,
    time: *const HyphaCycleTime,
    _emission: *mut HyphaCycleEmission,
    context: *mut c_void,
) -> HyphaCallbackResult {
    let log = unsafe { &*context.cast::<StepLog>() };
    let time = unsafe { &*time };
    let entry = (time.scheduled_nanos, time.missed_periods);
    log.steps.lock().unwrap_or_else(PoisonError::into_inner).push(entry);
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

#[test]
fn a_host_cycle_schedules_every_step_on_its_period() {
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

    // However late the host wakes it, a step is scheduled a whole number of periods after
    // the previous one, and the periods it skipped are reported as missed.
    let steps = log.steps.lock().unwrap_or_else(PoisonError::into_inner).clone();
    assert!(steps.len() >= 2, "a 50 Hz cycle ran {} steps in one second", steps.len());
    for pair in steps.windows(2) {
        let [(previous, _), (scheduled, missed)] = pair else { continue };
        let periods = i64::try_from(missed + 1).expect("missed periods");
        assert_eq!(
            scheduled - previous,
            periods * 20_000_000,
            "a step {} ns after the previous one reported {missed} missed periods",
            scheduled - previous
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
    // The last step ends the run; the limit only guards against a hang.
    const RUN_NANOS: u64 = 60_000_000_000;
    let _serial = serial();
    let node =
        common::create_sim_node_with_file(&format!("wakeups_sim_{}", std::process::id()), None);
    let steps = AtomicU32::new(0);
    let context: *mut c_void = (&raw const steps).cast_mut().cast();
    let (waitset, source, cycle) =
        idle_50_hz_cycle_with(node, Some(count_step_to_the_last), context);
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

    // One step per period of the clock second, however the host spaced the samples.
    assert_eq!(report.reason, HYPHA_RUN_REASON_STOPPED, "the clock second did not pass");
    let ticks = steps.load(Ordering::SeqCst);
    assert_eq!(ticks, SIM_STEPS, "a 50 Hz sim-time cycle ran {ticks} steps for one clock second");
    // About two turns per clock event (its wake, then its step); nothing polls. A 50 ms
    // poll would add 20 a second.
    assert!(
        turns <= 3 * (CLOCK_EVENTS + 1),
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
