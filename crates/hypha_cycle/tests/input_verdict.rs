//! One freshness verdict per binding, and the `ok` bit derived from it.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::arithmetic_side_effects)]

use hypha_cycle::{
    BindingCapture, BindingOptions, CaptureClock, CaptureMode, CycleHistory, InputVerdict,
    QueueOverflowPolicy, Supervision, SupervisionLimits,
};

const LIMIT: u64 = 100;

fn options(mode: CaptureMode, silence_nanos: u64) -> BindingOptions {
    BindingOptions {
        mode,
        silence_nanos,
        max_samples_per_iteration: 4,
        overflow_policy: QueueOverflowPolicy::DropOldest,
    }
}

/// One sample sent at `sent` on both clocks, captured at `now` on `clock_of(now)`.
fn judged_once(
    mode: CaptureMode,
    sent: u64,
    now: u64,
    clock_of: fn(u64) -> CaptureClock,
) -> BindingCapture {
    let mut history = CycleHistory::new(4).expect("history");
    history.push(&[1], sent, sent);
    let mut capture = BindingCapture::new(&history, mode, 4).expect("capture");
    capture.start_at(0);
    capture.capture_on(&history, options(mode, LIMIT), clock_of(now));
    capture
}

fn host(now: u64) -> CaptureClock {
    CaptureClock::Host { steady_nanos: now }
}

/// A replay cut admits samples stamped before it, so the cut is one past `now`.
fn replay(now: u64) -> CaptureClock {
    CaptureClock::Replay { cut_nanos: now }
}

#[test]
fn the_fault_bit_and_the_verdict_agree_for_every_age_around_the_limit() {
    let now = 1_000;
    let cases = [
        (now - (LIMIT - 1), InputVerdict::Usable),
        (now - LIMIT, InputVerdict::Usable),
        (now - (LIMIT + 1), InputVerdict::Stale),
    ];
    for mode in [CaptureMode::Latest, CaptureMode::Queued] {
        for clock_of in [host as fn(u64) -> CaptureClock, replay] {
            for (sent, expected) in cases {
                let capture = judged_once(mode, sent, now, clock_of);
                let context = format!("{mode:?} sent {sent} at {now} on {:?}", clock_of(now));
                assert_eq!(capture.verdict, expected, "{context}");
                assert_eq!(capture.stale(), expected == InputVerdict::Stale, "{context}");
                assert_eq!(capture.age_nanos, now - sent, "{context}");
            }
        }
    }
    // A sample stamped after the capture clock is not usable; only the host clock admits it.
    for mode in [CaptureMode::Latest, CaptureMode::Queued] {
        let capture = judged_once(mode, now + 1, now, host);
        assert_eq!(capture.verdict, InputVerdict::Stale, "{mode:?} future stamp");
        assert!(capture.stale(), "{mode:?} future stamp");
        assert!(capture.samples.is_empty(), "{mode:?} future stamp");
    }
}

#[test]
fn a_queued_input_without_a_new_sample_within_its_limit_is_usable() {
    let mut history = CycleHistory::new(4).expect("history");
    history.push(&[1], 10, 10);
    let mut capture = BindingCapture::new(&history, CaptureMode::Queued, 4).expect("capture");
    let queued = options(CaptureMode::Queued, LIMIT);
    capture.capture_on(&history, queued, host(20));
    assert_eq!(capture.samples.len(), 1);
    capture.capture_on(&history, queued, host(60));
    assert_eq!(capture.verdict, InputVerdict::Usable);
    assert!(capture.samples.is_empty());
    assert!(!capture.stale());
    capture.capture_on(&history, queued, host(10 + LIMIT + 1));
    assert_eq!(capture.verdict, InputVerdict::Stale);
}

#[test]
fn an_input_that_never_arrived_stays_never_arrived_past_its_limit() {
    let history = CycleHistory::new(4).expect("history");
    for mode in [CaptureMode::Latest, CaptureMode::Queued] {
        let mut capture = BindingCapture::new(&history, mode, 4).expect("capture");
        capture.capture_on(&history, options(mode, LIMIT), host(1_000));
        capture.capture_on(&history, options(mode, LIMIT), host(1_000 + LIMIT + 1));
        assert_eq!(capture.verdict, InputVerdict::NeverArrived, "{mode:?}");
        assert!(capture.stale(), "{mode:?}: the silence since the start is a fault");
        assert_eq!(capture.age_nanos, LIMIT + 1, "{mode:?}: the age counts from the start");
    }
    let mut absent = BindingCapture::absent(CaptureMode::Latest, 1).expect("capture");
    absent.capture_absent(options(CaptureMode::Latest, LIMIT), host(5));
    absent.capture_absent(options(CaptureMode::Latest, LIMIT), host(5 + LIMIT + 1));
    assert_eq!(absent.verdict, InputVerdict::NeverArrived);
}

