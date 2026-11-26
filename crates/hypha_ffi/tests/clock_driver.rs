//! A cycle that drives `/clock` for a simulator. Every case holds [`SERIAL`].
#![allow(
    clippy::arithmetic_side_effects,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::unwrap_used
)]

use std::ffi::c_void;
use std::ptr;
use std::sync::{MutexGuard, PoisonError};

mod common;
use crate::common::replay::*;
use crate::common::supervision::{bind_trigger, node_with_triggers};
use crate::common::{c_text, create_node, unique};
use hypha_ffi::waitset::*;
use hypha_ffi::*;

const MS: u64 = 1_000_000;
const PERIOD: u64 = 10 * MS;
const START: i64 = 1_000_000_000;

extern "C" fn empty_step(
    _image: *const HyphaCycleImage,
    _time: *const HyphaCycleTime,
    _emission: *mut HyphaCycleEmission,
    _context: *mut c_void,
) -> HyphaCallbackResult {
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

fn driver_options() -> HyphaClockDriverOptions {
    HyphaClockDriverOptions { start_nanos: START, timeout_nanos: 300 * MS, real_time_factor: 0.0 }
}

/// A node with one `WaitSet`, holding the serial lock and leaving simulated time off.
struct Host {
    node: *mut HyphaNode,
    waitset: *mut HyphaWaitSet,
    _serial: MutexGuard<'static, ()>,
}

impl Host {
    fn new(node: *mut HyphaNode) -> Self {
        let serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = hypha_clock_disable_sim_time();
        let waitset = unsafe { create_waitset(node) };
        Self { node, waitset, _serial: serial }
    }

    fn plain() -> Self {
        Self::new(node_with_triggers("driver_host", &[100]))
    }

    fn cycle(&self, period_nanos: u64) -> *mut HyphaCycle {
        let name = c_text(&unique("driver"));
        let mut cycle = ptr::null_mut();
        let status = unsafe {
            hypha_cycle_create(
                self.waitset,
                name.as_ptr(),
                name.as_bytes().len(),
                period_nanos,
                Some(empty_step),
                ptr::null_mut(),
                None,
                &raw mut cycle,
            )
        };
        assert_eq!(status, HyphaStatus::Ok);
        cycle
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        unsafe {
            hypha_node_destroy(self.node);
        }
        let _ = hypha_clock_disable_sim_time();
    }
}

/// Name the one participant a driver needs: it refuses to run with none.
fn expect_peer(cycle: *mut HyphaCycle) {
    let name = "peer";
    assert_eq!(
        unsafe { hypha_cycle_expect_participant(cycle, name.as_ptr().cast(), name.len()) },
        HyphaStatus::Ok
    );
}

#[test]
fn a_driver_without_a_participant_is_refused_at_commit() {
    let host = Host::plain();
    let cycle = host.cycle(PERIOD);
    let options = driver_options();
    assert_eq!(unsafe { hypha_cycle_set_clock_driver(cycle, &raw const options) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::InvalidArgument);
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
}

#[test]
fn set_clock_driver_refuses_a_cycle_without_a_rate() {
    let host = Host::plain();
    let cycle = host.cycle(0);
    let options = driver_options();
    assert_eq!(
        unsafe { hypha_cycle_set_clock_driver(cycle, &raw const options) },
        HyphaStatus::InvalidArgument
    );
    assert_eq!(
        unsafe { hypha_cycle_set_clock_driver(ptr::null_mut(), &raw const options) },
        HyphaStatus::InvalidArgument
    );
    assert_eq!(
        unsafe { hypha_cycle_set_clock_driver(cycle, ptr::null()) },
        HyphaStatus::InvalidArgument
    );
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
}

#[test]
fn set_clock_driver_refuses_a_triggered_cycle() {
    let host = Host::plain();
    let subscriber = unsafe {
        open_subscriber(
            host.node,
            &format!("/{}", unique("drv_trig")),
            "d9d213e8c146065ec958333c617eb1a6",
            4,
        )
    };
    let mut source = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_input_source_create(host.waitset, subscriber, &raw mut source) },
        HyphaStatus::Ok
    );
    let cycle = host.cycle(0);
    let bind = HyphaCycleBindingOptions {
        mode: HyphaCycleInputMode::Queued as u32,
        max_samples_per_iteration: 1,
        optional: 0,
    };
    let mut index = 0;
    assert_eq!(
        unsafe { hypha_cycle_bind_input(cycle, source, &raw const bind, &raw mut index) },
        HyphaStatus::Ok
    );
    let bound = unsafe { bind_trigger(cycle, host.node, index, 100) };
    assert_eq!(bound.status, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_cycle_set_trigger(cycle, index) }, HyphaStatus::Ok);
    let options = driver_options();
    assert_eq!(
        unsafe { hypha_cycle_set_clock_driver(cycle, &raw const options) },
        HyphaStatus::InvalidArgument
    );
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    unsafe { hypha_input_source_destroy(source) };
}

