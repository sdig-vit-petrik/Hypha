//! A latest input on the host clock under simulated time takes data stamped on another
//! clock as it comes, and its losses never fail the run.
#![allow(clippy::arithmetic_side_effects, clippy::expect_used, clippy::unwrap_used)]

mod common;
use crate::common::replay::*;
use crate::common::{c_text, create_node, unique};

use std::ptr;
use std::time::Duration;

use hypha_ffi::waitset::*;
use hypha_ffi::*;

struct Bound {
    node: *mut HyphaNode,
    waitset: *mut HyphaWaitSet,
    publisher: *mut HyphaPublisher,
    source: *mut HyphaInputSource,
    cycle: *mut HyphaCycle,
    index: u32,
    recorder: Box<Recorder>,
}

/// One cycle on simulated time that reads one input through a four-deep queue.
fn bind(label: &str, mode: HyphaCycleInputMode) -> Bound {
    let node = create_node(&unique(label));
    let waitset = unsafe { create_waitset(node) };
    let topic = format!("/{}", unique(label));
    let (publisher, subscriber) = unsafe { create_pubsub(node, &topic, 4) };
    let mut source = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_input_source_create(waitset, subscriber, &raw mut source) },
        HyphaStatus::Ok
    );
    let recorder = Box::new(Recorder::new(Duration::ZERO));
    let name = c_text(&unique(label));
    let mut cycle = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                waitset,
                name.as_ptr(),
                name.as_bytes().len(),
                u64::try_from(PERIOD_NANOS).expect("period"),
                Some(record_step),
                (&raw const *recorder).cast_mut().cast(),
                None,
                &raw mut cycle,
            )
        },
        HyphaStatus::Ok
    );
    let options =
        HyphaCycleBindingOptions { mode: mode as u32, max_samples_per_iteration: 1, optional: 0 };
    let mut index = 0_u32;
    assert_eq!(
        unsafe { hypha_cycle_bind_input(cycle, source, &raw const options, &raw mut index) },
        HyphaStatus::Ok
    );
    Bound { node, waitset, publisher, source, cycle, index, recorder }
}

impl Drop for Bound {
    fn drop(&mut self) {
        unsafe {
            assert_eq!(hypha_cycle_destroy(self.cycle), HyphaStatus::Ok);
            assert_eq!(hypha_input_source_destroy(self.source), HyphaStatus::Ok);
            hypha_publisher_destroy(self.publisher);
            hypha_node_destroy(self.node);
        }
    }
}

#[test]
fn data_on_another_clock_is_taken_as_it_comes_and_its_losses_do_not_fail_the_run() {
    let _clock = SimulatedClock::start(0);
    let rig = bind("host_clock", HyphaCycleInputMode::Latest);
    assert_eq!(unsafe { hypha_cycle_set_host_clock(rig.cycle, rig.index) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_cycle_commit(rig.cycle) }, HyphaStatus::Ok);

    // One sample first, so the transport has a sequence to compare the burst with.
    unsafe { send_stamped(rig.publisher, 0, MILLISECOND) };
    set_clock(25 * MILLISECOND);
    run_for(rig.waitset, 5_000_000);

    // Sixteen more, stamped 10 s ahead of this clock; the four-deep queue keeps the last four.
    for value in 1..=16_u8 {
        unsafe {
            send_stamped(rig.publisher, value, 10 * SECOND + MILLISECOND * i64::from(value));
        };
    }
    set_clock(50 * MILLISECOND);
    run_for(rig.waitset, 5_000_000);

    let steps = rig.recorder.steps();
    let last = steps.last().expect("a step after the burst");
    assert_eq!(last.newest, Some(16), "the newest sample, whatever its stamp: {steps:?}");
    let mut binding = HyphaCycleBindingView::default();
    assert_eq!(
        unsafe { hypha_cycle_binding(rig.cycle, rig.index, &raw mut binding) },
        HyphaStatus::Ok
    );
    assert_eq!(binding.health.overrun_total, 0, "a superseded sample is no loss");
}

#[test]
fn only_an_uncommitted_latest_binding_takes_the_host_clock() {
    let _clock = SimulatedClock::start(0);
    let queued = bind("host_clock_queued", HyphaCycleInputMode::Queued);
    assert_eq!(
        unsafe { hypha_cycle_set_host_clock(queued.cycle, queued.index) },
        HyphaStatus::InvalidArgument,
        "a queued input needs every sample in data order"
    );
    assert_eq!(
        unsafe { hypha_cycle_set_host_clock(queued.cycle, queued.index + 1) },
        HyphaStatus::InvalidArgument,
        "no such binding"
    );
    assert_eq!(
        unsafe { hypha_cycle_set_host_clock(ptr::null_mut(), 0) },
        HyphaStatus::InvalidArgument
    );

    let latest = bind("host_clock_committed", HyphaCycleInputMode::Latest);
    assert_eq!(unsafe { hypha_cycle_commit(latest.cycle) }, HyphaStatus::Ok);
    assert_eq!(
        unsafe { hypha_cycle_set_host_clock(latest.cycle, latest.index) },
        HyphaStatus::InvalidState
    );
}
