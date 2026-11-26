//! The time of a triggered step and the framework send stamp of its outputs (#239).
//!
//! On host time a step is stamped with the wall time at its start, so the stamp of an
//! output is not the stamp of the trigger sample that caused it. On simulated time the
//! step runs at the trigger sample's send time T, and its outputs carry T. Every bound
//! below holds by construction, so none depends on how long the machine takes.
#![allow(
    clippy::arithmetic_side_effects,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::too_many_lines,
    clippy::unwrap_used,
    reason = "a test asserts on known values and fails loudly"
)]

use std::ffi::c_void;
use std::ptr;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

mod common;
use crate::common::replay::{
    SERIAL, SimulatedClock, open_declared_publisher, open_subscriber, receive_frame, run_for,
    send_payload, send_stamped, set_clock,
};
use crate::common::supervision::{bind_trigger, node_with_triggers};
use crate::common::{c_text, emit_message, unique};
use hypha_ffi::waitset::*;
use hypha_ffi::*;

const MS: i64 = 1_000_000;
const SECOND: i64 = 1_000_000_000;
const TYPE: &str = "std_msgs/Counter";
const MD5: &str = "d9d213e8c146065ec958333c617eb1a6";
/// The trigger's silence limit where a sample must not go stale: ten seconds.
const LONG_LIMIT_MS: i64 = 10_000;
/// The longest a case waits for a step.
const BOUND: Duration = Duration::from_secs(20);

/// The wall clock in nanoseconds since the Unix epoch.
fn wall_now() -> i64 {
    let elapsed = SystemTime::now().duration_since(UNIX_EPOCH).expect("UTC");
    i64::try_from(elapsed.as_nanos()).expect("wall time fits i64")
}

/// What one step saw of its time, and nothing else.
#[derive(Debug, Clone, Copy)]
struct StepTime {
    scheduled: i64,
    now: i64,
    stamp: i64,
    /// Samples the trigger port held: 1 for a step of a sample, 0 for a fallback.
    samples: usize,
}

#[derive(Default)]
struct Context {
    steps: Mutex<Vec<StepTime>>,
}