#[test]
fn set_clock_driver_refuses_a_zero_timeout_and_a_negative_factor() {
    let host = Host::plain();
    let cycle = host.cycle(PERIOD);
    for options in [
        HyphaClockDriverOptions { timeout_nanos: 0, ..driver_options() },
        HyphaClockDriverOptions { real_time_factor: -1.0, ..driver_options() },
        HyphaClockDriverOptions { real_time_factor: f64::NAN, ..driver_options() },
        HyphaClockDriverOptions { start_nanos: -1, ..driver_options() },
    ] {
        assert_eq!(
            unsafe { hypha_cycle_set_clock_driver(cycle, &raw const options) },
            HyphaStatus::InvalidArgument
        );
    }
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
}

#[test]
fn expect_participant_refuses_a_cycle_that_is_not_a_driver() {
    let host = Host::plain();
    let cycle = host.cycle(PERIOD);
    let name = c_text("stage_a");
    assert_eq!(
        unsafe { hypha_cycle_expect_participant(cycle, name.as_ptr(), name.as_bytes().len()) },
        HyphaStatus::InvalidArgument
    );
    let options = driver_options();
    assert_eq!(unsafe { hypha_cycle_set_clock_driver(cycle, &raw const options) }, HyphaStatus::Ok);
    assert_eq!(
        unsafe { hypha_cycle_expect_participant(cycle, name.as_ptr(), 0) },
        HyphaStatus::InvalidArgument
    );
    assert_eq!(
        unsafe { hypha_cycle_expect_participant(cycle, name.as_ptr(), name.as_bytes().len()) },
        HyphaStatus::Ok
    );
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
}

#[test]
fn commit_refuses_a_driver_on_a_node_that_reads_clock() {
    let host = Host::new(create_sim_node(&unique("driver_sim")));
    let _ = hypha_clock_disable_sim_time();
    let cycle = host.cycle(PERIOD);
    let options = driver_options();
    assert_eq!(unsafe { hypha_cycle_set_clock_driver(cycle, &raw const options) }, HyphaStatus::Ok);
    expect_peer(cycle);
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::InvalidState);
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
}

#[test]
fn commit_refuses_a_driver_when_sim_time_is_already_enabled() {
    let host = Host::plain();
    let cycle = host.cycle(PERIOD);
    let options = driver_options();
    assert_eq!(unsafe { hypha_cycle_set_clock_driver(cycle, &raw const options) }, HyphaStatus::Ok);
    expect_peer(cycle);
    assert_eq!(hypha_clock_enable_sim_time(0), HyphaStatus::Ok as i32);
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::InvalidState);
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
}