#[test]
fn a_latest_input_reports_usable_only_with_a_held_sample() {
    // A replay step whose cut admits no retained sample, after one that held a value:
    // the held value is judged.
    let mut history = CycleHistory::new(1).expect("history");
    history.push(&[1], 10, 10);
    let mut capture = BindingCapture::new(&history, CaptureMode::Latest, 1).expect("capture");
    let latest = options(CaptureMode::Latest, LIMIT);
    capture.capture_on(&history, latest, replay(20));
    assert_eq!(capture.verdict, InputVerdict::Usable);
    assert!(capture.held().is_some());

    // The history keeps one sample: a newer one, stamped after the next cuts, evicts it.
    history.push(&[2], 500, 500);
    capture.capture_on(&history, latest, replay(40));
    assert!(capture.samples.is_empty());
    assert_eq!(capture.verdict, InputVerdict::Usable, "the held sample is 30 ns old");
    assert_eq!(capture.age_nanos, 30);
    assert!(capture.held().is_some());
    capture.capture_on(&history, latest, replay(10 + LIMIT + 1));
    assert_eq!(capture.verdict, InputVerdict::Stale, "the held sample is past the limit");

    // Every usable latest capture of the cases above holds a sample.
    for (sent, now) in [(900, 1_000), (999, 1_000), (100, 1_000)] {
        let capture = judged_once(CaptureMode::Latest, sent, now, host);
        let usable = capture.verdict == InputVerdict::Usable;
        assert!(!usable || capture.held().is_some(), "sent {sent} at {now}");
    }
}

#[test]
fn the_ok_bit_is_false_for_a_stagnant_counted_input_and_true_when_ignored_or_advisory() {
    let mut history = CycleHistory::new(4).expect("history");
    let mut capture = BindingCapture::new(&history, CaptureMode::Latest, 1).expect("capture");
    let latest = options(CaptureMode::Latest, 0);
    let limits = SupervisionLimits { silence_nanos: 0, stagnation_nanos: 50 };
    let mut supervision = Supervision::new(limits);
    for now in [0_u64, 100] {
        history.push(&[1], now, now);
        capture.capture_on(&history, latest, host(now));
        supervision.on_capture(i64::try_from(now).expect("time"), capture.received_total(), false);
        supervision.on_alive(7);
    }
    assert_eq!(capture.verdict, InputVerdict::Usable);
    assert_ne!(supervision.faults() & hypha_cycle::FAULT_STAGNANT, 0, "the alive value is stuck");

    assert!(!capture.ok(true, Some(&supervision)), "a stagnant counted input is not ok");
    assert!(capture.ok(false, Some(&supervision)), "an advisory or unwired input is ok");
    supervision.set_ignored(true);
    assert!(capture.ok(true, Some(&supervision)), "an ignored input is ok");

    let never =
        BindingCapture::new(&CycleHistory::new(1).expect("history"), CaptureMode::Latest, 1)
            .expect("capture");
    assert!(!never.ok(true, None), "a counted input that never arrived is not ok");
    assert!(capture.ok(true, None), "a usable unsupervised input is ok");
}

#[test]
fn a_rejected_newest_latest_sample_gives_way_to_the_one_held_before() {
    let mut history = CycleHistory::new(4).expect("history");
    let latest = options(CaptureMode::Latest, LIMIT);
    history.push(&[1], 10, 10);
    let mut capture = BindingCapture::new(&history, CaptureMode::Latest, 1).expect("capture");
    capture.capture_on(&history, latest, host(20));
    let first = capture.held().map(|sample| sample.sequence);

    let bad = history.push(&[0xff], 30, 30);
    capture.capture_on(&history, latest, host(40));
    assert_eq!(capture.held().map(|sample| sample.sequence), Some(bad));
    assert!(history.reject(bad));
    assert!(!history.reject(bad), "a sample is rejected once");
    capture.reject(&history, bad);
    assert_eq!(capture.held().map(|sample| sample.sequence), first);
    assert_eq!(capture.verdict, InputVerdict::Usable);
    assert_eq!(capture.age_nanos, 30, "judged by the sample sent at 10");
    assert_eq!(capture.received_total(), 1, "the rejected sample is not received");

    // The next capture does not select it again, and freshness keeps ageing.
    capture.capture_on(&history, latest, host(10 + LIMIT + 1));
    assert_eq!(capture.verdict, InputVerdict::Stale);
    assert_eq!(capture.held().map(|sample| sample.sequence), first);
}

