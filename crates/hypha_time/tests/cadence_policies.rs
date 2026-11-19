//! The period kernel that serves `hypha_core::Timer`: expiry counting, rewinds,
//! the epoch grid and the representable range.

use hypha_time::{Cadence, Duration, Time};

fn ms(millis: i64) -> Time {
    Time::from_nanos(millis.saturating_mul(1_000_000))
}

const PERIOD: Duration = Duration::from_millis(100);

#[test]
fn every_expired_period_is_counted() {
    let mut polled = Cadence::new(PERIOD, Time::ZERO);

    let step = polled.advance(ms(200));
    assert_eq!(step.expired, 2, "the deadlines at 100 ms and 200 ms both expired");
    assert_eq!(polled.next_deadline(), ms(300));
}

#[test]
fn a_rewind_is_measured_from_the_latest_time_seen() {
    let mut polled = Cadence::new(PERIOD, Time::ZERO);
    assert!(!polled.advance(ms(50)).rewound);

    assert!(!polled.advance(ms(60)).rewound, "60 ms is not earlier than the latest 50 ms");
    let step = polled.advance(ms(40));
    assert!(step.rewound);
    assert_eq!(step.expired, 0, "a rewind expires nothing");
    assert_eq!(polled.next_deadline(), ms(140));
}

#[test]
fn a_grid_cadence_keeps_its_phase_after_a_rewind() {
    // A grid cadence stays on the multiples of the period, so a rewind to 130 ms
    // leaves the next deadline at 200 ms, not at 230 ms.
    let mut polled = Cadence::on_grid(PERIOD, Time::ZERO);
    assert_eq!(polled.advance(ms(250)).expired, 2, "the deadlines at 100 and 200 ms expire");
    assert_eq!(polled.next_deadline(), ms(300));

    let step = polled.advance(ms(130));
    assert!(step.rewound);
    assert_eq!(polled.next_deadline(), ms(200), "the first grid point after the rewind");
}

#[test]
fn an_unrepresentable_deadline_exhausts_the_period() {
    let start = Time::from_nanos(i64::MAX - 5);
    let period = Duration::from_nanos(10);
    let maximum = Time::from_nanos(i64::MAX);

    let mut polled = Cadence::new(period, start);
    assert_eq!(
        polled.advance(maximum).expired,
        0,
        "the first deadline is past the representable range, so it never fires"
    );
    assert_eq!(polled.advance(maximum).expired, 0, "an exhausted period never fires");
}

#[test]
fn a_period_exhausts_after_it_fires_at_the_maximum() {
    let period = Duration::from_nanos(10);
    let maximum = Time::from_nanos(i64::MAX);
    let mut polled = Cadence::new(period, Time::from_nanos(i64::MAX - 10));
    assert_eq!(polled.next_deadline(), maximum);

    assert_eq!(polled.advance(maximum).expired, 1, "the deadline at i64::MAX expires");
    assert_eq!(
        polled.advance(maximum).expired,
        0,
        "the next deadline is past the range, so the period is exhausted"
    );
}

#[test]
fn a_rewind_revives_an_exhausted_period() {
    let period = Duration::from_nanos(10);
    let maximum = Time::from_nanos(i64::MAX);
    let mut polled = Cadence::new(period, Time::from_nanos(i64::MAX - 10));
    polled.advance(maximum);
    assert_eq!(polled.advance(maximum).expired, 0);

    let rewound = Time::from_nanos(i64::MAX - 20);
    let step = polled.advance(rewound);
    assert!(step.rewound, "time moved back from the maximum");
    assert_eq!(polled.next_deadline(), Time::from_nanos(i64::MAX - 10));
    assert_eq!(polled.advance(Time::from_nanos(i64::MAX - 10)).expired, 1);
}

#[test]
fn a_zero_period_never_expires_and_moves_no_anchor() {
    let mut polled = Cadence::new(Duration::ZERO, Time::ZERO);
    assert_eq!(polled.advance(ms(1_000)).expired, 0);
    assert_eq!(polled.anchor(), Time::ZERO, "time is not observed without a positive period");

    let step = polled.advance(ms(-1));
    assert!(step.rewound, "a rewind is still reported against the recorded time");
    assert_eq!(polled.anchor(), Time::ZERO, "a rewind does not move the recorded time");
}

#[test]
fn a_host_cadence_fed_only_steady_readings_fires_each_period() {
    // A host schedule reads only the monotonic clock: one period per reading, no rewind.
    let mut host = Cadence::new(PERIOD, ms(5_000));
    for index in 1..=10_i64 {
        let step = host.advance(ms(5_000 + 100 * index));
        assert!(!step.rewound, "a steady reading never moves back");
        assert_eq!(step.expired, 1, "period {index} expires once");
    }
    assert_eq!(host.next_deadline(), ms(6_100));
}
