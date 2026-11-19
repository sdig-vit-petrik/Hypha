//! A timer follows a backward jump of framework time.

use hypha_core::timer::Timer;
use hypha_time::{Duration, Time};

const PERIOD: Duration = Duration::from_millis(100);

const fn at_millis(millis: i64) -> Time {
    Time::from_nanos(millis.saturating_mul(1_000_000))
}

#[test]
fn a_rewound_timer_fires_one_period_after_the_new_time() {
    let mut timer = Timer::new(PERIOD, at_millis(1_000_000));
    assert_eq!(timer.poll(at_millis(1_000_100)), 1);

    assert_eq!(timer.poll(at_millis(5_000)), 0, "the jump itself fires nothing");
    assert_eq!(timer.next_due(), at_millis(5_100));
    assert_eq!(timer.poll(at_millis(5_099)), 0);
    assert_eq!(timer.poll(at_millis(5_100)), 1);
    assert_eq!(timer.next_due(), at_millis(5_200));
}

#[test]
fn a_rewind_within_the_current_period_restarts_the_period() {
    let mut timer = Timer::new(PERIOD, at_millis(1_000));
    assert_eq!(timer.poll(at_millis(1_050)), 0);

    assert_eq!(timer.poll(at_millis(1_020)), 0);
    assert_eq!(timer.poll(at_millis(1_100)), 0, "the due time before the jump is gone");
    assert_eq!(timer.poll(at_millis(1_120)), 1);
}

#[test]
fn a_first_poll_before_the_creation_time_is_a_rewind() {
    let mut timer = Timer::new(PERIOD, at_millis(10_000));

    assert_eq!(timer.poll(at_millis(2_000)), 0);
    assert_eq!(timer.next_due(), at_millis(2_100));
}

#[test]
fn a_rewind_counts_no_missed_period() {
    let mut timer = Timer::new(PERIOD, at_millis(1_000));
    assert_eq!(timer.poll(at_millis(1_100)), 1);

    assert_eq!(timer.poll(at_millis(0)), 0);
    assert_eq!(timer.poll(at_millis(100)), 1);
    assert_eq!(timer.missed_count(), 0);
}

#[test]
fn a_repeated_replay_fires_the_same_way_every_time() {
    let mut timer = Timer::new(PERIOD, at_millis(0));
    for _replay in 0..3 {
        let mut fired = 0_u32;
        for step in 1..=10_i64 {
            fired = fired.saturating_add(timer.poll(at_millis(step.saturating_mul(100))));
        }
        assert_eq!(fired, 10, "each replay of one second fires ten periods");
        assert_eq!(timer.poll(at_millis(0)), 0, "the replay starts again");
    }
    assert_eq!(timer.missed_count(), 0);
}

#[test]
fn a_jump_the_timer_never_saw_is_not_a_rewind() {
    let mut timer = Timer::new(PERIOD, at_millis(1_000));
    assert_eq!(timer.poll(at_millis(1_050)), 0);

    // Time went back and forward again between two polls.
    assert_eq!(timer.poll(at_millis(1_070)), 0);
    assert_eq!(timer.poll(at_millis(1_100)), 1);
}

#[test]
fn rewind_restarts_an_exhausted_timer() {
    let period = Duration::from_nanos(10);
    let maximum = Time::from_nanos(i64::MAX);
    let mut timer = Timer::new(period, Time::from_nanos(i64::MAX - 5));

    assert_eq!(timer.poll(maximum), 0);
    let rewound = Time::from_nanos(i64::MAX - 20);
    assert_eq!(timer.poll(rewound), 0);
    assert_eq!(timer.next_due(), Time::from_nanos(i64::MAX - 10));
    assert_eq!(timer.poll(Time::from_nanos(i64::MAX - 10)), 1);
}