#[test]
fn a_first_sample_that_is_rejected_leaves_the_input_never_arrived() {
    let mut history = CycleHistory::new(4).expect("history");
    let latest = options(CaptureMode::Latest, LIMIT);
    let mut capture = BindingCapture::new(&history, CaptureMode::Latest, 1).expect("capture");
    capture.capture_on(&history, latest, host(0));
    let bad = history.push(&[0xff], 5, 5);
    capture.capture_on(&history, latest, host(10));
    assert_eq!(capture.verdict, InputVerdict::Usable);
    assert!(history.reject(bad));
    capture.reject(&history, bad);
    assert_eq!(capture.verdict, InputVerdict::NeverArrived);
    assert!(capture.held().is_none());
    capture.capture_on(&history, latest, host(LIMIT + 1));
    assert_eq!(capture.verdict, InputVerdict::NeverArrived);
    assert!(capture.stale(), "the limit passed with no decodable sample");
}

#[test]
fn a_queued_input_of_rejected_samples_goes_stale_and_skips_them() {
    let mut history = CycleHistory::new(8).expect("history");
    let queued = options(CaptureMode::Queued, LIMIT);
    history.push(&[1], 0, 0);
    let mut capture = BindingCapture::new(&history, CaptureMode::Queued, 4).expect("capture");
    capture.capture_on(&history, queued, host(1));
    assert_eq!(capture.samples.len(), 1);
    for sent in [40_u64, 80, 120] {
        let bad = history.push(&[0xff], sent, sent);
        capture.capture_on(&history, queued, host(sent + 1));
        assert!(history.reject(bad));
        capture.reject(&history, bad);
    }
    assert_eq!(capture.verdict, InputVerdict::Stale, "judged by the sample sent at 0");
    let good = history.push(&[2], 130, 130);
    let bad = history.push(&[0xff], 131, 131);
    assert!(history.reject(bad));
    capture.capture_on(&history, queued, host(132));
    let delivered: Vec<u64> = capture.samples.iter().map(|sample| sample.sequence).collect();
    assert_eq!(delivered, [good], "a sample rejected before the capture is skipped");
    assert_eq!(capture.verdict, InputVerdict::Usable);
}

#[test]
fn a_rejected_newest_sample_gives_way_to_the_newest_decodable_one_of_the_same_drain() {
    // First selection: the drain holds a good and a bad sample; nothing was held before.
    let mut history = CycleHistory::new(4).expect("history");
    let latest = options(CaptureMode::Latest, LIMIT);
    let good = history.push(&[1], 10, 10);
    let bad = history.push(&[0xff], 12, 12);
    let mut capture = BindingCapture::new(&history, CaptureMode::Latest, 1).expect("capture");
    capture.capture_on(&history, latest, host(20));
    assert!(history.reject(bad));
    capture.reject(&history, bad);
    assert_eq!(capture.verdict, InputVerdict::Usable);
    assert_eq!(capture.held().map(|sample| sample.sequence), Some(good), "usable implies held");
    assert_eq!(capture.age_nanos, 10);

    // Steady state: a newer good sample of the same drain wins over the one held before.
    let newer = history.push(&[2], 30, 30);
    let worse = history.push(&[0xff], 31, 31);
    capture.capture_on(&history, latest, host(40));
    assert!(history.reject(worse));
    capture.reject(&history, worse);
    assert_eq!(capture.held().map(|sample| sample.sequence), Some(newer));
    assert_eq!(capture.age_nanos, 10);
}

#[test]
fn a_latest_replay_cursor_does_not_move_back_after_a_rejection() {
    let mut history = CycleHistory::new(2).expect("history");
    let latest = options(CaptureMode::Latest, 0);
    history.push(&[1], 10, 10);
    let bad = history.push(&[0xff], 20, 20);
    let mut capture = BindingCapture::new(&history, CaptureMode::Latest, 1).expect("capture");
    capture.capture_on(&history, latest, replay(25));
    assert!(history.reject(bad));
    capture.reject(&history, bad);
    // A later step admits nothing new; two pushes then evict the good and the bad sample.
    capture.capture_on(&history, latest, replay(26));
    history.push(&[3], 30, 30);
    history.push(&[4], 32, 32);
    capture.capture_on(&history, latest, replay(35));
    assert_eq!(capture.lost_since_capture, 0, "the rejected sample was passed, not lost");
}
