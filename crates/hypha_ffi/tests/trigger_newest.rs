//! An input bound newest only, `HYPHA_CYCLE_INPUT_MODE_NEWEST` (#210): ROS's subscriber
//! queue of one, decided by the reader alone, with no change to the topic's policy.
//!
//! Which sample steps, on either clock, is decided in `hypha_runtime::trigger_schedule`, and
//! the count of the passed samples in `hypha_cycle`. These cases wire it through the C ABI: a
//! slow step on a fast topic leaves a backlog, and the step after it holds only the newest
//! sample. Every case holds by construction: the backlog is sent by the step itself, before
//! it returns.

#![allow(
    clippy::arithmetic_side_effects,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used,
    reason = "a test asserts on known values and fails loudly"
)]

use std::ffi::c_void;
use std::ptr;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

mod common;
use crate::common::c_text;
use crate::common::replay::{
    SERIAL, open_declared_publisher, open_subscriber, run_for, send_payload,
};
use crate::common::supervision::{bind_trigger, node_with_triggers};
use crate::common::unique;
use hypha_ffi::waitset::*;
use hypha_ffi::*;

const MS: i64 = 1_000_000;
const TYPE: &str = "std_msgs/Counter";
const MD5: &str = "d9d213e8c146065ec958333c617eb1a6";
/// The trigger's silence limit: ten seconds, so no sample goes stale and no fallback runs.
const LIMIT_MS: i64 = 10_000;
/// The longest a case waits for a step.
const BOUND: Duration = Duration::from_secs(20);
/// The samples a queue keeps between steps, as an `Arrivals` port's `perStep()` does.
const QUEUE: usize = 8;

/// What one step saw of its input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Seen {
    /// The first byte of each sample the step holds: at most one for a trigger.
    newest: Option<u8>,
    samples: usize,
    /// Samples the input left on purpose since the step before.
    dropped: u64,
    dropped_total: u64,
}

#[derive(Default)]
struct Context {
    steps: Mutex<Vec<Seen>>,
    /// The publisher of the input, and the values the first step sends on it: the backlog a
    /// slow step finds when it returns.
    publisher: Mutex<Option<(usize, Vec<u8>)>>,
}

