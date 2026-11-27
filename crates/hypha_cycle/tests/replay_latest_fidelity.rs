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

/// 90 is taken at 100. Then 250, from a publisher that stamps ahead of the clock, and
/// 150 arrive: the step at 200 takes 150, the newest arrival before the cut, and passes
/// 250. A reader rejects 150: the step itself through [`BindingCapture::reject`] when
/// `own`, else another reader of the history.
fn passed_ahead_then_rejected(own: bool) -> (CycleHistory, BindingCapture) {
    let (mut history, mut binding) = latest_on(4);
    push_ms(&mut history, [90]);
    assert_eq!(step(&mut binding, &history, 100), Some(90));
    push_ms(&mut history, [250, 150]);
    assert_eq!(step(&mut binding, &history, 200), Some(150));
    let taken = binding.samples[0].sequence;
    assert!(history.reject(taken));
    if own {
        binding.reject(&history, taken);
    }
    (history, binding)
}

#[test]
fn a_sample_passed_for_one_a_reader_rejected_counts_when_it_is_evicted() {
    for own in [false, true] {
        let (mut history, mut binding) = passed_ahead_then_rejected(own);
        // 90, 250 and 150 are evicted, and nothing retained is before 300.
        push_ms(&mut history, [305, 315, 325, 335]);
        assert_eq!(step(&mut binding, &history, 300), None, "own {own}");
        // 250, which was due at 300, and the rejected 150: an evicted sample that may
        // have been rejected counts.
        assert_eq!(binding.lost_since_capture, 2, "own {own}");
    }
}

#[test]
fn a_step_that_falls_back_to_an_older_sample_keeps_the_passed_one_counted() {
    for own in [false, true] {
        let (mut history, mut binding) = passed_ahead_then_rejected(own);
        // At 210, 150 is rejected and 250 not admitted yet: the step takes 90 again.
        assert_eq!(step(&mut binding, &history, 210), Some(90), "own {own}");
        push_ms(&mut history, [305, 315, 325, 335]);
        // 250 was the value due at 300 and is gone with 90 and 150.
        assert_eq!(step(&mut binding, &history, 300), None, "own {own}");
        assert_eq!(binding.lost_since_capture, 2, "own {own}: 250 and the rejected 150");
    }
}

#[test]
fn a_fallback_before_the_first_passed_sample_keeps_the_passed_one_counted() {
    let (mut history, mut binding) = latest_on(5);
    push_ms(&mut history, [90]);
    assert_eq!(step(&mut binding, &history, 100), Some(90));
    // 105 and 110 in order, then 250 stamped ahead of the clock, then 150.
    push_ms(&mut history, [105, 110, 250, 150]);
    assert_eq!(step(&mut binding, &history, 200), Some(150));
    let taken = binding.samples[0].sequence;
    assert!(history.reject(taken));
    // 150 is rejected and 250 not admitted at 210: the step falls back to 110.
    assert_eq!(step(&mut binding, &history, 210), Some(110));
    // 90, 105, 110 and 250 are evicted; the rejected 150 stays.
    push_ms(&mut history, [305, 315, 325, 335]);
    let retained: Vec<u64> =
        history.samples().iter().map(|sample| sample.framework_sent_nanos / MS).collect();
    assert_eq!(retained, [150, 305, 315, 325, 335]);
    // 250 was the value due at 300.
    assert_eq!(step(&mut binding, &history, 300), None);
    assert_eq!(binding.lost_since_capture, 1, "250 is lost; the binding holds 110 for good");
}

/// A history of `depth` that assigns `first_sequence` first, and a Latest binding on it.
fn latest_from(depth: usize, first_sequence: u64) -> (CycleHistory, BindingCapture) {
    let history = CycleHistory::with_initial_sequence(depth, first_sequence).unwrap();
    let binding = BindingCapture::new(&history, CaptureMode::Latest, 1).unwrap();
    (history, binding)
}

