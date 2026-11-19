//! Behaviour of the clock registry and the real clocks.
//!
//! [`ClockFixture`] serialises every case and disables simulated time on the way out.

use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration as StdDuration;

use hypha_time::{Duration, Time};
use hypha_time::{
    advance_sim_time, disable_sim_time, enable_sim_time, is_sim_time, now, set_sim_time,
    steady_now, wall_now,
};

static SERIAL: Mutex<()> = Mutex::new(());

/// Holds the process-wide clock for one test and restores the monotonic clock afterwards.
struct ClockFixture {
    _serial: MutexGuard<'static, ()>,
}

impl ClockFixture {
    fn new() -> Self {
        let serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        disable_sim_time();
        Self { _serial: serial }
    }

    /// Enable simulated time reading `start`.
    ///
    /// Takes `&self` so a caller cannot enable simulated time without the fixture.
    #[allow(clippy::unused_self)]
    fn simulated_at(&self, start: Time) {
        assert!(enable_sim_time(start));
    }
}

impl Drop for ClockFixture {
    fn drop(&mut self) {
        disable_sim_time();
    }
}

#[test]
fn falls_back_to_monotonic_with_no_source() {
    let _fixture = ClockFixture::new();
    assert!(!is_sim_time());

    let first = now();
    thread::sleep(StdDuration::from_millis(5));
    assert!(now() > first);
}

#[test]
fn installed_source_drives_now() {
    let fixture = ClockFixture::new();
    fixture.simulated_at(Time::from_nanos(100_000_000_000));

    assert!(is_sim_time());
    assert!((now().seconds() - 100.0).abs() < 1e-12);

    assert!(advance_sim_time(Duration::from_secs(2)));
    assert!((now().seconds() - 102.0).abs() < 1e-12);
}

#[test]
fn simulated_time_does_not_advance_on_its_own() {
    let fixture = ClockFixture::new();
    fixture.simulated_at(Time::from_nanos(50_000_000_000));

    let before = now();
    thread::sleep(StdDuration::from_millis(20));
    assert_eq!(now(), before);
}

#[test]
fn simulated_time_starts_at_zero_meaning_unknown() {
    let fixture = ClockFixture::new();
    fixture.simulated_at(Time::ZERO);
    assert!(now().is_zero());
}

#[test]
fn simulated_clock_advance_saturates_at_the_latest_time() {
    let fixture = ClockFixture::new();
    fixture.simulated_at(Time::from_nanos(i64::MAX - 1));

    assert!(advance_sim_time(Duration::from_nanos(2)));

    assert_eq!(now(), Time::from_nanos(i64::MAX));
}

#[test]
fn simulated_clock_set_and_advance_are_refused_while_disabled() {
    let _fixture = ClockFixture::new();

    assert!(!set_sim_time(Time::from_nanos(1_000_000_000)));
    assert!(!advance_sim_time(Duration::from_secs(1)));
    assert!(!is_sim_time());
}

#[test]
fn a_negative_reading_or_span_is_refused_and_moves_nothing() {
    let fixture = ClockFixture::new();
    fixture.simulated_at(Time::from_nanos(5_000_000_000));

    assert!(!set_sim_time(Time::from_nanos(-1)));
    assert!(!advance_sim_time(Duration::from_nanos(-1)));
    assert_eq!(now(), Time::from_nanos(5_000_000_000));
}

#[test]
fn enabling_at_a_negative_reading_is_refused_and_keeps_the_clock() {
    let fixture = ClockFixture::new();
    fixture.simulated_at(Time::from_nanos(5_000_000_000));

    assert!(!enable_sim_time(Time::from_nanos(-1)));

    assert!(is_sim_time());
    assert_eq!(now(), Time::from_nanos(5_000_000_000));
}

#[test]
fn clearing_restores_the_fallback() {
    let fixture = ClockFixture::new();
    fixture.simulated_at(Time::ZERO);
    assert!(is_sim_time());

    disable_sim_time();
    assert!(!is_sim_time());
}

#[test]
fn wall_clock_ignores_the_installed_source() {
    let fixture = ClockFixture::new();
    fixture.simulated_at(Time::from_nanos(1_000_000_000));

    assert!(wall_now().seconds() > 1.0e9);
}

#[test]
fn steady_clock_ignores_the_installed_source() {
    let fixture = ClockFixture::new();
    fixture.simulated_at(Time::from_nanos(1_000_000_000_000));

    let first = steady_now();
    thread::sleep(StdDuration::from_millis(5));
    assert!(steady_now() > first);
    assert!(steady_now().seconds() > 0.0);
}