impl Context {
    fn steps(&self) -> Vec<StepTime> {
        self.steps.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

extern "C" fn emit_one_byte(
    image: *const HyphaCycleImage,
    time: *const HyphaCycleTime,
    emission: *mut HyphaCycleEmission,
    context: *mut c_void,
) -> HyphaCallbackResult {
    let context = unsafe { &*context.cast::<Context>() };
    let time = unsafe { *time };
    let mut binding = HyphaCycleBindingView::default();
    assert_eq!(unsafe { hypha_cycle_image_binding(image, 0, &raw mut binding) }, HyphaStatus::Ok);
    let step = StepTime {
        scheduled: time.scheduled_nanos,
        now: time.callback_start_nanos,
        stamp: time.message_stamp_nanos,
        samples: binding.health.capture.sample_count,
    };
    context.steps.lock().unwrap_or_else(PoisonError::into_inner).push(step);
    assert_eq!(unsafe { emit_message(emission, 0, &[7]) }, HyphaStatus::Ok);
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

/// One triggered cycle with one trigger port and one output, the trigger's topic read by
/// a second subscriber that sees each sample as the cycle does, and the output's topic
/// read by a third that sees each frame the step sent.
struct Rig {
    node: *mut HyphaNode,
    waitset: *mut HyphaWaitSet,
    cycle: *mut HyphaCycle,
    source: *mut HyphaInputSource,
    trigger: *mut HyphaPublisher,
    /// The trigger topic, as an observer sees the sample's own send stamp.
    seen_trigger: *mut HyphaSubscriber,
    /// The output topic, as a downstream node sees the frame's send stamp.
    seen_output: *mut HyphaSubscriber,
    context: Box<Context>,
}

impl Rig {
    fn new(label: &str, limit_ms: i64) -> Self {
        let node = node_with_triggers(label, &[limit_ms]);
        let mut waitset = ptr::null_mut();
        assert_eq!(unsafe { hypha_node_default_waitset(node, &raw mut waitset) }, HyphaStatus::Ok);
        let trigger_topic = format!("/{}", unique("step_stamp_in"));
        let output_topic = format!("/{}", unique("step_stamp_out"));
        let trigger = unsafe { open_declared_publisher(node, &trigger_topic, TYPE, MD5) };
        let output = unsafe { open_declared_publisher(node, &output_topic, TYPE, MD5) };
        let seen_trigger = unsafe { open_subscriber(node, &trigger_topic, MD5, 16) };
        let seen_output = unsafe { open_subscriber(node, &output_topic, MD5, 16) };
        let cycle_subscriber = unsafe { open_subscriber(node, &trigger_topic, MD5, 16) };
        let mut source = ptr::null_mut();
        assert_eq!(
            unsafe { hypha_input_source_create(waitset, cycle_subscriber, &raw mut source) },
            HyphaStatus::Ok
        );
        let context = Box::<Context>::default();
        let name = c_text(&unique("step_stamp"));
        let mut cycle = ptr::null_mut();
        assert_eq!(
            unsafe {
                hypha_cycle_create(
                    waitset,
                    name.as_ptr(),
                    name.as_bytes().len(),
                    0,
                    Some(emit_one_byte),
                    (&raw const *context).cast_mut().cast::<c_void>(),
                    None,
                    &raw mut cycle,
                )
            },
            HyphaStatus::Ok
        );
        let options = HyphaCycleBindingOptions {
            mode: HyphaCycleInputMode::Queued as u32,
            max_samples_per_iteration: 1,
            optional: 0,
        };
        let mut index = 0_u32;
        assert_eq!(
            unsafe { hypha_cycle_bind_input(cycle, source, &raw const options, &raw mut index) },
            HyphaStatus::Ok
        );
        let bound = unsafe { bind_trigger(cycle, node, index, limit_ms) };
        assert_eq!(bound.status, HyphaStatus::Ok, "{}", bound.detail);
        assert_eq!(unsafe { hypha_cycle_set_trigger(cycle, index) }, HyphaStatus::Ok);
        let mut out_index = 0_u32;
        assert_eq!(
            unsafe {
                hypha_cycle_bind_output(
                    cycle,
                    output,
                    HyphaCycleOutputFailure::FailRun as u32,
                    1,
                    &raw mut out_index,
                )
            },
            HyphaStatus::Ok
        );
        assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);
        Self { node, waitset, cycle, source, trigger, seen_trigger, seen_output, context }
    }

    /// Run the `WaitSet` in short slices until `count` steps ran, or the bound passes.
    fn run_until_steps(&self, count: usize) -> Vec<StepTime> {
        let started = Instant::now();
        while self.context.steps().len() < count {
            assert!(started.elapsed() < BOUND, "{count} steps did not run: {:?}", self.context);
            run_for(self.waitset, 5_000_000);
        }
        self.context.steps()
    }

    /// The framework send stamp of the next frame the step sent.
    fn output_stamp(&self) -> i64 {
        let (_, sent) = unsafe { receive_frame(self.seen_output) }.expect("the step sent a frame");
        i64::try_from(sent.framework_sent_nanos).expect("the stamp fits i64")
    }

    /// The framework send stamp the trigger sample carries.
    fn trigger_stamp(&self) -> i64 {
        let (_, sent) = unsafe { receive_frame(self.seen_trigger) }.expect("the sample arrived");
        i64::try_from(sent.framework_sent_nanos).expect("the stamp fits i64")
    }
}

impl std::fmt::Debug for Context {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_list().entries(self.steps()).finish()
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        unsafe {
            hypha_cycle_destroy(self.cycle);
            hypha_input_source_destroy(self.source);
            hypha_subscriber_destroy(self.seen_trigger);
            hypha_subscriber_destroy(self.seen_output);
            hypha_publisher_destroy(self.trigger);
            hypha_node_destroy(self.node);
        }
    }
}

#[test]
fn on_host_time_the_output_of_a_triggered_step_is_stamped_with_the_wall_time_of_the_step() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let rig = Rig::new("step_stamp_host", LONG_LIMIT_MS);

    let before = wall_now();
    unsafe { send_payload(rig.trigger, &[1]) };
    let steps = rig.run_until_steps(1);
    let after = wall_now();

    let trigger_sent = rig.trigger_stamp();
    let output_sent = rig.output_stamp();
    let [step] = steps[..] else { panic!("one step: {steps:?}") };
    assert_eq!(step.samples, 1, "a step of the sample: {step:?}");
    // The sample was sent after `before`, and the step read its wall time after the
    // sample was sent and before `after`, so its stamp is between the two readings.
    assert!(before <= trigger_sent && trigger_sent <= after, "{before} {trigger_sent} {after}");
    assert!(
        trigger_sent <= output_sent && output_sent <= after,
        "the output is stamped when the step started, after the sample was sent: \
         sample {trigger_sent}, output {output_sent}, after {after}"
    );
    // One reading: the step's `now`, its header stamp and the stamp of its frame agree.
    assert_eq!(step.now, output_sent, "{step:?}");
    assert_eq!(step.stamp, output_sent, "{step:?}");
}