#[test]
fn a_driver_cycle_commits_and_destroys() {
    let host = Host::plain();
    let cycle = host.cycle(PERIOD);
    let options = driver_options();
    assert_eq!(unsafe { hypha_cycle_set_clock_driver(cycle, &raw const options) }, HyphaStatus::Ok);
    expect_peer(cycle);
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);
    assert_eq!(hypha_clock_is_sim_time(), 1, "the driver owns simulated time");
    assert_eq!(hypha_clock_now_nanos(), START);
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    assert_eq!(hypha_clock_is_sim_time(), 0, "destroying the driver gives simulated time back");
}

// The participants are created first: a node on simulated time resets the process
// clock when it is created, so the driver takes the clock after.

const MD5: &str = "d9d213e8c146065ec958333c617eb1a6";
const DATA_TYPE: &str = "std_msgs/Counter";
const DEPTH: usize = 8;

/// One step as the test reads it back: its deadline and the newest word it captured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Record {
    scheduled: i64,
    seen: Option<u64>,
}

#[derive(Default)]
struct Context {
    steps: std::sync::Mutex<Vec<Record>>,
}

/// Emits its own deadline when `context` says so, else the newest captured word.
struct StepContext {
    record: Context,
    emit_deadline: bool,
}

extern "C" fn echo_step(
    image: *const HyphaCycleImage,
    time: *const HyphaCycleTime,
    emission: *mut HyphaCycleEmission,
    context: *mut c_void,
) -> HyphaCallbackResult {
    let context = unsafe { &*context.cast::<StepContext>() };
    let scheduled = unsafe { (*time).scheduled_nanos };
    let mut binding = HyphaCycleBindingView::default();
    let mut seen = None;
    if unsafe { hypha_cycle_image_binding(image, 0, &raw mut binding) } == HyphaStatus::Ok
        && let Some(last) = binding.health.capture.sample_count.checked_sub(1)
    {
        let mut view = HyphaCycleSampleView {
            payload: ptr::null(),
            payload_len: 0,
            transport_sent_nanos: 0,
            sequence: 0,
            publisher_id: 0,
            publisher_sequence: 0,
        };
        assert_eq!(
            unsafe { hypha_cycle_image_sample(image, 0, last, &raw mut view) },
            HyphaStatus::Ok
        );
        let bytes = unsafe { std::slice::from_raw_parts(view.payload, view.payload_len) };
        seen = Some(u64::from_le_bytes(bytes.try_into().expect("eight bytes")));
    }
    context.record.steps.lock().expect("steps").push(Record { scheduled, seen });
    let word =
        if context.emit_deadline { u64::try_from(scheduled).unwrap() } else { seen.unwrap_or(0) };
    assert_eq!(
        unsafe { crate::common::emit_message(emission, 0, &word.to_le_bytes()) },
        HyphaStatus::Ok
    );
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

struct Stage {
    name: String,
    node: *mut HyphaNode,
    waitset: *mut HyphaWaitSet,
    sources: Vec<*mut HyphaInputSource>,
    cycle: *mut HyphaCycle,
    context: Box<StepContext>,
}

enum Mode {
    /// A replay rate cycle on a node that reads `/clock`.
    Rate,
    /// A triggered cycle on a node that reads `/clock`.
    Triggered,
    /// A triggered cycle on a node that reads `/clock`, with a second trigger on a topic
    /// nobody publishes.
    TwoTriggers,
    /// The driver, on a plain node.
    Driver(Vec<String>, u64),
}

impl Stage {
    fn new(label: &str, mode: &Mode, input: &str, output: &str) -> Self {
        let name = unique(label);
        let node = match mode {
            Mode::Driver(..) => create_node(&name),
            _ => create_sim_node_with_triggers(&name, &[50_000, 60_000]),
        };
        let waitset = unsafe { create_waitset(node) };
        let quiet = format!("{input}_quiet");
        let inputs =
            if matches!(mode, Mode::TwoTriggers) { vec![input, &quiet] } else { vec![input] };
        let sources: Vec<*mut HyphaInputSource> = inputs
            .into_iter()
            .map(|topic| {
                let subscriber = unsafe { open_subscriber(node, topic, MD5, DEPTH) };
                let mut source = ptr::null_mut();
                assert_eq!(
                    unsafe { hypha_input_source_create(waitset, subscriber, &raw mut source) },
                    HyphaStatus::Ok
                );
                source
            })
            .collect();
        let publisher = unsafe { open_publisher(node, output, DATA_TYPE, MD5, "") };
        let context = Box::new(StepContext {
            record: Context::default(),
            emit_deadline: matches!(mode, Mode::Driver(..)),
        });
        let label = c_text(&name);
        let period = if matches!(mode, Mode::Triggered | Mode::TwoTriggers) { 0 } else { PERIOD };
        let mut cycle = ptr::null_mut();
        let status = unsafe {
            hypha_cycle_create(
                waitset,
                label.as_ptr(),
                label.as_bytes().len(),
                period,
                Some(echo_step),
                (&raw const *context).cast_mut().cast::<c_void>(),
                None,
                &raw mut cycle,
            )
        };
        assert_eq!(status, HyphaStatus::Ok);
        let bind = HyphaCycleBindingOptions {
            mode: HyphaCycleInputMode::Queued as u32,
            max_samples_per_iteration: DEPTH,
            optional: 0,
        };
        let indices: Vec<u32> = sources
            .iter()
            .map(|&source| {
                let mut index = 0_u32;
                assert_eq!(
                    unsafe {
                        hypha_cycle_bind_input(cycle, source, &raw const bind, &raw mut index)
                    },
                    HyphaStatus::Ok
                );
                index
            })
            .collect();
        let mut out_index = 0_u32;
        let status = unsafe {
            hypha_cycle_bind_output(
                cycle,
                publisher,
                HyphaCycleOutputFailure::FailRun as u32,
                1,
                &raw mut out_index,
            )
        };
        assert_eq!(status, HyphaStatus::Ok);
        match mode {
            Mode::Rate => {}
            Mode::Triggered | Mode::TwoTriggers => {
                for (&index, silence_ms) in indices.iter().zip([50_000, 60_000]) {
                    let bound = unsafe { bind_trigger(cycle, node, index, silence_ms) };
                    assert_eq!(bound.status, HyphaStatus::Ok, "{}", bound.detail);
                    assert_eq!(unsafe { hypha_cycle_set_trigger(cycle, index) }, HyphaStatus::Ok);
                }
            }
            Mode::Driver(expected, timeout) => {
                let options =
                    HyphaClockDriverOptions { timeout_nanos: *timeout, ..driver_options() };
                assert_eq!(
                    unsafe { hypha_cycle_set_clock_driver(cycle, &raw const options) },
                    HyphaStatus::Ok
                );
                for participant in expected {
                    let text = c_text(participant);
                    assert_eq!(
                        unsafe {
                            hypha_cycle_expect_participant(
                                cycle,
                                text.as_ptr(),
                                text.as_bytes().len(),
                            )
                        },
                        HyphaStatus::Ok
                    );
                }
                // The participants enabled a placeholder clock; the driver takes it over.
                let _ = hypha_clock_disable_sim_time();
            }
        }
        assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);
        Self { name, node, waitset, sources, cycle, context }
    }

    fn steps(&self) -> Vec<Record> {
        self.context.record.steps.lock().expect("steps").clone()
    }

    fn run(&self, nanos: u64) -> i32 {
        let mut report =
            HyphaWaitSetRunReport { reason: 0, status: 0, attachment: 0, slow_callbacks: 0 };
        assert_eq!(
            unsafe { hypha_waitset_run(self.waitset, nanos, &raw mut report) },
            HyphaStatus::Ok
        );
        report.status
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        unsafe {
            hypha_cycle_destroy(self.cycle);
            for &source in &self.sources {
                hypha_input_source_destroy(source);
            }
            hypha_node_destroy(self.node);
        }
    }
}

