//! Input supervision across the C ABI: declaration checks and the image report.
#![allow(
    clippy::arithmetic_side_effects,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::indexing_slicing
)]

mod common;
use crate::common::c_text;
use crate::common::create_node_with_file;
use crate::common::supervision::bind;
use crate::common::unique;

use std::ffi::c_void;
use std::path::Path;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use hypha_ffi::waitset::*;
use hypha_ffi::*;

static SERIAL: Mutex<()> = Mutex::new(());

const SILENCE_NANOS: i64 = 100_000_000;
const STAGNATION_NANOS: i64 = 300_000_000;
const SLICE_NANOS: i64 = 10_000_000;
const MAX_SLICES: usize = 300;
const POLL_BOUND: std::time::Duration = std::time::Duration::from_secs(10);

unsafe fn create_waitset(node: *mut HyphaNode) -> *mut HyphaWaitSet {
    let mut waitset = ptr::null_mut();
    assert_eq!(unsafe { hypha_node_default_waitset(node, &raw mut waitset) }, HyphaStatus::Ok);
    waitset
}

unsafe fn create_pubsub(
    node: *mut HyphaNode,
    topic_str: &str,
) -> (*mut HyphaPublisher, *mut HyphaSubscriber) {
    let topic = c_text(topic_str);
    let data_type = c_text("std_msgs/Counter");
    let md5 = c_text("d9d213e8c146065ec958333c617eb1a6");
    let definition = c_text("");

    let pub_qos =
        HyphaPublisherOptions { latch: 0, loaned_samples: 0, qos: crate::common::test_qos() };
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
                definition.as_ptr(),
                0,
                &raw const pub_qos,
                &raw mut publisher,
            )
        },
        HyphaStatus::Ok
    );

    let sub_qos = HyphaSubscriberOptions {
        depth: 4,
        no_publisher_warning_after_nanos: 0,
        qos: hypha_ffi::HyphaQos::default(),
    };
    let mut subscriber = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_subscribe_typed(
                node,
                topic.as_ptr(),
                topic.as_bytes().len(),
                std::ptr::null(),
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

unsafe fn send_sample(publisher: *mut HyphaPublisher, data: &[u8]) {
    let mut loan = ptr::null_mut();
    let mut bytes = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_publisher_loan_frame(publisher, data.len(), &raw mut loan, &raw mut bytes) },
        HyphaStatus::Ok
    );
    unsafe { ptr::copy_nonoverlapping(data.as_ptr(), bytes, data.len()) };
    assert_eq!(unsafe { hypha_loan_send(loan, -1) }, HyphaStatus::Ok);
}

struct SimulatedNode {
    node: *mut HyphaNode,
    _serial: MutexGuard<'static, ()>,
}

impl SimulatedNode {
    fn with_parameters(label: &str, start_nanos: i64, parameters: Option<&Path>) -> Self {
        let serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let node = create_node_with_file(&unique(label), parameters);
        assert_eq!(hypha_clock_enable_sim_time(0), HyphaStatus::Ok as i32);
        let simulated = Self { node, _serial: serial };
        assert_eq!(hypha_clock_set_sim_time(start_nanos), HyphaStatus::Ok as i32);
        simulated
    }
}

impl Drop for SimulatedNode {
    fn drop(&mut self) {
        unsafe {
            assert_eq!(hypha_clock_disable_sim_time(), HyphaStatus::Ok as i32);
            hypha_node_destroy(self.node);
        }
    }
}

fn run_waitset_for(waitset: *mut HyphaWaitSet, nanos: u64) {
    let mut report =
        HyphaWaitSetRunReport { reason: 0, status: 0, attachment: 0, slow_callbacks: 0 };
    let _ = unsafe { hypha_waitset_run(waitset, nanos, &raw mut report) };
}

/// Advances simulated time one slice at a time and runs the `WaitSet` for it.
struct SimDriver {
    waitset: *mut HyphaWaitSet,
    now: i64,
}

impl SimDriver {
    fn slice(&mut self) {
        self.now += SLICE_NANOS;
        assert_eq!(hypha_clock_set_sim_time(self.now), HyphaStatus::Ok as i32);
        run_waitset_for(self.waitset, 10_000_000);
    }
}

/// One image report as read by the step callback.
#[derive(Clone, Copy, Default)]
struct Tick {
    callback_start_nanos: i64,
    received_total: u64,
    report_status: i32,
    report: HyphaSupervisionReport,
    null_image_status: i32,
    unknown_binding_status: i32,
    null_out_status: i32,
    null_alive_status: i32,
    unknown_alive_status: i32,
    null_ignore_status: i32,
    unknown_ignore_status: i32,
}

