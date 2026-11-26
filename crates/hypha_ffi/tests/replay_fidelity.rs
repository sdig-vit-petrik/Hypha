//! Replay on simulated time, through the C ABI.
#![allow(
    clippy::arithmetic_side_effects,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::unwrap_used
)]

mod common;
use crate::common::replay::*;
use crate::common::{c_text, create_node, unique};

use std::ffi::c_void;
use std::ptr;
use std::sync::Mutex;
use std::time::Duration;

use hypha_ffi::waitset::*;
use hypha_ffi::*;

#[test]
pub(crate) fn a_jump_of_300_periods_runs_300_steps_without_missed_periods() {
    let _clock = SimulatedClock::start(0);
    let rig = Rig::new("replay_jump", HyphaCycleInputMode::Latest, 0, (16, 1024));
    // 300 periods.
    set_clock(6 * SECOND);
    rig.settle();

    let steps = rig.steps();
    assert_eq!(steps.len(), 300, "an unannounced jump steps through every deadline");
    assert!(steps.iter().all(|step| step.missed == 0), "no period is skipped: {steps:?}");
    let last = steps.last().expect("the last step");
    assert_eq!(last.scheduled, 6 * SECOND, "the last step runs at the last deadline");
}

#[test]
pub(crate) fn a_throttled_step_with_overflow_fails_the_run_and_is_counted() {
    let _clock = SimulatedClock::start(0);
    let node = create_node(&unique("replay_fidelity"));
    let waitset = unsafe { create_waitset(node) };
    let topic = format!("/{}", unique("replay_lossy"));
    // A four-deep queue: sixteen sends overflow it.
    let (publisher, subscriber) = unsafe { create_pubsub(node, &topic, 4) };
    let mut source = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_input_source_create(waitset, subscriber, &raw mut source) },
        HyphaStatus::Ok
    );
    let recorder = Box::new(Recorder::new(Duration::from_millis(2)));
    let name = c_text(&unique("lossy_cycle"));
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
    let bind = HyphaCycleBindingOptions {
        mode: HyphaCycleInputMode::Queued as u32,
        max_samples_per_iteration: 64,
        optional: 0,
    };
    let mut index = 0_u32;
    assert_eq!(
        unsafe { hypha_cycle_bind_input(cycle, source, &raw const bind, &raw mut index) },
        HyphaStatus::Ok
    );
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);

    // One sample first, so the transport has a sequence to compare the burst with.
    unsafe { send_stamped(publisher, 0, MILLISECOND) };
    set_clock(25 * MILLISECOND);
    run_for(waitset, 5_000_000);

    // Sixteen more, stamped after the deadlines that have run: the queue keeps the
    // last four. The step then runs late.
    let warnings_before = hypha_log::health().lines_written;
    for value in 1..=16_u8 {
        unsafe {
            send_stamped(publisher, value, 30 * MILLISECOND + 10 * MILLISECOND * i64::from(value));
        };
    }
    set_clock(200 * MILLISECOND);
    // A step that lost samples is not faithful: the run stops with SequenceGap.
    let mut report =
        HyphaWaitSetRunReport { reason: 0, status: 0, attachment: 0, slow_callbacks: 0 };
    assert_eq!(
        unsafe { hypha_waitset_run(waitset, 100_000_000, &raw mut report) },
        HyphaStatus::Ok
    );
    assert_eq!(report.status, HyphaStatus::SequenceGap as i32, "the lossy run must fail");

    let mut binding = HyphaCycleBindingView::default();
    assert_eq!(unsafe { hypha_cycle_binding(cycle, 0, &raw mut binding) }, HyphaStatus::Ok);
    let health = binding.health;
    assert!(health.overrun_total > 0, "the losses are counted, never silent: {health:?}");
    assert!(
        hypha_log::health().lines_written > warnings_before,
        "the replay loss is written as an error"
    );

    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_input_source_destroy(source) }, HyphaStatus::Ok);
    unsafe {
        hypha_publisher_destroy(publisher);
        hypha_node_destroy(node);
    }
}

