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
    // tell whether 215 supersedes 205 there: it counts the loss. This errs towards a loss
    // only when more samples than the history holds are stamped from the cut through the
    // newest, here 205..235 in a history of 3.
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

/// The step at 100 takes 90; 180 and 190 arrive, and the step at 200 takes 190, which a
/// reader then rejects as undecodable: by the step itself through
/// [`BindingCapture::reject`] when `own`, else by another reader of the history. Returns
/// the history and the binding after the step at 200.
fn reject_the_sample_the_step_at_200_took(
    depth: usize,
    own: bool,
) -> (CycleHistory, BindingCapture) {
    let (mut history, mut binding) = latest_on(depth);
    push_ms(&mut history, [90]);
    assert_eq!(step(&mut binding, &history, 100), Some(90));
    push_ms(&mut history, [180, 190]);
    assert_eq!(step(&mut binding, &history, 200), Some(190));
    assert_eq!(binding.lost_since_capture, 0);
    let taken = binding.samples[0].sequence;
    assert!(history.reject(taken));
    if own {
        binding.reject(&history, taken);
    }
    (history, binding)
}

#[test]
fn an_eviction_counts_when_the_step_rejects_the_sample_that_superseded_it() {
    // Faithful: the history keeps 180, which the step at 300 takes.
    let (history, mut faithful) = reject_the_sample_the_step_at_200_took(8, true);
    assert_eq!(step(&mut faithful, &history, 300), Some(180));
    assert_eq!(faithful.counters.overrun_total, 0);
    // A history of one evicted 180 before the step at 200: it is lost.
    let (history, mut lossy) = reject_the_sample_the_step_at_200_took(1, true);
    assert_eq!(step(&mut lossy, &history, 300), None);
    assert_eq!(lossy.lost_since_capture, 1);
    assert_eq!(lossy.counters.overrun_total, 1);
}

#[test]
fn an_eviction_counts_when_another_reader_rejects_the_sample_that_superseded_it() {
    let (history, mut faithful) = reject_the_sample_the_step_at_200_took(8, false);
    assert_eq!(step(&mut faithful, &history, 300), Some(180));
    assert_eq!(faithful.counters.overrun_total, 0);
    let (history, mut lossy) = reject_the_sample_the_step_at_200_took(1, false);
    assert_eq!(step(&mut lossy, &history, 300), None);
    assert_eq!(lossy.lost_since_capture, 1);
}

#[test]
fn an_eviction_counts_when_the_rejected_superseding_sample_is_evicted_before_the_next_step() {
    let (mut history, mut faithful) = reject_the_sample_the_step_at_200_took(8, false);
    push_ms(&mut history, [305]);
    assert_eq!(step(&mut faithful, &history, 300), Some(180));
    // The drain evicts 190 and with it the history's record of its rejection.
    let (mut history, mut lossy) = reject_the_sample_the_step_at_200_took(1, false);
    push_ms(&mut history, [305]);
    assert!(!history.is_rejected(lossy.held().unwrap().sequence));
    assert_eq!(step(&mut lossy, &history, 300), None);
    // 180, and 190 too: the capture cannot tell an evicted rejected sample from another.
    assert_eq!(lossy.lost_since_capture, 2);
}

#[test]
fn a_passed_sample_evicted_after_its_step_is_lost_unless_the_binding_holds_it() {
    // At 200 the history holds 90, 180 and 190: the step takes 190 and passes 180, which
    // a drain then evicts with 190 retained.
    let (mut history, mut faithful) = reject_the_sample_the_step_at_200_took(8, false);
    push_ms(&mut history, [300, 305]);
    assert_eq!(step(&mut faithful, &history, 300), Some(180));
    // The step's own rejection falls back to 180 at once, and the binding holds it.
    let (mut history, mut own) = reject_the_sample_the_step_at_200_took(3, true);
    assert_eq!(own.held().map(|held| held.framework_sent_nanos / MS), Some(180));
    push_ms(&mut history, [300, 305]);
    assert_eq!(step(&mut own, &history, 300), None);
    assert_eq!(own.held().map(|held| held.framework_sent_nanos / MS), Some(180));
    assert_eq!(own.lost_since_capture, 0);
    // Another reader's rejection leaves the binding holding 190: 180 is lost.
    let (mut history, mut other) = reject_the_sample_the_step_at_200_took(3, false);
    push_ms(&mut history, [300, 305]);
    assert_eq!(step(&mut other, &history, 300), None);
    assert_eq!(other.lost_since_capture, 1);
}