/// Every refused image call of one tick must report `InvalidArgument`.
fn assert_refused_image_calls(tick: &Tick) {
    let refused = [
        tick.null_image_status,
        tick.unknown_binding_status,
        tick.null_out_status,
        tick.null_alive_status,
        tick.unknown_alive_status,
        tick.null_ignore_status,
        tick.unknown_ignore_status,
    ];
    for status in refused {
        assert_eq!(status, HyphaStatus::InvalidArgument as i32);
    }
}

/// Step inputs and recorded ticks. The test thread sets the flags between slices.
#[derive(Default)]
struct Context {
    ticks: Mutex<Vec<Tick>>,
    publish_alive: AtomicBool,
    alive_value: AtomicU64,
    /// Set by the test; the step applies it once and clears it.
    ignore: AtomicBool,
    /// Set by the test; the step clears the override once and clears the flag.
    restore: AtomicBool,
}

/// Reports the supervision of binding 0 and applies the flagged updates first.
extern "C" fn supervision_step(
    image: *const HyphaCycleImage,
    time: *const HyphaCycleTime,
    _emission: *mut HyphaCycleEmission,
    context: *mut c_void,
) -> HyphaCallbackResult {
    let ctx = unsafe { &*context.cast::<Context>() };
    if ctx.publish_alive.load(Ordering::SeqCst) {
        let value = ctx.alive_value.load(Ordering::SeqCst);
        assert_eq!(unsafe { hypha_cycle_image_alive(image, 0, value) }, HyphaStatus::Ok);
    }
    if ctx.ignore.swap(false, Ordering::SeqCst) {
        assert_eq!(unsafe { hypha_cycle_image_set_ignored(image, 0, 1) }, HyphaStatus::Ok);
    }
    if ctx.restore.swap(false, Ordering::SeqCst) {
        assert_eq!(unsafe { hypha_cycle_image_set_ignored(image, 0, 0) }, HyphaStatus::Ok);
    }

    let mut binding = HyphaCycleBindingView::default();
    let mut tick =
        Tick { callback_start_nanos: unsafe { (*time).callback_start_nanos }, ..Tick::default() };
    let mut scratch = HyphaCycleBindingView::default();
    tick.null_image_status =
        unsafe { hypha_cycle_image_binding(ptr::null(), 0, &raw mut scratch) } as i32;
    tick.report_status = unsafe { hypha_cycle_image_binding(image, 0, &raw mut binding) } as i32;
    tick.received_total = binding.held.received_total;
    tick.report = binding.supervision;
    tick.unknown_binding_status =
        unsafe { hypha_cycle_image_binding(image, 99, &raw mut scratch) } as i32;
    tick.null_out_status = unsafe { hypha_cycle_image_binding(image, 0, ptr::null_mut()) } as i32;
    tick.null_alive_status = unsafe { hypha_cycle_image_alive(ptr::null(), 0, 0) } as i32;
    tick.unknown_alive_status = unsafe { hypha_cycle_image_alive(image, 99, 0) } as i32;
    tick.null_ignore_status = unsafe { hypha_cycle_image_set_ignored(ptr::null(), 0, 1) } as i32;
    tick.unknown_ignore_status = unsafe { hypha_cycle_image_set_ignored(image, 99, 1) } as i32;
    if let Ok(mut ticks) = ctx.ticks.lock() {
        ticks.push(tick);
    }
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

fn recorded_ticks(context: &Context) -> Vec<Tick> {
    context.ticks.lock().map_or_else(|_| Vec::new(), |ticks| ticks.clone())
}

unsafe fn create_supervised_cycle(
    waitset: *mut HyphaWaitSet,
    label: &str,
    context: *mut c_void,
) -> *mut HyphaCycle {
    let name = c_text(label);
    let mut cycle = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                waitset,
                name.as_ptr(),
                name.as_bytes().len(),
                10_000_000,
                Some(supervision_step),
                context,
                None,
                &raw mut cycle,
            )
        },
        HyphaStatus::Ok
    );
    cycle
}

fn bind_options() -> HyphaCycleBindingOptions {
    HyphaCycleBindingOptions {
        mode: HyphaCycleInputMode::Latest as u32,
        max_samples_per_iteration: 1,
        optional: 0,
    }
}

/// A parameter file with one supervised input `speed`, whose table is `table`.
fn port_parameters(label: &str, table: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(unique(label));
    std::fs::create_dir_all(&dir).expect("create fixture directory");
    let path = dir.join("params.toml");
    let text = format!("[ports.inputs.speed]\ntopic = \"/speed\"\n{table}\n");
    std::fs::write(&path, text).expect("write parameter fixture");
    path
}

const SPEED_TABLE: &str = "silence_ms = 100\nstagnation_ms = 300";
const BOTH_CHECKS: (bool, bool, bool) = (true, true, true);