/// A driver and one participant `stage`, created in that order of clock ownership.
struct Loop {
    _serial: MutexGuard<'static, ()>,
    stage: Stage,
    driver: Stage,
}

impl Loop {
    fn new(stage_mode: &Mode, expect_stage: bool, timeout: u64) -> Self {
        let serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = hypha_clock_disable_sim_time();
        let (to_stage, to_driver) =
            (format!("/{}", unique("drv_out")), format!("/{}", unique("drv_back")));
        let stage = Stage::new("stage", stage_mode, &to_stage, &to_driver);
        let expected = if expect_stage { vec![stage.name.clone()] } else { Vec::new() };
        let driver = Stage::new("driver", &Mode::Driver(expected, timeout), &to_driver, &to_stage);
        Self { _serial: serial, stage, driver }
    }

    /// Run both `WaitSet`s alternately, in `order`, until the driver ran `steps` steps.
    fn run(&self, steps: usize, stage_first: bool) {
        let started = std::time::Instant::now();
        while self.driver.steps().len() < steps {
            assert!(started.elapsed() < std::time::Duration::from_secs(10), "the driver stalled");
            let order =
                if stage_first { [&self.stage, &self.driver] } else { [&self.driver, &self.stage] };
            for stage in order {
                assert_eq!(stage.run(2 * MS), 0, "a run failed");
            }
        }
    }
}

