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
    let one_publisher: Vec<(Vec<(usize, i64)>, i64)> = chunks
        .iter()
        .map(|(stamps, close)| (stamps.iter().map(|&stamp| (0, stamp)).collect(), *close))
        .collect();
    replay_publishers(Input { bind, queue, period_ms: 100, publishers: 1 }, start, &one_publisher)
}

/// The input of a replay test: how its cycle binds it, the depth of its queue, the
/// period of its cycle in ms, and how many publishers send on its topic.
#[derive(Clone, Copy)]
struct Input {
    bind: HyphaCycleBindingOptions,
    queue: usize,
    period_ms: i64,
    publishers: usize,
}

/// [`replay_an_input`] for `input`, whose chunks name the publisher of each sample:
/// `(publisher, stamp in ms)`.
fn replay_publishers(
    input: Input,
    start: i64,
    chunks: &[(Vec<(usize, i64)>, i64)],
) -> (Vec<i32>, HyphaCycleBindingHealth, Vec<Step>) {
    let Input { bind, queue, period_ms, publishers } = input;
    let _clock = SimulatedClock::start(start * MILLISECOND);
    let node = create_node(&unique("replay_sizing_latest"));
    let waitset = unsafe { create_waitset(node) };
    let topic = format!("/{}", unique("replay_sizing_latest"));
    let (publisher, subscriber) = unsafe { create_pubsub(node, &topic, queue) };
    let mut senders = vec![publisher];
    senders.extend((1..publishers).map(|_| unsafe {
        open_publisher(node, &topic, "std_msgs/Counter", "d9d213e8c146065ec958333c617eb1a6", "")
    }));
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
                u64::try_from(period_ms * MILLISECOND).expect("period"),
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
    for (samples, close) in chunks {
        for &(sender, stamp) in samples {
            let value = u8::try_from(stamp / 10).expect("a small value");
            let sender = *senders.get(sender).expect("a publisher of the input");
            unsafe { send_stamped(sender, value, stamp * MILLISECOND) };
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
        for sender in senders {
            hypha_publisher_destroy(sender);
        }
        hypha_node_destroy(node);
    }
    (statuses, binding.health, steps)
}

/// A queued (`Arrivals`) binding that takes up to `per_step` samples a step.
const fn queued(per_step: usize) -> HyphaCycleBindingOptions {
    HyphaCycleBindingOptions {
        mode: HyphaCycleInputMode::Queued as u32,
        max_samples_per_iteration: per_step,
        optional: 0,
    }
}

/// Each step's deadline and how many samples it took, in ms.
fn deadlines_and_counts(steps: &[Step]) -> Vec<(i64, usize)> {
    steps.iter().map(|step| (step.scheduled / MILLISECOND, step.sample_count)).collect()
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

/// The chunks `hypha play --chunk 100` sends of a 100 Hz topic from 10 ms to a subscriber
/// queue of 10 (4.3): the first chunk, 10..110, is one sample past the queue, so the
/// player closes it at 109 and sends 110 alone. Every later chunk is 10 samples.
fn split_chunks_off_the_grid() -> Vec<(Vec<i64>, i64)> {
    vec![
        (every_10_ms(10, 100), 109),
        (vec![110], 110),
        (every_10_ms(120, 210), 210),
        (every_10_ms(220, 310), 310),
        (every_10_ms(320, 410), 410),
    ]
}

#[test]
pub(crate) fn a_queued_queue_of_one_chunk_keeps_up_off_the_deadline_grid() {
    // A queue of 100 ms x 100 Hz and a `perStep` of 100 ms x 100 Hz. The `/clock` at 110
    // runs no step and still drains 110, so the queue never holds more than one chunk.
    let (statuses, health, steps) =
        replay_an_input(queued(10), 10, 10, &split_chunks_off_the_grid());
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
pub(crate) fn a_latest_queue_of_one_chunk_keeps_up_off_the_deadline_grid() {
    // Issue #222: a queue of 100 ms x 100 Hz. The `/clock` at 110 runs no step and still
    // drains 110, so the queue of 10 holds 120..210 alone. The history evicts samples that
    // the step's selection supersedes, which is no loss.
    let (statuses, health, steps) = replay_an_input(LATEST, 10, 10, &split_chunks_off_the_grid());
    assert!(statuses.iter().all(|status| *status == HyphaStatus::Ok as i32), "{health:?}");
    assert_eq!(health.capture.lost_total, 0, "no eviction is a loss: {health:?}");
    assert_eq!(health.overrun_total, 0, "no transport gap: {health:?}");
    assert_eq!(
        deadlines_and_values(&steps),
        [(100, Some(9)), (200, Some(19)), (300, Some(29)), (400, Some(39))]
    );
}

/// What a queued input with a queue of 10 and a `perStep` of `per_step` on a 200 ms cycle
/// sees of the player's chunks of a 100 Hz topic from `start` ms through the first step
/// (#247): the status of each run, the binding's health and the steps.
fn replay_a_first_period_of_two_chunks(
    per_step: usize,
    start: i64,
) -> (Vec<i32>, HyphaCycleBindingHealth, Vec<Step>) {
    // The player splits every chunk that holds one sample past the queue (4.3).
    let chunks: Vec<(Vec<i64>, i64)> = if start == 0 {
        vec![(every_10_ms(0, 90), 99), (every_10_ms(100, 190), 199), (vec![200], 200)]
    } else {
        vec![(every_10_ms(10, 100), 109), (vec![110], 110), (every_10_ms(120, 210), 210)]
    };
    let one: Vec<(Vec<(usize, i64)>, i64)> = chunks
        .into_iter()
        .map(|(stamps, close)| (stamps.into_iter().map(|stamp| (0, stamp)).collect(), close))
        .collect();
    let input = Input { bind: queued(per_step), queue: 10, period_ms: 200, publishers: 1 };
    replay_publishers(input, start, &one)
}

#[test]
pub(crate) fn the_first_frames_of_a_period_longer_than_a_queue_are_all_delivered() {
    // #247: the first step, at 200, comes two chunks after the first frame. Each `/clock`
    // before it drains its chunk, so the queue of 10 never holds more than one chunk and
    // no first frame is pushed out unseen. The history of 10 + 20 holds 10..210.
    let (statuses, health, steps) = replay_a_first_period_of_two_chunks(20, 10);
    assert!(statuses.iter().all(|status| *status == HyphaStatus::Ok as i32), "{statuses:?}");
    assert_eq!(health.overrun_total, 0, "{health:?}");
    assert_eq!(deadlines_and_counts(&steps), [(200, 19)], "10..190, every one");
    // On the deadline grid: 0..190 at the step at 200, every one.
    let (statuses, health, steps) = replay_a_first_period_of_two_chunks(20, 0);
    assert!(statuses.iter().all(|status| *status == HyphaStatus::Ok as i32), "{statuses:?}");
    assert_eq!(health.overrun_total, 0, "{health:?}");
    assert_eq!(deadlines_and_counts(&steps), [(200, 20)], "0..190, every one");
}

#[test]
pub(crate) fn a_history_too_short_for_the_step_fails_it_and_counts_the_loss() {
    // The same replay with a `perStep` of 10: a history of 10 + 10 cannot hold the 21
    // samples 10..210 that arrive before the step at 200, and it evicts 10 unread. The
    // drain moved the loss from the transport into the history, where it is counted.
    let (statuses, health, steps) = replay_a_first_period_of_two_chunks(10, 10);
    let ok = HyphaStatus::Ok as i32;
    assert_eq!(statuses, [ok, ok, HyphaStatus::SequenceGap as i32], "{health:?}");
    assert_eq!(health.capture.lost_total, 1, "10 is lost: {health:?}");
    assert_eq!(health.overrun_total, 1, "no transport gap: {health:?}");
    // The `perStep` of 10 takes the newest 10 of 20..190 and drops 20..90 on purpose.
    assert_eq!(health.work_limit_drops_total, 8, "{health:?}");
    assert_eq!(deadlines_and_counts(&steps), [(200, 10)]);
}

#[test]
pub(crate) fn several_clock_values_between_two_deadlines_each_drain_their_chunk() {
    // Chunks of 50 ms of a 100 Hz topic into a queue of 6, and a 200 ms cycle: four
    // `/clock` values come before the first step, and three of them run no step.
    let chunks = [
        (every_10_ms(10, 60), 60),
        (every_10_ms(70, 110), 110),
        (every_10_ms(120, 160), 160),
        (every_10_ms(170, 210), 210),
    ];
    let one: Vec<(Vec<(usize, i64)>, i64)> = chunks
        .into_iter()
        .map(|(stamps, close)| (stamps.into_iter().map(|stamp| (0, stamp)).collect(), close))
        .collect();
    let input = Input { bind: queued(20), queue: 6, period_ms: 200, publishers: 1 };
    let (statuses, health, steps) = replay_publishers(input, 10, &one);
    assert!(statuses.iter().all(|status| *status == HyphaStatus::Ok as i32), "{statuses:?}");
    assert_eq!(health.overrun_total, 0, "{health:?}");
    assert_eq!(deadlines_and_counts(&steps), [(200, 19)], "10..190, every one");
    // A `Latest` input of the same queue takes the newest sample before the deadline.
    let input = Input { bind: LATEST, ..input };
    let (statuses, health, steps) = replay_publishers(input, 10, &one);
    assert!(statuses.iter().all(|status| *status == HyphaStatus::Ok as i32), "{health:?}");
    assert_eq!(health.overrun_total, 0, "{health:?}");
    assert_eq!(deadlines_and_values(&steps), [(200, Some(19))]);
}

#[test]
pub(crate) fn two_publishers_on_one_input_are_drained_before_each_acknowledgement() {
    // Each chunk brings 5 samples of the player-like publisher 0 and 3 of publisher 1,
    // into a queue of 5 per publisher. A drain that stopped after one queue's worth would
    // leave 2 samples per chunk behind, and a queue would overflow within three chunks.
    // Every sample is stamped before the step at 200, which the last `/clock` runs.
    let chunk = |first: i64| -> Vec<(usize, i64)> {
        let player = (first..first + 50).step_by(10).filter(|stamp| *stamp < 200);
        let other = (first + 5..first + 35).step_by(10);
        player.map(|stamp| (0, stamp)).chain(other.map(|stamp| (1, stamp))).collect()
    };
    let mut chunks: Vec<(Vec<(usize, i64)>, i64)> =
        [10, 60, 110, 160].into_iter().map(|first| (chunk(first), first + 49)).collect();
    chunks.push((Vec::new(), 210));
    let input = Input { bind: queued(64), queue: 5, period_ms: 200, publishers: 2 };
    let (statuses, health, steps) = replay_publishers(input, 10, &chunks);
    assert!(statuses.iter().all(|status| *status == HyphaStatus::Ok as i32), "{statuses:?}");
    assert_eq!(health.overrun_total, 0, "{health:?}");
    // 10..190 of publisher 0 and the 12 samples of publisher 1.
    assert_eq!(deadlines_and_counts(&steps), [(200, 31)]);
}

#[test]
pub(crate) fn a_latest_queue_that_fits_each_drain_keeps_up_off_the_deadline_grid() {
    // A queue of 11, the first chunk's samples, so no chunk is shortened. The history of
    // 12 cannot hold 100..220: the drain at 220 ms evicts 100, which the step at 100 left
    // unread and 190 supersedes at 200, and the one at 320 ms evicts 200, which 290
    // supersedes at 300. No step could take either, so nothing is lost.
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
pub(crate) fn a_first_chunk_past_the_queue_before_any_turn_is_not_counted() {
    // #247, its limit: all 11 frames reach a queue of 10 before any turn, so no drain can
    // run between them. The first frame is pushed out before the publisher's first
    // accepted frame, which no sequence check sees (communication-integrity.md §5).
    for (start, due, taken) in [(10, 9, 8), (0, 10, 9)] {
        let chunk = [(every_10_ms(start, start + 100), start + 100)];
        let (statuses, health, steps) = replay_an_input(queued(10), 10, start, &chunk);
        assert_eq!(statuses, [HyphaStatus::Ok as i32], "start {start}");
        assert_eq!(health.overrun_total, 0, "start {start}: {health:?}");
        assert_eq!(deadlines_and_counts(&steps), [(100, taken)], "{due} due from {start}");
    }
}

/// A source on `topic` of `node`'s `waitset`, with a queue of `queue`, bound to `cycle`
/// as `bind`; the publisher that feeds it and the source.
unsafe fn bound_input(
    node: *mut HyphaNode,
    waitset: *mut HyphaWaitSet,
    cycle: *mut HyphaCycle,
    queue: usize,
    bind: HyphaCycleBindingOptions,
) -> (*mut HyphaPublisher, *mut HyphaInputSource, u32) {
    let topic = format!("/{}", unique("replay_both"));
    let (publisher, subscriber) = unsafe { create_pubsub(node, &topic, queue) };
    let mut source = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_input_source_create(waitset, subscriber, &raw mut source) },
        HyphaStatus::Ok
    );
    let mut index = 0_u32;
    assert_eq!(
        unsafe { hypha_cycle_bind_input(cycle, source, &raw const bind, &raw mut index) },
        HyphaStatus::Ok
    );
    (publisher, source, index)
}

/// An uncommitted cycle of `waitset` with `period_nanos` (zero: triggered) that records
/// each step in `recorder`.
unsafe fn recorded_cycle(
    waitset: *mut HyphaWaitSet,
    period_nanos: u64,
    recorder: &Recorder,
) -> *mut HyphaCycle {
    let name = c_text(&unique("replay_both_cycle"));
    let mut cycle = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                waitset,
                name.as_ptr(),
                name.as_bytes().len(),
                period_nanos,
                Some(record_step),
                ptr::from_ref(recorder).cast_mut().cast(),
                None,
                &raw mut cycle,
            )
        },
        HyphaStatus::Ok
    );
    cycle
}