/// Run slices until `done` holds. `before_slice` runs before each slice.
fn slices_until(
    driver: &mut SimDriver,
    context: &Context,
    mut before_slice: impl FnMut(),
    done: impl Fn(&[Tick]) -> bool,
) -> bool {
    for _ in 0..MAX_SLICES {
        if done(&recorded_ticks(context)) {
            return true;
        }
        before_slice();
        driver.slice();
    }
    done(&recorded_ticks(context))
}

fn run_until(
    waitset: *mut HyphaWaitSet,
    bound: std::time::Duration,
    mut done: impl FnMut() -> bool,
) -> bool {
    let started = std::time::Instant::now();
    while !done() {
        if started.elapsed() >= bound {
            return false;
        }
        run_waitset_for(waitset, 10_000_000);
    }
    true
}

#[test]
fn supervision_calls_refuse_bad_input_and_calls_after_commit() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let parameters = port_parameters("sup_refuse", SPEED_TABLE);
    let node = create_node_with_file(&unique("sup_refuse"), Some(&parameters));
    let waitset = unsafe { create_waitset(node) };
    let (pub_in, sub_in) = unsafe { create_pubsub(node, &format!("/{}", unique("refuse_in"))) };
    let mut source = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_input_source_create(waitset, sub_in, &raw mut source) },
        HyphaStatus::Ok
    );

    let context = Box::new(Context::default());
    let cycle = unsafe {
        create_supervised_cycle(waitset, "refuse_cycle", (&raw const *context).cast_mut().cast())
    };
    let bind_opts = bind_options();
    let mut in_idx = 0;
    assert_eq!(
        unsafe { hypha_cycle_bind_input(cycle, source, &raw const bind_opts, &raw mut in_idx) },
        HyphaStatus::Ok
    );
    let null_cycle = unsafe { bind(ptr::null_mut(), node, 0, "speed", BOTH_CHECKS) };
    assert_eq!(null_cycle.status, HyphaStatus::InvalidArgument, "null cycle");
    let unknown = unsafe { bind(cycle, node, 7, "speed", BOTH_CHECKS) };
    assert_eq!(unknown.status, HyphaStatus::InvalidArgument, "unknown binding");
    let valid = unsafe { bind(cycle, node, 0, "speed", BOTH_CHECKS) };
    assert_eq!(valid.status, HyphaStatus::Ok, "{}", valid.detail);
    assert_eq!(valid.attached, 1);

    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);
    let after_commit = unsafe { bind(cycle, node, 0, "speed", BOTH_CHECKS) };
    assert_eq!(after_commit.status, HyphaStatus::InvalidState, "a call after commit");

    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_input_source_destroy(source) }, HyphaStatus::Ok);
    unsafe {
        hypha_publisher_destroy(pub_in);
        hypha_node_destroy(node);
    }
}

#[test]
fn a_disabled_source_accepts_supervision_and_reports_no_faults() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let parameters = port_parameters("sup_disabled", SPEED_TABLE);
    let node = create_node_with_file(&unique("sup_disabled"), Some(&parameters));
    let waitset = unsafe { create_waitset(node) };
    let mut source = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_input_source_create(waitset, ptr::null_mut(), &raw mut source) },
        HyphaStatus::Ok
    );

    let context = Box::new(Context::default());
    let cycle = unsafe {
        create_supervised_cycle(waitset, "disabled_cycle", (&raw const *context).cast_mut().cast())
    };
    let bind_opts = bind_options();
    let mut in_idx = 0;
    assert_eq!(
        unsafe { hypha_cycle_bind_input(cycle, source, &raw const bind_opts, &raw mut in_idx) },
        HyphaStatus::Ok
    );
    let disabled = unsafe { bind(cycle, node, 0, "speed", BOTH_CHECKS) };
    assert_eq!(disabled.status, HyphaStatus::Ok, "{}", disabled.detail);
    assert_eq!(disabled.attached, 0, "a disabled input attaches nothing");
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);

    assert!(run_until(waitset, POLL_BOUND, || recorded_ticks(&context).len() >= 3));
    for tick in recorded_ticks(&context) {
        assert_eq!(tick.report_status, HyphaStatus::Ok as i32);
        assert_eq!(tick.report.faults, 0, "a disabled input has no faults");
        assert_eq!(tick.report.ignored, 0, "a disabled input attaches no override");
        assert_refused_image_calls(&tick);
    }

    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_input_source_destroy(source) }, HyphaStatus::Ok);
    unsafe {
        hypha_node_destroy(node);
    }
}

#[path = "input_supervision_abi/image_report.rs"]
mod image_report;

#[path = "input_supervision_abi/bind_call.rs"]
mod bind_call;
