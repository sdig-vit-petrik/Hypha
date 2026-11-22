//! A timer reset restarts the period from the time it is given.

use hypha_core::timer::Timer;
use hypha_time::{Duration, Time};

fn ms(millis: i64) -> Time {
    Time::from_nanos(millis.wrapping_mul(1_000_000))
}

#[test]
fn a_reset_restarts_the_period_from_the_reset_time() {
    let mut timer = Timer::new(Duration::from_millis(100), Time::ZERO);
    timer.reset(ms(250));
    assert_eq!(timer.next_due(), ms(350));
    assert_eq!(timer.poll(ms(349)), 0);
    assert_eq!(timer.poll(ms(350)), 1);
}

#[test]
fn a_reset_to_an_earlier_time_is_not_counted_as_missed_periods() {
    let mut timer = Timer::new(Duration::from_millis(100), ms(1_000));
    timer.reset(ms(500));
    assert_eq!(timer.poll(ms(600)), 1);
    assert_eq!(timer.missed_count(), 0);
}

#[test]
fn a_reset_of_a_timer_without_a_positive_period_never_fires() {
    let mut timer = Timer::new(Duration::ZERO, Time::ZERO);
    timer.reset(ms(10));
    assert_eq!(timer.poll(ms(1_000_000)), 0);
}

#[test]
fn the_latest_time_seen_follows_the_polls_and_a_reset() {
    let mut timer = Timer::new(Duration::from_millis(100), ms(1_000));
    assert_eq!(timer.latest_seen(), ms(1_000));
    let _fired = timer.poll(ms(1_250));
    assert_eq!(timer.latest_seen(), ms(1_250));
    timer.reset(ms(1_300));
    assert_eq!(timer.latest_seen(), ms(1_300));
}
