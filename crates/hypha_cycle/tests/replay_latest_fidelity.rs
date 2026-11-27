//! Replay on a Latest binding: an evicted sample that a step still to run could have
//! selected is lost, and one that a retained sample before the cut supersedes is not.

#![allow(
    clippy::arithmetic_side_effects,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::unwrap_used
)]

use hypha_cycle::{
    BindingCapture, BindingCounters, BindingOptions, CaptureClock, CaptureMode, CycleHistory,
    InputVerdict, QueueOverflowPolicy,
};

const MS: u64 = 1_000_000;

/// 50 Hz upstream into a 50 Hz Latest input, 100 ms chunks: each drain brings 5 samples
/// stamped D1..D5, then the consumer steps D1..D5.
fn run(history_samples: usize, transport_depth: usize) -> (Vec<Option<u64>>, BindingCounters) {
    let mut history = CycleHistory::new(history_samples).unwrap();
    let mut binding = BindingCapture::new(&history, CaptureMode::Latest, 1).unwrap();
    let opts = BindingOptions {
        mode: CaptureMode::Latest,
        silence_nanos: 0,
        max_samples_per_iteration: 1,
        overflow_policy: QueueOverflowPolicy::DropOldest,
    };
    let mut seen = Vec::new();
    for chunk in 0..4u64 {
        let deadlines: Vec<u64> = (1..=5).map(|i| (chunk * 100 + i * 20) * MS).collect();
        // The transport queue keeps the newest `transport_depth` samples of the burst.
        for &d in deadlines.iter().rev().take(transport_depth).rev() {
            history.push(&[0u8; 16], d, d);
        }
        for &d in &deadlines {
            binding.capture_on(&history, opts, CaptureClock::Replay { cut_nanos: d });
            seen.push(
                (binding.verdict == InputVerdict::Usable)
                    .then(|| binding.samples[0].framework_sent_nanos / MS),
            );
        }
    }
    (seen, binding.counters)
}

#[test]
fn default_limits_deliver_faithful_replay_without_overruns() {
    let (seen, counters) = run(256, 8);
    let expected: Vec<Option<u64>> =
        std::iter::once(None).chain((1..20).map(|i| Some(i * 20))).collect();
    assert_eq!(seen, expected);
    assert_eq!(counters.overrun_total, 0);
}

#[test]
fn history_of_one_detects_overrun_and_lost_samples() {
    // History 1 with transport 8 evicts 4 samples in the first burst.
    let (seen_burst, counters_burst) = run(1, 8);
    assert!(counters_burst.overrun_total > 0, "history eviction must count as overrun");
    assert_ne!(
        seen_burst,
        std::iter::once(None).chain((1..20).map(|i| Some(i * 20))).collect::<Vec<_>>()
    );

    // History 1 with transport 1 evicts across chunk boundaries.
    let (seen_single, counters_single) = run(1, 1);
    assert!(
        counters_single.overrun_total > 0,
        "history eviction across chunks must count as overrun"
    );
    assert!(seen_single.iter().all(Option::is_none));
}

const LATEST: BindingOptions = BindingOptions {
    mode: CaptureMode::Latest,
    silence_nanos: 0,
    max_samples_per_iteration: 1,
    overflow_policy: QueueOverflowPolicy::DropOldest,
};

/// A history of `depth` samples and a Latest binding on it, before any sample.
fn latest_on(depth: usize) -> (CycleHistory, BindingCapture) {
    let history = CycleHistory::new(depth).unwrap();
    let binding = BindingCapture::new(&history, CaptureMode::Latest, 1).unwrap();
    (history, binding)
}

/// Push one sample per stamp, in ms, on both clocks.
fn push_ms(history: &mut CycleHistory, stamps: impl IntoIterator<Item = u64>) {
    for stamp in stamps {
        history.push(&[0u8; 4], stamp * MS, stamp * MS);
    }
}

/// The stamps from `first` through `last` ms, every 10 ms.
fn every_10_ms(first: u64, last: u64) -> impl Iterator<Item = u64> {
    (first..=last).step_by(10)
}

/// Run the replay step at `cut_ms` and return the stamp of the sample it took, in ms.
fn step(binding: &mut BindingCapture, history: &CycleHistory, cut_ms: u64) -> Option<u64> {
    binding.capture_on(history, LATEST, CaptureClock::Replay { cut_nanos: cut_ms * MS });
    binding.samples.last().map(|sample| sample.framework_sent_nanos / MS)
}