#[test]
pub(crate) fn a_rate_cycle_beside_a_triggered_cycle_drains_on_every_turn() {
    // One `WaitSet`: a triggered cycle, which drains its own inputs on every turn, and a
    // 200 ms rate cycle whose queued input has a queue of one chunk. The rate cycle's
    // input is drained on the turns that run no step of it too.
    const SILENCE_MS: i64 = 1_000;
    let _clock = SimulatedClock::start(10 * MILLISECOND);
    let node = crate::common::supervision::node_with_triggers("replay_both", &[SILENCE_MS]);
    let waitset = unsafe { create_waitset(node) };
    let triggered_steps = Box::new(Recorder::new(Duration::ZERO));
    let triggered = unsafe { recorded_cycle(waitset, 0, &triggered_steps) };
    let (trigger, trigger_source, port) =
        unsafe { bound_input(node, waitset, triggered, 4, queued(4)) };
    let bound =
        unsafe { crate::common::supervision::bind_trigger(triggered, node, port, SILENCE_MS) };
    assert_eq!(bound.status, HyphaStatus::Ok, "{}", bound.detail);
    assert_eq!(unsafe { hypha_cycle_set_trigger(triggered, port) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_cycle_commit(triggered) }, HyphaStatus::Ok);
    let rate_steps = Box::new(Recorder::new(Duration::ZERO));
    let rate = unsafe { recorded_cycle(waitset, 200_000_000, &rate_steps) };
    let (data, data_source, _) = unsafe { bound_input(node, waitset, rate, 10, queued(20)) };
    assert_eq!(unsafe { hypha_cycle_commit(rate) }, HyphaStatus::Ok);

    let chunks =
        [(every_10_ms(10, 100), 50, 109), (vec![110], 110, 110), (every_10_ms(120, 210), 150, 210)];
    let mut statuses = Vec::new();
    for (stamps, fires, close) in chunks {
        for stamp in stamps {
            let value = u8::try_from(stamp / 10).expect("a small value");
            unsafe { send_stamped(data, value, stamp * MILLISECOND) };
        }
        unsafe { send_stamped(trigger, 1, fires * MILLISECOND) };
        set_clock(close * MILLISECOND);
        statuses.push(run_status(waitset, 5_000_000));
    }
    assert!(statuses.iter().all(|status| *status == HyphaStatus::Ok as i32), "{statuses:?}");
    let fired: Vec<i64> =
        triggered_steps.steps().iter().map(|step| step.scheduled / MILLISECOND).collect();
    assert_eq!(fired, [50, 110, 150], "one step per trigger sample");
    assert_eq!(deadlines_and_counts(&rate_steps.steps()), [(200, 19)], "10..190, every one");

    unsafe {
        assert_eq!(hypha_cycle_destroy(rate), HyphaStatus::Ok);
        assert_eq!(hypha_cycle_destroy(triggered), HyphaStatus::Ok);
        assert_eq!(hypha_input_source_destroy(data_source), HyphaStatus::Ok);
        assert_eq!(hypha_input_source_destroy(trigger_source), HyphaStatus::Ok);
        hypha_publisher_destroy(data);
        hypha_publisher_destroy(trigger);
        hypha_node_destroy(node);
    }
}

