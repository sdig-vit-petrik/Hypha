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

/// A `Latest` binding.
const LATEST: HyphaCycleBindingOptions = HyphaCycleBindingOptions {
    mode: HyphaCycleInputMode::Latest as u32,
    max_samples_per_iteration: 1,
    optional: 0,
};

/// What an input bound as `bind` with a queue of `queue` on a 100 ms cycle sees of a
/// 100 Hz topic whose samples and `/clock` arrive as `chunks` gives them, from a first
/// clock at `start` ms: each chunk is the stamps of its samples, in ms, then the `/clock`
/// that closes it. The status of each run, the binding's health and the steps.
fn replay_an_input(
    bind: HyphaCycleBindingOptions,
    queue: usize,
    start: i64,
    chunks: &[(Vec<i64>, i64)],
) -> (Vec<i32>, HyphaCycleBindingHealth, Vec<Step>) {
    const CYCLE_PERIOD: i64 = 100 * MILLISECOND;
    let _clock = SimulatedClock::start(start * MILLISECOND);
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
    let mut index = 0_u32;
    assert_eq!(
        unsafe { hypha_cycle_bind_input(cycle, source, &raw const bind, &raw mut index) },
        HyphaStatus::Ok
    );
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);

    let mut statuses = Vec::new();
    for (stamps, close) in chunks {
        for stamp in stamps {
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
    (statuses, binding.health, steps)
}

/// The stamps from `first` through `last` ms, every 10 ms.
fn every_10_ms(first: i64, last: i64) -> Vec<i64> {
    (first..=last).step_by(10).collect()
}

/// The player's chunks of `hypha play --chunk 100` for a 100 Hz topic stamped on
/// multiples of 10 ms, from `start` ms through the boundaries `closes`: each closed at
/// `T` holds every sample stamped at or before `T`, the first one its start too.
fn chunks_of_100_ms(start: i64, closes: &[i64]) -> Vec<(Vec<i64>, i64)> {
    let mut first = start;
    closes
        .iter()
        .map(|&close| {
            let stamps = every_10_ms(first, close);
            first = close + 10;
            (stamps, close)
        })
        .collect()
}

/// Each step's deadline and the newest sample it took, in ms and as the sample's value.
fn deadlines_and_values(steps: &[Step]) -> Vec<(i64, Option<u8>)> {
    steps.iter().map(|step| (step.scheduled / MILLISECOND, step.newest)).collect()
}

#[test]
pub(crate) fn a_latest_input_sized_for_a_chunk_and_a_period_keeps_up_off_the_deadline_grid() {
    // (100 ms chunk + 100 ms period) x 100 Hz: the documented size of a `Latest` input.
    let chunks = chunks_of_100_ms(10, &[110, 210, 310, 410]);
    let (statuses, health, steps) = replay_an_input(LATEST, 20, 10, &chunks);
    assert!(statuses.iter().all(|status| *status == HyphaStatus::Ok as i32), "{statuses:?}");
    assert_eq!(health.overrun_total, 0, "no sample is lost: {health:?}");
    // Each step takes the newest sample before its deadline.
    assert_eq!(
        deadlines_and_values(&steps),
        [(100, Some(9)), (200, Some(19)), (300, Some(29)), (400, Some(39))]
    );
}

#[test]
pub(crate) fn a_queued_input_sized_for_a_chunk_and_a_period_keeps_up_off_the_deadline_grid() {
    // (100 ms chunk + 100 ms period) x 100 Hz, and a `perStep` of 100 ms x 100 Hz.
    let queued = HyphaCycleBindingOptions {
        mode: HyphaCycleInputMode::Queued as u32,
        max_samples_per_iteration: 10,
        optional: 0,
    };
    let chunks = chunks_of_100_ms(10, &[110, 210, 310, 410]);
    let (statuses, health, steps) = replay_an_input(queued, 20, 10, &chunks);
    assert!(statuses.iter().all(|status| *status == HyphaStatus::Ok as i32), "{statuses:?}");
    assert_eq!(health.overrun_total, 0, "no sample is lost: {health:?}");
    let taken: Vec<(i64, usize, Option<u8>)> = steps
        .iter()
        .map(|step| (step.scheduled / MILLISECOND, step.sample_count, step.newest))
        .collect();
    // Each step takes every sample stamped from the last deadline up to its own.
    assert_eq!(
        taken,
        [(100, 9, Some(9)), (200, 10, Some(19)), (300, 10, Some(29)), (400, 10, Some(39))]
    );
}

#[test]
pub(crate) fn a_latest_queue_of_one_chunk_drops_a_sample_off_the_deadline_grid() {
    // A queue of 100 ms x 100 Hz: the player shortens the first chunk, of 11 samples, at
    // 109 ms (4.3). The `/clock` at 110 runs no step, so nothing drains 110 before the
    // step at 200, and the queue of 10 cannot hold 110..210. The capture loses nothing,
    // and the step at 200 still takes 190, but the transport gap counts all the same.
    // This pins the known limitation that issue #222 leaves open: a gap of a sample that
    // the step's selection supersedes is no loss either, and the fix of that half flips
    // the run at 210 to Ok and `overrun_total` to 0.
    let chunks = [
        (every_10_ms(10, 100), 109),
        (vec![110], 110),
        (every_10_ms(120, 210), 210),
        (every_10_ms(220, 310), 310),
    ];
    let (statuses, health, steps) = replay_an_input(LATEST, 10, 10, &chunks);
    let ok = HyphaStatus::Ok as i32;
    assert_eq!(statuses, [ok, ok, HyphaStatus::SequenceGap as i32, ok], "{health:?}");
    assert_eq!(health.capture.lost_total, 0, "no eviction is a loss: {health:?}");
    assert_eq!(health.overrun_total, 1, "one transport gap: {health:?}");
    assert_eq!(deadlines_and_values(&steps), [(100, Some(9)), (200, Some(19)), (300, Some(29))]);
}

#[test]
pub(crate) fn a_latest_queue_that_fits_each_drain_keeps_up_off_the_deadline_grid() {
    // A queue of 11, the first chunk's samples, so no chunk is shortened and each drain,
    // the samples from one step's `/clock` to the next, fits. The history of 12 then cannot hold 100..220: the drain at 220 ms evicts 100,
    // which the step at 100 left unread and 190 supersedes at 200, and the one at 320 ms
    // evicts 200, which 290 supersedes at 300. No step could take either, so nothing is
    // lost.
    let chunks = chunks_of_100_ms(20, &[120, 220, 320]);
    let (statuses, health, steps) = replay_an_input(LATEST, 11, 20, &chunks);
    let all_ok = statuses.iter().all(|status| *status == HyphaStatus::Ok as i32);
    assert!(all_ok, "{statuses:?}, {health:?}");
    assert_eq!(health.overrun_total, 0, "no sample is lost: {health:?}");
    assert_eq!(deadlines_and_values(&steps), [(100, Some(9)), (200, Some(19)), (300, Some(29))]);
}

#[test]
pub(crate) fn a_latest_input_that_lost_the_sample_a_step_is_due_fails_the_step() {
    // A publisher that stamps ahead of the clock, which the player never does: the drain
    // at 210 ms evicts 190, the newest sample before 200, and every retained sample is
    // stamped at or after 200. Each drain fits the queue, so the transport loses nothing.
    let mut early = vec![190];
    early.extend(every_10_ms(200, 280));
    let chunks = [(early, 109), (vec![290, 300], 210)];
    let (statuses, health, steps) = replay_an_input(LATEST, 10, 10, &chunks);
    assert_eq!(statuses, [HyphaStatus::Ok as i32, HyphaStatus::SequenceGap as i32]);
    assert_eq!(health.capture.lost_total, 1, "{health:?}");
    assert_eq!(health.overrun_total, 1, "no transport gap: {health:?}");
    assert_eq!(deadlines_and_values(&steps), [(100, None), (200, None)]);
}

/// The step's context: it records each step and, at 200 ms, rejects the newest sample
/// of binding `reject` as undecodable.
struct RejectAt200 {
    recorder: Recorder,
    reject: u32,
}

extern "C" fn record_and_reject_at_200(
    image: *const HyphaCycleImage,
    time: *const HyphaCycleTime,
    emission: *mut HyphaCycleEmission,
    context: *mut c_void,
) -> HyphaCallbackResult {
    let rejecting = unsafe { &*context.cast::<RejectAt200>() };
    let recorded = (&raw const rejecting.recorder).cast_mut().cast();
    let result = record_step(image, time, emission, recorded);
    if unsafe { (*time).scheduled_nanos } != 200 * MILLISECOND {
        return result;
    }
    let mut binding = HyphaCycleBindingView::default();
    assert_eq!(
        unsafe { hypha_cycle_image_binding(image, rejecting.reject, &raw mut binding) },
        HyphaStatus::Ok
    );
    let count = binding.health.capture.sample_count;
    assert!(count > 0, "the step at 200 holds a sample to reject");
    let mut view = HyphaCycleSampleView {
        payload: ptr::null(),
        payload_len: 0,
        transport_sent_nanos: 0,
        sequence: 0,
        publisher_id: 0,
        publisher_sequence: 0,
    };
    assert_eq!(
        unsafe { hypha_cycle_image_sample(image, rejecting.reject, count - 1, &raw mut view) },
        HyphaStatus::Ok
    );
    let mut after = HyphaCycleBindingView::default();
    assert_eq!(
        unsafe {
            hypha_cycle_image_reject_sample(image, rejecting.reject, view.sequence, &raw mut after)
        },
        HyphaStatus::Ok
    );
    result
}

/// A `Latest` input with a queue of `queue` on a 100 ms cycle, whose step at 200 ms
/// rejects the sample stamped 190 it took. With `co_reader` the step rejects it as the
/// sample of a queued binding on the same source instead. The step at 100 takes 90 of
/// 80 and 90, the one at 200 takes 190 and passes 180, and 300 and 305, stamped at or
/// after 300, then arrive. Returns the status of each run, the latest binding's view
/// and the steps.
fn replay_a_rejection_at_200(
    queue: usize,
    co_reader: bool,
) -> (Vec<i32>, HyphaCycleBindingView, Vec<Step>) {
    let _clock = SimulatedClock::start(10 * MILLISECOND);
    let node = create_node(&unique("replay_reject_latest"));
    let waitset = unsafe { create_waitset(node) };
    let topic = format!("/{}", unique("replay_reject_latest"));
    let (publisher, subscriber) = unsafe { create_pubsub(node, &topic, queue) };
    let mut source = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_input_source_create(waitset, subscriber, &raw mut source) },
        HyphaStatus::Ok
    );
    let context = Box::new(RejectAt200 {
        recorder: Recorder::new(Duration::ZERO),
        reject: u32::from(co_reader),
    });
    let name = c_text(&unique("reject_cycle_latest"));
    let mut cycle = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                waitset,
                name.as_ptr(),
                name.as_bytes().len(),
                100_000_000,
                Some(record_and_reject_at_200),
                (&raw const *context).cast_mut().cast(),
                None,
                &raw mut cycle,
            )
        },
        HyphaStatus::Ok
    );
    let latest = LATEST;
    let mut index = 0_u32;
    assert_eq!(
        unsafe { hypha_cycle_bind_input(cycle, source, &raw const latest, &raw mut index) },
        HyphaStatus::Ok
    );
    if co_reader {
        let queued = HyphaCycleBindingOptions {
            mode: HyphaCycleInputMode::Queued as u32,
            max_samples_per_iteration: 1,
            optional: 0,
        };
        assert_eq!(
            unsafe { hypha_cycle_bind_input(cycle, source, &raw const queued, &raw mut index) },
            HyphaStatus::Ok
        );
    }
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);

    let chunks = [(vec![80, 90], 109), (vec![180, 190], 210), (vec![300, 305], 310)];
    let mut statuses = Vec::new();
    for (stamps, close) in chunks {
        for stamp in stamps {
            let value = u8::try_from(stamp / 10).expect("a small value");
            unsafe { send_stamped(publisher, value, stamp * MILLISECOND) };
        }
        set_clock(close * MILLISECOND);
        statuses.push(run_status(waitset, 5_000_000));
    }

    let mut binding = HyphaCycleBindingView::default();
    assert_eq!(unsafe { hypha_cycle_binding(cycle, 0, &raw mut binding) }, HyphaStatus::Ok);
    let steps = context.recorder.steps();
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_input_source_destroy(source) }, HyphaStatus::Ok);
    unsafe {
        hypha_publisher_destroy(publisher);
        hypha_node_destroy(node);
    }
    (statuses, binding, steps)
}