#[test]
pub(crate) fn a_latest_step_with_replay_loss_fails_the_run_and_is_counted() {
    let _clock = SimulatedClock::start(0);
    let node = create_node(&unique("replay_fidelity_latest"));
    let waitset = unsafe { create_waitset(node) };
    let topic = format!("/{}", unique("replay_lossy_latest"));
    // A four-deep queue: sixteen sends overflow it.
    let (publisher, subscriber) = unsafe { create_pubsub(node, &topic, 4) };
    let mut source = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_input_source_create(waitset, subscriber, &raw mut source) },
        HyphaStatus::Ok
    );
    let recorder = Box::new(Recorder::new(Duration::from_millis(2)));
    let name = c_text(&unique("lossy_cycle_latest"));
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
    let bind = HyphaCycleBindingOptions {
        mode: HyphaCycleInputMode::Latest as u32,
        max_samples_per_iteration: 1,
        optional: 0,
    };
    let mut index = 0_u32;
    assert_eq!(
        unsafe { hypha_cycle_bind_input(cycle, source, &raw const bind, &raw mut index) },
        HyphaStatus::Ok
    );
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);

    // One sample first, so the transport has a sequence to compare the burst with.
    unsafe { send_stamped(publisher, 0, MILLISECOND) };
    set_clock(25 * MILLISECOND);
    run_for(waitset, 5_000_000);

    // Sixteen more, stamped after the deadlines that have run: the queue keeps the
    // last four. The step then runs late.
    let warnings_before = hypha_log::health().lines_written;
    for value in 1..=16_u8 {
        unsafe {
            send_stamped(publisher, value, 30 * MILLISECOND + 10 * MILLISECOND * i64::from(value));
        };
    }
    set_clock(200 * MILLISECOND);
    // A step that lost samples is not faithful: the run stops with SequenceGap.
    let mut report =
        HyphaWaitSetRunReport { reason: 0, status: 0, attachment: 0, slow_callbacks: 0 };
    assert_eq!(
        unsafe { hypha_waitset_run(waitset, 100_000_000, &raw mut report) },
        HyphaStatus::Ok
    );
    assert_eq!(report.status, HyphaStatus::SequenceGap as i32, "the lossy run must fail");

    let mut binding = HyphaCycleBindingView::default();
    assert_eq!(unsafe { hypha_cycle_binding(cycle, 0, &raw mut binding) }, HyphaStatus::Ok);
    let health = binding.health;
    assert!(health.overrun_total > 0, "the losses are counted, never silent: {health:?}");
    assert!(
        hypha_log::health().lines_written > warnings_before,
        "the replay loss is written as an error"
    );

    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_input_source_destroy(source) }, HyphaStatus::Ok);
    unsafe {
        hypha_publisher_destroy(publisher);
        hypha_node_destroy(node);
    }
}

/// What a `Latest` input on a 100 ms cycle sees when `hypha play --chunk 100` replays a
/// 100 Hz topic whose chunk grid starts 10 ms past the deadline grid: the status of each
/// run, the binding's losses and the steps.
fn replay_a_latest_input_off_the_deadline_grid(queue: usize) -> (Vec<i32>, u64, Vec<Step>) {
    const CYCLE_PERIOD: i64 = 100 * MILLISECOND;
    let _clock = SimulatedClock::start(10 * MILLISECOND);
    let node = create_node(&unique("replay_sizing_latest"));
    let waitset = unsafe { create_waitset(node) };
    let topic = format!("/{}", unique("replay_sizing_latest"));
    let (publisher, subscriber) = unsafe { create_pubsub(node, &topic, queue) };
    let mut source = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_input_source_create(waitset, subscriber, &raw mut source) },
        HyphaStatus::Ok
    );
    let recorder = Box::new(Recorder::new(Duration::ZERO));
    let name = c_text(&unique("sizing_cycle_latest"));
    let mut cycle = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                waitset,
                name.as_ptr(),
                name.as_bytes().len(),
                u64::try_from(CYCLE_PERIOD).expect("period"),
                Some(record_step),
                (&raw const *recorder).cast_mut().cast(),
                None,
                &raw mut cycle,
            )
        },
        HyphaStatus::Ok
    );
    let bind = HyphaCycleBindingOptions {
        mode: HyphaCycleInputMode::Latest as u32,
        max_samples_per_iteration: 1,
        optional: 0,
    };
    let mut index = 0_u32;
    assert_eq!(
        unsafe { hypha_cycle_bind_input(cycle, source, &raw const bind, &raw mut index) },
        HyphaStatus::Ok
    );
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);

    // The player's order: the start barrier's clock at 10 ms, then each chunk's samples,
    // its own boundary included, before the clock that closes it.
    let mut statuses = Vec::new();
    for close in [110, 210, 310, 410] {
        for stamp in (close - 100..=close).step_by(10).filter(|stamp| *stamp >= 10) {
            let value = u8::try_from(stamp / 10).expect("a small value");
            unsafe { send_stamped(publisher, value, stamp * MILLISECOND) };
        }
        set_clock(close * MILLISECOND);
        statuses.push(run_status(waitset, 5_000_000));
    }

    let mut binding = HyphaCycleBindingView::default();
    assert_eq!(unsafe { hypha_cycle_binding(cycle, 0, &raw mut binding) }, HyphaStatus::Ok);
    let steps = recorder.steps();
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_input_source_destroy(source) }, HyphaStatus::Ok);
    unsafe {
        hypha_publisher_destroy(publisher);
        hypha_node_destroy(node);
    }
    (statuses, binding.health.overrun_total, steps)
}