#[test]
pub(crate) fn on_host_time_a_turn_without_a_step_drains_no_input() {
    let _serial = SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let node = create_node(&unique("host_no_drain"));
    let waitset = unsafe { create_waitset(node) };
    let steps = Box::new(Recorder::new(Duration::ZERO));
    // A 1 s cycle, so its first step comes long after the turns below.
    let cycle = unsafe { recorded_cycle(waitset, 1_000_000_000, &steps) };
    let (publisher, source, _) = unsafe { bound_input(node, waitset, cycle, 4, queued(16)) };
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);
    run_for(waitset, 2_000_000);
    for value in 1..=4 {
        unsafe { send_stamped(publisher, value, 0) };
    }
    // A turn that runs no step leaves the four in the queue.
    run_for(waitset, 2_000_000);
    for value in 5..=8 {
        unsafe { send_stamped(publisher, value, 0) };
    }
    let started = std::time::Instant::now();
    while steps.steps().is_empty() {
        assert!(started.elapsed() < Duration::from_secs(5), "the first step never ran");
        run_for(waitset, 50_000_000);
    }
    // The queue of 4 kept 5..8. Host time detects no loss before a publisher's first
    // accepted frame (communication-integrity.md §5).
    let first = steps.steps().first().copied().expect("a step");
    assert_eq!((first.sample_count, first.newest), (4, Some(8)));
    unsafe {
        assert_eq!(hypha_cycle_destroy(cycle), HyphaStatus::Ok);
        assert_eq!(hypha_input_source_destroy(source), HyphaStatus::Ok);
        hypha_publisher_destroy(publisher);
        hypha_node_destroy(node);
    }
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