/// The faithful replay of [`replay_a_rejection_at_200`]: the history keeps 180, which
/// the step at 300 takes.
fn assert_a_faithful_rejection_at_200(co_reader: bool) {
    let ok = HyphaStatus::Ok as i32;
    let (statuses, binding, steps) = replay_a_rejection_at_200(8, co_reader);
    assert_eq!(statuses, [ok, ok, ok], "{binding:?}");
    assert_eq!(binding.health.overrun_total, 0, "{binding:?}");
    assert_eq!(deadlines_and_values(&steps), [(100, Some(9)), (200, Some(19)), (300, Some(18))]);
}

#[test]
pub(crate) fn a_step_that_rejects_its_sample_holds_the_one_before_through_its_eviction() {
    assert_a_faithful_rejection_at_200(false);
    // A history of 3 evicts 180 after the step at 200. The rejection fell back to 180,
    // which the binding holds: the step at 300 has it as its value, as the faithful one.
    let (statuses, binding, steps) = replay_a_rejection_at_200(2, false);
    assert!(statuses.iter().all(|status| *status == HyphaStatus::Ok as i32), "{binding:?}");
    assert_eq!(binding.health.overrun_total, 0, "{binding:?}");
    assert_eq!(deadlines_and_values(&steps), [(100, Some(9)), (200, Some(19)), (300, None)]);
    assert_eq!((binding.held.has_held, binding.held.held_sequence), (1, 2), "180 is held");
}

#[test]
pub(crate) fn a_queued_co_reader_that_rejects_the_sample_loses_the_one_before_for_latest() {
    assert_a_faithful_rejection_at_200(true);
    // The queued binding's rejection leaves the latest binding holding 190, and a history
    // of 3 evicts 180 after the step at 200: the step at 300 has lost it.
    let (statuses, binding, steps) = replay_a_rejection_at_200(2, true);
    let ok = HyphaStatus::Ok as i32;
    assert_eq!(statuses, [ok, ok, HyphaStatus::SequenceGap as i32], "{binding:?}");
    assert_eq!(binding.health.capture.lost_total, 1, "180 is lost: {binding:?}");
    assert_eq!(binding.health.overrun_total, 1, "no transport gap: {binding:?}");
    assert_eq!(deadlines_and_values(&steps), [(100, Some(9)), (200, Some(19)), (300, None)]);
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