#[test]
fn an_unread_eviction_that_a_retained_sample_supersedes_is_no_loss() {
    // Issue #222: a queue of 10, so a history of 11, and a 100 Hz topic.
    let (mut history, mut binding) = latest_on(11);
    push_ms(&mut history, every_10_ms(10, 100));
    assert_eq!(step(&mut binding, &history, 100), Some(90));
    // Evicts 100, which the step at 100 left unread; 190 supersedes it at 200.
    push_ms(&mut history, every_10_ms(110, 210));
    assert_eq!(step(&mut binding, &history, 200), Some(190));
    assert_eq!(binding.lost_since_capture, 0);
    assert_eq!(binding.counters.overrun_total, 0);
}

#[test]
fn many_evictions_in_one_interval_are_no_loss_when_the_step_selects_a_retained_sample() {
    let (mut history, mut binding) = latest_on(3);
    // The first step: 10..60 are evicted before any step read them; 90 supersedes them.
    push_ms(&mut history, every_10_ms(10, 90));
    assert_eq!(step(&mut binding, &history, 100), Some(90));
    assert_eq!(binding.lost_since_capture, 0);
    // 100..160 are evicted; 190 supersedes them.
    push_ms(&mut history, every_10_ms(100, 190));
    assert_eq!(step(&mut binding, &history, 200), Some(190));
    assert_eq!(binding.lost_since_capture, 0);
    // Nothing new: the step holds 190.
    assert_eq!(step(&mut binding, &history, 300), Some(190));
    assert_eq!(binding.counters.overrun_total, 0);
}

#[test]
fn evicting_the_sample_a_step_is_due_is_a_loss() {
    let (mut history, mut binding) = latest_on(3);
    push_ms(&mut history, every_10_ms(10, 90));
    assert_eq!(step(&mut binding, &history, 100), Some(90));
    // 190, the newest sample before 200, is evicted with every unread one before it, and
    // every retained sample is stamped at or after 200.
    push_ms(&mut history, every_10_ms(100, 220));
    assert_eq!(step(&mut binding, &history, 200), None);
    assert_eq!(binding.lost_since_capture, 10, "100..190 were never read");
    assert_eq!(step(&mut binding, &history, 300), Some(220));
    assert_eq!(binding.lost_since_capture, 0, "a loss is counted once");
    assert_eq!(binding.counters.overrun_total, 10);
}

#[test]
fn an_eviction_with_no_retained_sample_before_the_cut_is_a_loss_whatever_its_stamp() {
    let (mut history, mut binding) = latest_on(3);
    push_ms(&mut history, every_10_ms(10, 90));
    assert_eq!(step(&mut binding, &history, 100), Some(90));
    // 205 is evicted. The capture at 200 does not know the next deadline, so it cannot
    // tell whether 215 supersedes 205 there: it counts the loss.
    push_ms(&mut history, [205, 215, 225, 235]);
    assert_eq!(step(&mut binding, &history, 200), None);
    assert_eq!(binding.lost_since_capture, 1);
}

#[test]
fn a_retained_sample_a_reader_rejected_supersedes_no_eviction() {
    let (mut history, mut binding) = latest_on(3);
    push_ms(&mut history, every_10_ms(10, 90));
    assert_eq!(step(&mut binding, &history, 100), Some(90));
    // Retains 170, 180 and 190; 100..160 are evicted.
    push_ms(&mut history, every_10_ms(100, 190));
    let retained: Vec<u64> = history.samples().iter().map(|sample| sample.sequence).collect();
    // With 190 rejected the step selects 180, which still supersedes every eviction.
    let mut partly = binding.clone();
    let mut partly_rejected = history.clone();
    assert!(partly_rejected.reject(retained[2]));
    assert_eq!(step(&mut partly, &partly_rejected, 200), Some(180));
    assert_eq!(partly.lost_since_capture, 0);
    // With all three rejected the step would have taken 160, which is gone.
    for sequence in retained {
        assert!(history.reject(sequence));
    }
    assert_eq!(step(&mut binding, &history, 200), None);
    assert_eq!(binding.lost_since_capture, 7);
}

#[test]
fn a_latest_binding_on_host_time_counts_no_eviction() {
    let (mut history, mut binding) = latest_on(3);
    push_ms(&mut history, every_10_ms(10, 90));
    binding.capture(&history, LATEST, 1_000 * MS);
    push_ms(&mut history, every_10_ms(100, 220));
    binding.capture(&history, LATEST, 2_000 * MS);
    assert_eq!(binding.samples.last().map(|sample| sample.framework_sent_nanos / MS), Some(220));
    assert_eq!(binding.lost_since_capture, 0);
    assert_eq!(binding.counters.overrun_total, 0);
}