/// A capture on the host clock, which admits every retained sample.
fn host_capture(binding: &mut BindingCapture, history: &CycleHistory) -> Option<u64> {
    binding.capture_on(history, LATEST, CaptureClock::Host { steady_nanos: 1_000_000 * MS });
    binding.samples.last().map(|sample| sample.framework_sent_nanos / MS)
}

#[test]
fn a_host_capture_between_replay_steps_keeps_a_due_loss_counted() {
    // A host capture holds 500, which no cut has admitted: that is no value for good.
    let (mut history, mut binding) = latest_from(3, 0);
    push_ms(&mut history, [10]);
    assert_eq!(step(&mut binding, &history, 100), Some(10));
    push_ms(&mut history, [50, 500]);
    assert_eq!(host_capture(&mut binding, &history), Some(500));
    push_ms(&mut history, [510, 520, 530]);
    assert_eq!(step(&mut binding, &history, 200), None);
    // 50 was due at 200; 500, not due yet, counts with it (a capture that selects
    // nothing counts evicted samples stamped after its cut).
    assert_eq!(binding.lost_since_capture, 2);
    // Without the host capture the count is the same.
    let (mut history, mut binding) = latest_from(3, 0);
    push_ms(&mut history, [10]);
    assert_eq!(step(&mut binding, &history, 100), Some(10));
    push_ms(&mut history, [50, 500, 510, 520, 530]);
    assert_eq!(step(&mut binding, &history, 200), None);
    assert_eq!(binding.lost_since_capture, 2);
}

#[test]
fn the_rules_hold_across_a_sequence_wrap() {
    for first in [u64::MAX - 1, u64::MAX, 0] {
        // Evicting the due sample is a loss.
        let (mut history, mut binding) = latest_from(3, first);
        push_ms(&mut history, (10..=90).step_by(10));
        assert_eq!(step(&mut binding, &history, 100), Some(90));
        push_ms(&mut history, (100..=220).step_by(10));
        assert_eq!(step(&mut binding, &history, 200), None, "first {first}");
        assert_eq!(binding.lost_since_capture, 10, "first {first}");
        // A retained superseding sample excuses its evictions.
        let (mut history, mut binding) = latest_from(3, first);
        push_ms(&mut history, [90]);
        assert_eq!(step(&mut binding, &history, 100), Some(90));
        push_ms(&mut history, (100..=190).step_by(10));
        assert_eq!(step(&mut binding, &history, 200), Some(190), "first {first}");
        assert_eq!(binding.lost_since_capture, 0, "first {first}");
        // A passed sample counts once the selection that passed it is rejected.
        let (mut history, mut binding) = latest_from(4, first);
        push_ms(&mut history, [90]);
        assert_eq!(step(&mut binding, &history, 100), Some(90));
        push_ms(&mut history, [250, 150]);
        assert_eq!(step(&mut binding, &history, 200), Some(150));
        assert!(history.reject(binding.samples[0].sequence));
        push_ms(&mut history, [305, 315, 325, 335]);
        assert_eq!(step(&mut binding, &history, 300), None, "first {first}");
        assert_eq!(binding.lost_since_capture, 2, "first {first}");
    }
}

#[test]
fn the_first_capture_counts_nothing_from_before_the_binding() {
    let mut history = CycleHistory::with_initial_sequence(3, u64::MAX - 5).unwrap();
    push_ms(&mut history, (10..=90).step_by(10));
    let mut binding = BindingCapture::new(&history, CaptureMode::Latest, 1).unwrap();
    assert_eq!(step(&mut binding, &history, 100), Some(90));
    assert_eq!(binding.lost_since_capture, 0);
    // On an empty history at sequence 0, the cursor's predecessor is u64::MAX.
    let (mut history, mut binding) = latest_from(2, 0);
    push_ms(&mut history, [210, 220, 230]);
    assert_eq!(step(&mut binding, &history, 200), None);
    assert_eq!(binding.lost_since_capture, 1, "210, stamped after the cut, counts");
}

