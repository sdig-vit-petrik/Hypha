//! Replay on a Latest binding: history eviction must be detected as an overrun.

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