impl Context {
    fn steps(&self) -> Vec<Seen> {
        self.steps.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

extern "C" fn record(
    image: *const HyphaCycleImage,
    _time: *const HyphaCycleTime,
    _emission: *mut HyphaCycleEmission,
    context: *mut c_void,
) -> HyphaCallbackResult {
    let context = unsafe { &*context.cast::<Context>() };
    let mut binding = HyphaCycleBindingView::default();
    assert_eq!(unsafe { hypha_cycle_image_binding(image, 0, &raw mut binding) }, HyphaStatus::Ok);
    let report = binding.health.capture;
    let newest = (report.sample_count > 0).then(|| {
        let mut view = HyphaCycleSampleView {
            payload: ptr::null(),
            payload_len: 0,
            transport_sent_nanos: 0,
            sequence: 0,
            publisher_id: 0,
            publisher_sequence: 0,
        };
        let last = report.sample_count - 1;
        assert_eq!(
            unsafe { hypha_cycle_image_sample(image, 0, last, &raw mut view) },
            HyphaStatus::Ok
        );
        unsafe { *view.payload }
    });
    let seen = Seen {
        newest,
        samples: report.sample_count,
        dropped: report.work_limit_drops,
        dropped_total: binding.health.work_limit_drops_total,
    };
    context.steps.lock().unwrap_or_else(PoisonError::into_inner).push(seen);
    // The topic is faster than the step: while it works, more samples arrive.
    let backlog = context.publisher.lock().unwrap_or_else(PoisonError::into_inner).take();
    if let Some((publisher, values)) = backlog {
        for value in values {
            unsafe { send_payload(publisher as *mut HyphaPublisher, &[value]) };
        }
    }
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

/// One cycle with one input on a fresh topic: a trigger when `period_nanos` is zero, else
/// an input of a rate cycle.
struct Rig {
    node: *mut HyphaNode,
    waitset: *mut HyphaWaitSet,
    cycle: *mut HyphaCycle,
    source: *mut HyphaInputSource,
    publisher: *mut HyphaPublisher,
    context: Box<Context>,
}

impl Rig {
    fn new(label: &str, mode: HyphaCycleInputMode, period_nanos: u64) -> Self {
        let node = node_with_triggers(label, &[LIMIT_MS]);
        let mut waitset = ptr::null_mut();
        assert_eq!(unsafe { hypha_node_default_waitset(node, &raw mut waitset) }, HyphaStatus::Ok);
        let topic = format!("/{}", unique("newest_in"));
        let publisher = unsafe { open_declared_publisher(node, &topic, TYPE, MD5) };
        // The subscriber queues QUEUE samples, as many as the binding keeps.
        let subscriber = unsafe { open_subscriber(node, &topic, MD5, QUEUE) };
        let mut source = ptr::null_mut();
        assert_eq!(
            unsafe { hypha_input_source_create(waitset, subscriber, &raw mut source) },
            HyphaStatus::Ok
        );
        let context = Box::<Context>::default();
        let name = c_text(&unique("newest"));
        let mut cycle = ptr::null_mut();
        assert_eq!(
            unsafe {
                hypha_cycle_create(
                    waitset,
                    name.as_ptr(),
                    name.as_bytes().len(),
                    period_nanos,
                    Some(record),
                    (&raw const *context).cast_mut().cast::<c_void>(),
                    None,
                    &raw mut cycle,
                )
            },
            HyphaStatus::Ok
        );
        let options = HyphaCycleBindingOptions {
            mode: mode as u32,
            max_samples_per_iteration: QUEUE,
            optional: 0,
        };
        let mut index = 0_u32;
        assert_eq!(
            unsafe { hypha_cycle_bind_input(cycle, source, &raw const options, &raw mut index) },
            HyphaStatus::Ok
        );
        if period_nanos == 0 {
            let bound = unsafe { bind_trigger(cycle, node, index, LIMIT_MS) };
            assert_eq!(bound.status, HyphaStatus::Ok, "{}", bound.detail);
            assert_eq!(unsafe { hypha_cycle_set_trigger(cycle, index) }, HyphaStatus::Ok);
        }
        assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);
        Self { node, waitset, cycle, source, publisher, context }
    }

    /// The first step sends `values`: the samples that arrive while it works.
    fn backlog_of_first_step(&self, values: &[u8]) {
        let backlog = (self.publisher as usize, values.to_vec());
        *self.context.publisher.lock().unwrap_or_else(PoisonError::into_inner) = Some(backlog);
    }

    /// Run the `WaitSet` in short slices until `count` steps ran, then a little longer, so
    /// that a step too many would show.
    fn run_until_steps(&self, count: usize) -> Vec<Seen> {
        let started = Instant::now();
        while self.context.steps().len() < count {
            assert!(started.elapsed() < BOUND, "{count} steps did not run: {:?}", self.steps());
            run_for(self.waitset, 5_000_000);
        }
        run_for(self.waitset, 30_000_000);
        self.context.steps()
    }

    fn steps(&self) -> Vec<Seen> {
        self.context.steps()
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        unsafe {
            hypha_cycle_destroy(self.cycle);
            hypha_input_source_destroy(self.source);
            hypha_publisher_destroy(self.publisher);
            hypha_node_destroy(self.node);
        }
    }
}

/// The values the steps held, in order.
fn values(steps: &[Seen]) -> Vec<Option<u8>> {
    steps.iter().map(|step| step.newest).collect()
}

#[test]
fn on_host_time_a_newest_only_trigger_steps_on_the_newest_sample_a_slow_step_left_behind() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let rig = Rig::new("newest_host_trigger", HyphaCycleInputMode::Newest, 0);
    rig.backlog_of_first_step(&[2, 3, 4, 5]);
    unsafe { send_payload(rig.publisher, &[1]) };

    let steps = rig.run_until_steps(2);

    assert_eq!(values(&steps), [Some(1), Some(5)], "{steps:?}");
    // The three samples passed for the newest are counted, on the step that passed them
    // and in the input's cumulative count, which ~/state adds to its `dropped`.
    assert_eq!((steps[0].dropped, steps[0].dropped_total), (0, 0), "{steps:?}");
    assert_eq!((steps[1].dropped, steps[1].dropped_total), (3, 3), "{steps:?}");
    assert_eq!(steps[1].samples, 1, "{steps:?}");
}

#[test]
fn on_host_time_a_newest_only_input_of_a_rate_cycle_takes_its_newest_sample() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let rig = Rig::new("newest_host_rate", HyphaCycleInputMode::Newest, 20 * MS as u64);
    rig.backlog_of_first_step(&[2, 3, 4, 5]);

    // The first step runs on an empty input and sends the backlog; a later one finds it.
    let steps = rig.run_until_steps(2);
    let found = steps.iter().find(|step| step.samples > 0).expect("a step found the backlog");

    assert_eq!((found.samples, found.newest), (1, Some(5)), "{steps:?}");
    assert_eq!((found.dropped, found.dropped_total), (3, 3), "{steps:?}");
    assert!(steps.iter().all(|step| step.samples <= 1), "{steps:?}");
}

#[test]
fn newest_mode_needs_a_queue() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let node = node_with_triggers("newest_zero", &[LIMIT_MS]);
    let mut waitset = ptr::null_mut();
    assert_eq!(unsafe { hypha_node_default_waitset(node, &raw mut waitset) }, HyphaStatus::Ok);
    let topic = format!("/{}", unique("newest_zero"));
    let subscriber = unsafe { open_subscriber(node, &topic, MD5, QUEUE) };
    let mut source = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_input_source_create(waitset, subscriber, &raw mut source) },
        HyphaStatus::Ok
    );
    let name = c_text(&unique("newest_zero"));
    let mut cycle = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                waitset,
                name.as_ptr(),
                name.as_bytes().len(),
                0,
                Some(record),
                ptr::null_mut(),
                None,
                &raw mut cycle,
            )
        },
        HyphaStatus::Ok
    );
    let options = HyphaCycleBindingOptions {
        mode: HyphaCycleInputMode::Newest as u32,
        max_samples_per_iteration: 0,
        optional: 0,
    };
    let mut index = u32::MAX;
    assert_eq!(
        unsafe { hypha_cycle_bind_input(cycle, source, &raw const options, &raw mut index) },
        HyphaStatus::InvalidArgument
    );
    unsafe {
        hypha_cycle_destroy(cycle);
        hypha_input_source_destroy(source);
        hypha_node_destroy(node);
    }
}
