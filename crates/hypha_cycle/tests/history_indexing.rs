//! Indexed capture must preserve cut boundaries, wraparound and unordered arrivals.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::arithmetic_side_effects)]

use hypha_cycle::{
    BindingCapture, BindingOptions, CaptureClock, CaptureMode, CycleHistory, InputVerdict,
    QueueOverflowPolicy,
};

fn queued(policy: QueueOverflowPolicy) -> BindingOptions {
    BindingOptions {
        mode: CaptureMode::Queued,
        silence_nanos: 0,
        max_samples_per_iteration: 1,
        overflow_policy: policy,
    }
}

#[test]
fn replay_cuts_keep_equal_stamps_for_the_next_step_across_cursor_wrap() {
    for policy in [QueueOverflowPolicy::Carry, QueueOverflowPolicy::DropOldest] {
        let mut history = CycleHistory::with_initial_sequence(8, u64::MAX - 1).expect("history");
        for stamp in [10, 10, 20, 20, 30] {
            history.push(&[1], 0, stamp);
        }
        let mut capture = BindingCapture::new(&history, CaptureMode::Queued, 1).expect("capture");
        let clock = CaptureClock::Replay { cut_nanos: 20 };
        assert_eq!(capture.pending_samples_on(&history, clock), 2);
        capture.capture_on(&history, queued(policy), clock);
        let sequence =
            if policy == QueueOverflowPolicy::DropOldest { u64::MAX } else { u64::MAX - 1 };
        assert_eq!(capture.samples[0].sequence, sequence);
        assert_eq!(capture.received_total(), 2);
        let carrying = policy == QueueOverflowPolicy::Carry;
        assert_eq!(capture.backlog_samples, usize::from(carrying));
        assert_eq!(capture.work_limit_drops, u64::from(!carrying));
        if carrying {
            capture.capture_on(&history, queued(policy), clock);
            assert_eq!(capture.samples[0].sequence, u64::MAX);
        }
        capture.capture_on(&history, queued(policy), CaptureClock::Replay { cut_nanos: 30 });
        let sequence = u64::from(policy == QueueOverflowPolicy::DropOldest);
        assert_eq!(capture.samples[0].sequence, sequence);
        assert_eq!(capture.received_total(), 4);
        assert_eq!(capture.lost_since_capture, 0);
    }
}

#[test]
fn an_unordered_replay_queue_stops_at_the_first_unadmitted_arrival() {
    let mut history = CycleHistory::new(4).expect("history");
    for stamp in [10, 30, 20, 40] {
        history.push(&[1], 0, stamp);
    }
    let mut capture = BindingCapture::new(&history, CaptureMode::Queued, 1).expect("capture");
    let clock = CaptureClock::Replay { cut_nanos: 25 };
    assert_eq!(capture.pending_samples_on(&history, clock), 2);
    capture.capture_on(&history, queued(QueueOverflowPolicy::Carry), clock);
    assert_eq!(capture.samples[0].framework_sent_nanos, 10);
    assert_eq!(capture.backlog_samples, 1);
    assert_eq!(capture.received_total(), 2);
    capture.capture_on(&history, queued(QueueOverflowPolicy::Carry), clock);
    assert_eq!(capture.samples.len(), 0);
    assert_eq!(capture.verdict, InputVerdict::Usable);
    assert_eq!(capture.backlog_samples, 1);
    for expected in [30, 20] {
        capture.capture_on(
            &history,
            queued(QueueOverflowPolicy::Carry),
            CaptureClock::Replay { cut_nanos: 35 },
        );
        assert_eq!(capture.samples[0].framework_sent_nanos, expected);
    }
}

#[test]
fn a_history_without_a_byte_limit_keeps_every_sample_within_its_depth() {
    // Only the depth bounds a history, not payload size.
    let mut history = CycleHistory::new(3).expect("history");
    for (stamp, bytes) in [(10, 1), (20, 64 << 10), (30, 1 << 20)] {
        history.push(&vec![1; bytes], 0, stamp);
    }
    let sizes: Vec<usize> =
        history.samples().iter().map(|sample| sample.payload.to_vec().len()).collect();
    assert_eq!(sizes, [1, 64 << 10, 1 << 20]);
    let sequence = history.push(&vec![1; 2 << 20], 0, 40);
    assert_eq!(sequence, 3);
    let stamps: Vec<u64> =
        history.samples().iter().map(|sample| sample.framework_sent_nanos).collect();
    assert_eq!(stamps, [20, 30, 40]);
    assert_eq!(history.stats().accepted, 4);
}

#[test]
fn timestamp_search_matches_retained_data_after_inversions_and_evictions() {
    let mut history = CycleHistory::new(5).expect("history");
    // Searches must work while descending pairs are retained and after they are gone.
    for stamp in [50, 20, 40, 10, 30, 40, 50, 60, 70] {
        history.push(&[1], 0, stamp);
        for cut in [0, 10, 20, 30, 40, 50, 60, 70, u64::MAX] {
            let expected =
                history.samples().iter().filter(|sample| sample.framework_sent_nanos > cut).count();
            assert_eq!(history.stamped_beyond(cut, 0), expected);
            assert_eq!(history.stamped_beyond(cut, u64::MAX), 0);
            let mut latest =
                BindingCapture::new(&history, CaptureMode::Latest, 1).expect("capture");
            latest.capture_on(
                &history,
                BindingOptions { mode: CaptureMode::Latest, ..queued(QueueOverflowPolicy::Carry) },
                CaptureClock::Replay { cut_nanos: cut },
            );
            let expected = history
                .samples()
                .iter()
                .rev()
                .find(|sample| sample.framework_sent_nanos < cut)
                .map(|sample| sample.sequence);
            assert_eq!(latest.samples.first().map(|sample| sample.sequence), expected);
        }
    }
}