#[test]
fn on_host_time_an_output_does_not_carry_the_stamp_of_the_trigger_sample_that_caused_it() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let rig = Rig::new("step_stamp_old", LONG_LIMIT_MS);

    // A sample stamped two seconds before it is sent, as a relay or a sensor driver may
    // stamp it. The step runs on the wall clock, so its stamp is not the sample's.
    let before = wall_now();
    let stamped = before - 2 * SECOND;
    unsafe { send_stamped(rig.trigger, 1, stamped) };
    let steps = rig.run_until_steps(1);
    let after = wall_now();

    assert_eq!(rig.trigger_stamp(), stamped, "the sample keeps the stamp it was sent with");
    let output_sent = rig.output_stamp();
    assert!(
        before <= output_sent && output_sent <= after,
        "the output is stamped within the run: {before} {output_sent} {after}"
    );
    assert_ne!(output_sent, stamped, "the output does not carry the sample's stamp");
    let [step] = steps[..] else { panic!("one step: {steps:?}") };
    assert_eq!(step.now, output_sent, "{step:?}");
}

#[test]
fn on_host_time_the_scheduled_time_of_a_triggered_step_is_the_sample_on_the_monotonic_clock() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let rig = Rig::new("step_stamp_sched", LONG_LIMIT_MS);

    // The sample's wall age is taken back from the monotonic reading of the turn, so a
    // sample stamped two seconds ago is scheduled at least a second before now.
    let steady_before = hypha_clock_steady_now_nanos();
    unsafe { send_stamped(rig.trigger, 1, wall_now() - 2 * SECOND) };
    let steps = rig.run_until_steps(1);
    let steady_after = hypha_clock_steady_now_nanos();

    let [step] = steps[..] else { panic!("one step: {steps:?}") };
    assert!(
        step.scheduled < steady_before - SECOND,
        "scheduled {} is not the sample's time on the monotonic clock ({steady_before})",
        step.scheduled
    );
    assert!(step.scheduled < steady_after, "{step:?}");
    // The step's time is the wall clock, not the schedule: the two clocks differ by far
    // more than the two seconds.
    assert_ne!(step.scheduled, step.now, "{step:?}");
}

#[test]
fn on_host_time_the_output_of_a_fallback_step_is_stamped_with_the_wall_time_of_the_step() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let rig = Rig::new("step_stamp_fallback", 50);

    let before = wall_now();
    let steps = rig.run_until_steps(1);
    let after = wall_now();

    let output_sent = rig.output_stamp();
    let step = steps[0];
    assert_eq!(step.samples, 0, "a fallback holds no sample: {step:?}");
    assert!(before <= output_sent && output_sent <= after, "{before} {output_sent} {after}");
    assert_eq!(step.now, output_sent, "{step:?}");
}

#[test]
fn on_simulated_time_the_output_of_a_triggered_step_is_stamped_with_the_trigger_send_time() {
    // The clock starts at one second and the sample is sent at T = 1.005 s. The clock is
    // then set past T, so a stamp that read the clock would differ from T.
    let base = SECOND;
    let sent_at = base + 5 * MS;
    let clock = SimulatedClock::start(base);
    let rig = Rig::new("step_stamp_sim", LONG_LIMIT_MS);

    unsafe { send_stamped(rig.trigger, 1, sent_at) };
    set_clock(base + 40 * MS);
    let steps = rig.run_until_steps(1);

    let [step] = steps[..] else { panic!("one step: {steps:?}") };
    assert_eq!(step.samples, 1, "{step:?}");
    assert_eq!(step.scheduled, sent_at, "{step:?}");
    assert_eq!(step.now, sent_at, "{step:?}");
    assert_eq!(step.stamp, sent_at, "{step:?}");
    assert_eq!(rig.trigger_stamp(), sent_at);
    assert_eq!(rig.output_stamp(), sent_at, "the output carries T");
    drop(rig);
    drop(clock);
}

#[test]
fn on_simulated_time_the_output_of_a_fallback_step_is_stamped_with_the_fallback_time() {
    let base = SECOND;
    let sent_at = base + 5 * MS;
    let limit_ms = 100;
    let clock = SimulatedClock::start(base);
    let rig = Rig::new("step_stamp_sim_fallback", limit_ms);

    // The limit counts from the last step or the first evaluation, whichever is later, so
    // the clock is at T when the cycle is first evaluated.
    unsafe { send_stamped(rig.trigger, 1, sent_at) };
    set_clock(sent_at);
    rig.run_until_steps(1);
    assert_eq!(rig.output_stamp(), sent_at);

    // The fallback is due one limit after the last step, and runs at that time.
    let due = sent_at + limit_ms * MS;
    set_clock(due + 3 * MS);
    let steps = rig.run_until_steps(2);

    let step = steps[1];
    assert_eq!(step.samples, 0, "a fallback holds no sample: {step:?}");
    assert_eq!(step.scheduled, due, "{step:?}");
    assert_eq!(step.now, due, "{step:?}");
    assert_eq!(rig.output_stamp(), due, "the output carries the fallback time");
    drop(rig);
    drop(clock);
}