#[test]
pub(crate) fn a_latest_input_sized_for_a_chunk_and_a_period_keeps_up_off_the_deadline_grid() {
    // (100 ms chunk + 100 ms period) x 100 Hz: the documented size of a `Latest` input.
    let (statuses, overruns, steps) = replay_a_latest_input_off_the_deadline_grid(20);
    assert!(statuses.iter().all(|status| *status == HyphaStatus::Ok as i32), "{statuses:?}");
    assert_eq!(overruns, 0, "no sample is lost");
    let seen: Vec<(i64, Option<u8>)> =
        steps.iter().map(|step| (step.scheduled / MILLISECOND, step.newest)).collect();
    // Each step takes the newest sample before its deadline.
    assert_eq!(seen, [(100, Some(9)), (200, Some(19)), (300, Some(29)), (400, Some(39))]);
}

#[test]
pub(crate) fn a_plain_timer_still_fires_once_and_counts_the_missed_periods() {
    extern "C" fn count(missed: u64, context: *mut c_void) -> HyphaCallbackResult {
        let slot = unsafe { &*context.cast::<Mutex<Vec<u64>>>() };
        slot.lock().expect("missed").push(missed);
        HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
    }
    let _clock = SimulatedClock::start(0);
    let node = create_node(&unique("plain_timer"));
    let waitset = unsafe { create_waitset(node) };
    let seen: Box<Mutex<Vec<u64>>> = Box::new(Mutex::new(Vec::new()));
    let mut attachment: HyphaAttachmentId = 0;
    assert_eq!(
        unsafe {
            hypha_waitset_attach(
                waitset,
                &crate::common::attach::timer(
                    u64::try_from(PERIOD_NANOS).expect("period"),
                    Some(count),
                    (&raw const *seen).cast_mut().cast(),
                    None,
                ),
                &raw mut attachment,
            )
        },
        HyphaStatus::Ok
    );
    // Ten periods at once: one callback that counts the nine it did not run.
    set_clock(10 * PERIOD_NANOS);
    run_for(waitset, 5_000_000);
    assert_eq!(seen.lock().expect("missed").as_slice(), &[9_u64]);
    unsafe {
        hypha_node_destroy(node);
    }
}

#[test]
pub(crate) fn a_long_jump_is_stepped_in_bounded_slices_with_every_deadline_once() {
    let _clock = SimulatedClock::start(0);
    let rig = Rig::new("replay_slices", HyphaCycleInputMode::Latest, 0, (16, 1024));
    // 3000 periods of 20 ms: three slices of at most 1024 steps.
    set_clock(60 * SECOND);
    let mut runs = 0;
    while rig.steps().len() < 3000 && runs < 100 {
        rig.settle();
        runs += 1;
    }
    let steps = rig.steps();
    assert_eq!(steps.len(), 3000, "every deadline runs once, over {runs} runs");
    assert!(steps.iter().all(|step| step.missed == 0), "no period is skipped");
    for (index, step) in steps.iter().enumerate() {
        let expected = (i64::try_from(index).expect("index") + 1) * 20_000_000;
        assert_eq!(step.scheduled, expected, "step {index} runs at its own deadline");
    }
}

#[test]
pub(crate) fn a_bounded_run_ends_after_one_slice_of_a_long_jump() {
    let _clock = SimulatedClock::start(0);
    // A step of 2 ms: the 50 ms slice budget ends a turn after about 25 steps.
    let rig = Rig::with_step_time(
        "replay_slice_budget",
        HyphaCycleInputMode::Latest,
        0,
        (16, 1024),
        Duration::from_millis(2),
    );
    set_clock(4 * SECOND);
    rig.settle();
    let ran = rig.steps().len();
    assert!(ran >= 1, "a turn runs at least one step");
    // 200 steps of 2 ms each outlast the 5 ms slice: fewer ran, so the slice ended the turn.
    assert!(ran < 200, "the 5 ms run ended after its slice, not after all 200 steps: {ran}");
    let mut runs = 1;
    while rig.steps().len() < 200 && runs < 100 {
        rig.settle();
        runs += 1;
    }
    assert_eq!(rig.steps().len(), 200, "the rest runs on later turns");
}