#[test]
fn a_superseding_sample_evicted_without_a_rejection_keeps_its_evictions_uncounted() {
    let (mut history, mut binding) = latest_on(2);
    push_ms(&mut history, [90]);
    assert_eq!(step(&mut binding, &history, 100), Some(90));
    // 170 is evicted; 190 supersedes it and 180.
    push_ms(&mut history, [170, 180, 190]);
    assert_eq!(step(&mut binding, &history, 200), Some(190));
    // 180 and 190 are evicted, 190 unrejected: no step can take 170 or 180 again.
    push_ms(&mut history, [305, 315]);
    assert_eq!(step(&mut binding, &history, 300), None);
    assert_eq!(binding.lost_since_capture, 0);
    assert_eq!(binding.counters.overrun_total, 0);
}

#[test]
fn two_publishers_out_of_stamp_order_supersede_by_arrival() {
    // Selection is by arrival: the newest arrival before the cut supersedes every
    // eviction, whatever the stamps of the two publishers.
    for depth in [3, 16] {
        let (mut history, mut binding) = latest_on(depth);
        push_ms(&mut history, [90]);
        assert_eq!(step(&mut binding, &history, 100), Some(90));
        push_ms(&mut history, [150, 120, 190, 160]);
        assert_eq!(step(&mut binding, &history, 200), Some(160), "depth {depth}");
        assert_eq!(binding.lost_since_capture, 0, "depth {depth}");
    }
}

#[test]
fn several_deadlines_after_one_drain_count_an_eviction_once() {
    // One drain, then the steps at 160, 180, 190 and 200.
    let (mut history, mut binding) = latest_on(3);
    push_ms(&mut history, [90]);
    assert_eq!(step(&mut binding, &history, 100), Some(90));
    push_ms(&mut history, every_10_ms(110, 190));
    // 150, the newest sample before 160, is gone with 110..160.
    assert_eq!(step(&mut binding, &history, 160), None);
    assert_eq!(binding.lost_since_capture, 6);
    assert_eq!(step(&mut binding, &history, 180), Some(170));
    assert_eq!(step(&mut binding, &history, 190), Some(180));
    assert_eq!(step(&mut binding, &history, 200), Some(190));
    assert_eq!(binding.lost_since_capture, 0);
    assert_eq!(binding.counters.overrun_total, 6);

    // The same drain, first stepped at 180: 170 supersedes every eviction.
    let (mut history, mut binding) = latest_on(3);
    push_ms(&mut history, [90]);
    assert_eq!(step(&mut binding, &history, 100), Some(90));
    push_ms(&mut history, every_10_ms(110, 190));
    let taken: Vec<Option<u64>> =
        [180, 190, 200].iter().map(|cut| step(&mut binding, &history, *cut)).collect();
    assert_eq!(taken, [Some(170), Some(180), Some(190)]);
    assert_eq!(binding.counters.overrun_total, 0);
}

#[test]
fn a_latest_and_a_queued_binding_on_one_history_count_their_own_losses() {
    let queued_options =
        BindingOptions { mode: CaptureMode::Queued, max_samples_per_iteration: 4, ..LATEST };
    let (mut history, mut latest) = latest_on(3);
    let mut queued = BindingCapture::new(&history, CaptureMode::Queued, 4).unwrap();
    push_ms(&mut history, [80, 90]);
    let cut = CaptureClock::Replay { cut_nanos: 100 * MS };
    queued.capture_on(&history, queued_options, cut);
    assert_eq!(step(&mut latest, &history, 100), Some(90));
    // 110..160 are evicted: the queued binding read none of them, the latest one needs
    // only 190.
    push_ms(&mut history, every_10_ms(110, 190));
    let cut = CaptureClock::Replay { cut_nanos: 200 * MS };
    queued.capture_on(&history, queued_options, cut);
    assert_eq!(step(&mut latest, &history, 200), Some(190));
    assert_eq!(queued.lost_since_capture, 6);
    assert_eq!(latest.lost_since_capture, 0);
}