impl Drop for Loop {
    fn drop(&mut self) {
        let _ = hypha_clock_disable_sim_time();
    }
}

fn deadline(step: usize) -> i64 {
    START + i64::try_from(step).unwrap() * i64::try_from(PERIOD).unwrap()
}

#[test]
fn a_driver_steps_only_after_every_participant_acknowledges() {
    let run = Loop::new(&Mode::Rate, true, 5_000 * MS);
    run.run(50, false);
    let driver: Vec<i64> = run.driver.steps().iter().map(|step| step.scheduled).collect();
    let expected: Vec<i64> = (0..driver.len()).map(deadline).collect();
    assert_eq!(driver, expected, "the driver steps start + k * period");
    // Step k + 1 waits for the stage to acknowledge step k, so the stage is never more
    // than two values behind.
    let stage = run.stage.steps();
    assert!(
        stage.len() + 2 >= driver.len(),
        "stage {} steps, driver {}",
        stage.len(),
        driver.len()
    );
}

#[test]
fn the_driver_reads_its_participants_output_one_step_later() {
    let run = Loop::new(&Mode::Triggered, true, 5_000 * MS);
    run.run(30, false);
    for step in run.driver.steps().iter().skip(2) {
        let echo = step.seen.expect("the echo of the previous step");
        assert_eq!(
            i64::try_from(echo).unwrap(),
            step.scheduled - i64::try_from(PERIOD).unwrap(),
            "{step:?}"
        );
    }
}

#[test]
fn a_two_trigger_cycle_acknowledges_every_driver_step() {
    let run = Loop::new(&Mode::TwoTriggers, true, 5_000 * MS);
    run.run(30, false);
    // The quiet second trigger holds nothing back: the stage steps on each driver output.
    let stage: Vec<i64> = run
        .stage
        .steps()
        .iter()
        .filter(|step| step.seen.is_some())
        .map(|step| step.scheduled)
        .collect();
    let driver: Vec<i64> = run.driver.steps().iter().map(|step| step.scheduled).collect();
    assert!(stage.len() + 2 >= driver.len(), "stage {stage:?}, driver {driver:?}");
    assert_eq!(Some(stage.as_slice()), driver.get(..stage.len()));
}

fn driver_inputs(stage_first: bool) -> Vec<Record> {
    let run = Loop::new(&Mode::Triggered, true, 5_000 * MS);
    run.run(30, stage_first);
    run.driver.steps().into_iter().take(30).collect()
}

#[test]
fn the_result_does_not_depend_on_the_turn_order() {
    assert_eq!(driver_inputs(false), driver_inputs(true));
}