#[test]
fn selecting_the_same_sample_again_keeps_the_passed_run() {
    let (mut history, mut binding) = latest_from(3, 0);
    push_ms(&mut history, [90]);
    assert_eq!(step(&mut binding, &history, 100), Some(90));
    push_ms(&mut history, [110, 120, 300]);
    for cut in [200, 210, 220] {
        assert_eq!(step(&mut binding, &history, cut), Some(120), "cut {cut}");
    }
    // 120 is evicted unrejected: 110 is superseded for good.
    push_ms(&mut history, [310, 320]);
    assert_eq!(step(&mut binding, &history, 290), None);
    assert_eq!(binding.counters.overrun_total, 0);
}

#[test]
fn two_publishers_out_of_stamp_order_count_the_due_sample_in_every_rejection_order() {
    // Arrival 90, 250 (stamped ahead), 150, 160: the step at 200 takes 160 and passes
    // 250 and 150. Then 160 and 150 are rejected, by the step or by another reader, with
    // a step at 210 between or not, and everything is evicted. Due at 300: 250.
    for first_own in [false, true] {
        for second_own in [false, true] {
            for step_between in [false, true] {
                let case = format!("{first_own} {second_own} {step_between}");
                let (mut history, mut binding) = latest_from(4, 0);
                push_ms(&mut history, [90]);
                assert_eq!(step(&mut binding, &history, 100), Some(90));
                push_ms(&mut history, [250, 150, 160]);
                assert_eq!(step(&mut binding, &history, 200), Some(160));
                let s160 = binding.samples[0].sequence;
                let s150 = s160 - 1;
                assert!(history.reject(s160));
                if first_own {
                    binding.reject(&history, s160);
                }
                if step_between {
                    assert_eq!(step(&mut binding, &history, 210), Some(150), "{case}");
                }
                assert!(history.reject(s150));
                // A step's own rejection reaches only a sample it holds.
                let holds_150 = binding.held().is_some_and(|held| held.sequence == s150);
                if second_own && holds_150 {
                    binding.reject(&history, s150);
                }
                push_ms(&mut history, [305, 315, 325, 335]);
                assert_eq!(step(&mut binding, &history, 300), None, "{case}");
                assert!(binding.lost_since_capture >= 1, "{case}: 250 is gone");
            }
        }
    }
}

#[test]
fn a_queued_co_reader_rejecting_the_latest_selection_keeps_the_loss_counted() {
    let queued_options =
        BindingOptions { mode: CaptureMode::Queued, max_samples_per_iteration: 8, ..LATEST };
    let (mut history, mut latest) = latest_from(3, 0);
    let mut queued = BindingCapture::new(&history, CaptureMode::Queued, 8).unwrap();
    push_ms(&mut history, [90]);
    assert_eq!(step(&mut latest, &history, 100), Some(90));
    push_ms(&mut history, [180, 190]);
    queued.capture_on(&history, queued_options, CaptureClock::Replay { cut_nanos: 200 * MS });
    assert_eq!(step(&mut latest, &history, 200), Some(190));
    assert!(history.reject(latest.samples[0].sequence));
    push_ms(&mut history, [305, 315]);
    assert_eq!(step(&mut latest, &history, 300), None);
    // 180 was due at 300 and is gone; the rejected 190 is still retained.
    assert_eq!(latest.lost_since_capture, 1);
}

#[test]
fn a_sample_stamped_after_the_cut_that_is_evicted_counts_at_a_step_that_selects_nothing() {
    // An over-count, towards failure: 205 is evicted before any cut admits it, the step
    // at 200 selects nothing and counts it, though 235 supersedes it at 300.
    let (mut history, mut binding) = latest_from(3, 0);
    push_ms(&mut history, [90]);
    assert_eq!(step(&mut binding, &history, 100), Some(90));
    push_ms(&mut history, [205, 215, 225, 235]);
    assert_eq!(step(&mut binding, &history, 200), None);
    assert_eq!(binding.lost_since_capture, 1);
    assert_eq!(step(&mut binding, &history, 300), Some(235));
}
