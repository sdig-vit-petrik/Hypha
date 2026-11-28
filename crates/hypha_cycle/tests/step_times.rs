//! The step-time distribution: accuracy against exact quantiles, the window, the memory.

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "a test asserts on known values and fails loudly"
)]

use hypha_cycle::{StepSummary, StepTimes};

const SECOND: u64 = 1_000_000_000;
const WINDOW: u64 = StepTimes::WINDOW_NANOS;

/// The exact quantile of `sorted` (the smallest value with at least `q` of the values at or
/// below it), the definition `StepTimes` approximates.
fn exact(sorted: &[u64], q: f64) -> u64 {
    let rank = (q * sorted.len() as f64).ceil().max(1.0) as usize;
    sorted[rank - 1]
}

/// A reported quantile never reads below the exact one and at most 1/16 above it.
fn assert_close(reported: u64, exact: u64, what: &str) {
    assert!(reported >= exact, "{what}: {reported} below the exact {exact}");
    let allowed = exact + exact / 16 + 1;
    assert!(reported <= allowed, "{what}: {reported} above {allowed} (exact {exact})");
}

fn record_all(values: &[u64]) -> StepTimes {
    let mut times = StepTimes::default();
    for (index, value) in values.iter().enumerate() {
        times.record(*value, index as u64 * 1_000);
    }
    times
}

fn check(values: &[u64], name: &str) {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let summary = record_all(values).summary();
    assert_close(summary.p50_nanos, exact(&sorted, 0.5), &format!("{name} p50"));
    assert_close(summary.p90_nanos, exact(&sorted, 0.9), &format!("{name} p90"));
    assert_close(summary.p99_nanos, exact(&sorted, 0.99), &format!("{name} p99"));
}

/// A deterministic stream of 64-bit values (splitmix64).
fn stream(seed: u64) -> impl Iterator<Item = u64> {
    let mut state = seed;
    std::iter::repeat_with(move || {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    })
}

#[test]
fn a_uniform_distribution_is_reported_within_a_sixteenth() {
    let values: Vec<u64> = stream(1).take(100_000).map(|raw| 50_000 + raw % 950_000).collect();
    check(&values, "uniform 50 us to 1 ms");
}

#[test]
fn a_log_normal_like_distribution_is_reported_within_a_sixteenth() {
    // 2^(10 + 10 u) ns with u uniform: 1 us to 1 ms, a decade of steps in each octave.
    let values: Vec<u64> = stream(2)
        .take(100_000)
        .map(|raw| 2.0_f64.powf(10.0 + 10.0 * (raw >> 11) as f64 / (1_u64 << 53) as f64) as u64)
        .collect();
    check(&values, "log uniform 1 us to 1 ms");
}

#[test]
fn a_bimodal_distribution_keeps_its_slow_mode_in_the_p99() {
    // 98.5 % of the steps take 100 us, 1.5 % take 10 ms: the p99 is the slow mode.
    let values: Vec<u64> =
        (0..20_000).map(|i| if i % 200 < 3 { 10_000_000 } else { 100_000 }).collect();
    check(&values, "bimodal");
    let summary = record_all(&values).summary();
    assert!(summary.p99_nanos >= 10_000_000, "p99 {}", summary.p99_nanos);
    assert!(summary.p90_nanos < 110_000, "p90 {}", summary.p90_nanos);
}

#[test]
fn values_below_sixteen_nanoseconds_are_exact_and_zero_is_a_value() {
    let summary = record_all(&[0, 0, 0, 3, 3, 15]).summary();
    assert_eq!((summary.p50_nanos, summary.p90_nanos, summary.p99_nanos), (0, 15, 15));
}

#[test]
fn a_constant_distribution_reads_as_the_constant() {
    let summary = record_all(&[12_345_678; 500]).summary();
    for reported in [summary.p50_nanos, summary.p90_nanos, summary.p99_nanos] {
        assert_close(reported, 12_345_678, "constant");
    }
    assert_eq!(summary.recent_max_nanos, 12_345_678, "the maximum is exact");
}

#[test]
fn a_quantile_never_exceeds_the_largest_step() {
    let summary = record_all(&[1_000_003; 10]).summary();
    assert_eq!(summary.p99_nanos, 1_000_003, "clamped to the exact maximum, not its bucket");
}

#[test]
fn a_step_longer_than_the_range_is_counted_in_the_last_bucket_and_kept_as_the_maximum() {
    let hour = 3600 * SECOND;
    let mut times = StepTimes::default();
    times.record(1_000, 0);
    times.record(hour, 1);
    let summary = times.summary();
    assert_eq!(summary.recent_max_nanos, hour);
    assert_eq!(summary.p99_nanos, hour);
}

#[test]
fn no_step_gives_a_zero_summary() {
    assert_eq!(StepTimes::default().summary(), StepSummary::default());
}

#[test]
fn the_memory_is_fixed_whatever_is_recorded() {
    // No field is a `Vec`: the type has one size, and recording a million steps and several
    // windows cannot change it. Three histograms of 592 counters of 8 bytes.
    let size = std::mem::size_of::<StepTimes>();
    assert!(size <= 15_000, "{size} bytes");
    let mut times = StepTimes::default();
    for (index, raw) in stream(3).take(1_000_000).enumerate() {
        times.record(raw % 5_000_000, index as u64 * 100_000);
    }
    assert_eq!(std::mem::size_of_val(&times), size);
}

#[test]
fn a_start_up_spike_leaves_the_recent_maximum_after_two_windows_and_stays_in_the_total() {
    let mut times = StepTimes::default();
    // 50 ms at the start, then 1 ms steps every 10 ms for 30 s.
    times.record(50_000_000, 0);
    let mut now = 10_000_000;
    while now < 30 * SECOND {
        times.record(1_000_000, now);
        now += 10_000_000;
    }
    let summary = times.summary();
    assert_eq!(summary.recent_max_nanos, 1_000_000, "the spike is out of the window");
    assert_close(summary.recent_p99_nanos, 1_000_000, "recent p99");
    // One step in 3000 is not the 99th percentile of the total either.
    assert_close(summary.p99_nanos, 1_000_000, "total p99");
}

#[test]
fn the_window_holds_between_one_and_two_window_lengths_of_steps() {
    let mut times = StepTimes::default();
    // A slow step in the first window is still reported just after the window turned over,
    // and gone once a whole further window passed without one.
    times.record(9_000_000, 1);
    times.record(1_000_000, WINDOW + 1);
    assert_eq!(times.summary().recent_max_nanos, 9_000_000, "the previous window still counts");
    times.record(1_000_000, 2 * WINDOW + 2);
    assert_eq!(times.summary().recent_max_nanos, 1_000_000, "two windows later it is gone");
}

#[test]
fn a_long_silence_empties_the_window() {
    let mut times = StepTimes::default();
    times.record(9_000_000, 1);
    times.record(2_000_000, 10 * WINDOW);
    assert_eq!(times.summary().recent_max_nanos, 2_000_000);
}

#[test]
fn the_recent_quantiles_follow_a_change_the_total_follows_slowly() {
    let mut times = StepTimes::default();
    let mut now = 0;
    for _ in 0..10_000 {
        times.record(1_000_000, now);
        now += 1_000_000;
    }
    // The load changes: steps take 5 ms for the next two windows.
    for _ in 0..4_000 {
        times.record(5_000_000, now);
        now += 5_000_000;
    }
    let summary = times.summary();
    assert_close(summary.recent_p50_nanos, 5_000_000, "recent median follows");
    assert_close(summary.p50_nanos, 1_000_000, "total median does not");
}