#[test]
fn a_participant_that_stops_fails_the_driver_naming_it() {
    let run = Loop::new(&Mode::Rate, true, 300 * MS);
    run.run(10, false);
    let started = std::time::Instant::now();
    let mut status = 0;
    while status == 0 && started.elapsed() < std::time::Duration::from_secs(2) {
        status = run.driver.run(50 * MS);
    }
    assert_eq!(status, HyphaStatus::Timeout as i32);
    assert!(started.elapsed() < std::time::Duration::from_secs(1), "{:?}", started.elapsed());
}

#[test]
fn an_expected_participant_holds_the_start_and_the_value_is_resent() {
    let serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let _ = hypha_clock_disable_sim_time();
    let topic = format!("/{}", unique("drv_solo"));
    let driver = Stage::new(
        "driver",
        &Mode::Driver(vec!["nobody".to_owned()], 5_000 * MS),
        &topic,
        &format!("{topic}_out"),
    );
    let reader_node = create_node(&unique("clock_reader"));
    let reader = unsafe { open_subscriber(reader_node, "/clock", CLOCK_MD5, 32) };
    let started = std::time::Instant::now();
    while started.elapsed() < std::time::Duration::from_secs(1) {
        assert_eq!(driver.run(20 * MS), 0);
    }
    assert_eq!(driver.steps().len(), 1, "only the start value ran");
    let mut received = 0;
    while unsafe { receive_payload(reader) }.is_some() {
        received += 1;
    }
    assert!(received >= 4, "the start value was sent {received} times");
    unsafe {
        hypha_subscriber_destroy(reader);
        hypha_node_destroy(reader_node);
    }
    drop(driver);
    let _ = hypha_clock_disable_sim_time();
    drop(serial);
}

#[test]
fn commit_refuses_a_driver_when_another_clock_publisher_exists() {
    let host = Host::plain();
    let other = create_node(&unique("other_clock"));
    let clock = unsafe { clock_publisher(other) };
    let cycle = host.cycle(PERIOD);
    let options = driver_options();
    assert_eq!(unsafe { hypha_cycle_set_clock_driver(cycle, &raw const options) }, HyphaStatus::Ok);
    expect_peer(cycle);
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::InvalidState);
    assert_eq!(hypha_clock_is_sim_time(), 0, "a refused driver leaves the clock alone");
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    unsafe {
        hypha_publisher_destroy(clock);
        hypha_node_destroy(other);
    }
}

#[test]
fn a_driver_node_runs_no_other_cycle() {
    let host = Host::plain();
    // A cycle committed after the driver is refused.
    let driver = host.cycle(PERIOD);
    let options = driver_options();
    assert_eq!(
        unsafe { hypha_cycle_set_clock_driver(driver, &raw const options) },
        HyphaStatus::Ok
    );
    expect_peer(driver);
    assert_eq!(unsafe { hypha_cycle_commit(driver) }, HyphaStatus::Ok);
    let other = host.cycle(PERIOD);
    assert_eq!(unsafe { hypha_cycle_commit(other) }, HyphaStatus::InvalidState);
    assert_eq!(unsafe { hypha_cycle_destroy(other) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_cycle_destroy(driver) }, HyphaStatus::Ok);
    // A driver committed after another cycle is refused too.
    let first = host.cycle(PERIOD);
    assert_eq!(unsafe { hypha_cycle_commit(first) }, HyphaStatus::Ok);
    let late = host.cycle(PERIOD);
    assert_eq!(unsafe { hypha_cycle_set_clock_driver(late, &raw const options) }, HyphaStatus::Ok);
    expect_peer(late);
    assert_eq!(unsafe { hypha_cycle_commit(late) }, HyphaStatus::InvalidState);
    assert_eq!(unsafe { hypha_cycle_destroy(late) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_cycle_destroy(first) }, HyphaStatus::Ok);
}
