use super::{Instant, Schedule};
use std::time::Duration;

fn schedule_at(anchor_log: u64, anchor_wall: Instant) -> Schedule {
    Schedule { anchor_log, anchor_wall }
}

#[test]
fn a_deadline_is_anchored_to_the_pass_start() {
    let start = Instant::now();
    let schedule = schedule_at(1_000_000_000, start);

    let first = schedule.deadline(1_500_000_000, 1.0);
    let later = schedule.deadline(3_000_000_000, 1.0);

    assert_eq!(first.map(|d| d.duration_since(start)), Some(Duration::from_millis(500)));
    assert_eq!(later.map(|d| d.duration_since(start)), Some(Duration::from_secs(2)));
}

#[test]
fn a_rate_too_slow_for_the_schedule_is_refused_not_a_panic() {
    let schedule = schedule_at(0, Instant::now());
    assert_eq!(schedule.deadline(u64::MAX, 1e-15), None);
}

#[test]
fn a_deadline_refuses_a_non_finite_rate() {
    let schedule = schedule_at(0, Instant::now());
    assert_eq!(schedule.deadline(1, f64::NAN), None);
}

#[test]
fn a_rate_change_keeps_the_schedule_where_it_was() {
    let start = Instant::now();
    let mut schedule = schedule_at(0, start);

    // 100 s into the pass at rate 1, the rate falls to 0.8.
    let now = start + Duration::from_secs(100);
    schedule.freeze(now, 1.0);
    assert_eq!(schedule.virtual_log_time(now, 0.8), Some(100_000_000_000));

    // The next message, 10 s of log time on, is due 12.5 s later.
    let deadline = schedule.deadline(110_000_000_000, 0.8);
    assert_eq!(deadline.map(|d| d.duration_since(now)), Some(Duration::from_millis(12_500)));
}

#[test]
fn a_pause_holds_the_schedule_at_its_position() {
    let start = Instant::now();
    let mut schedule = schedule_at(0, start);
    schedule.freeze(start + Duration::from_secs(10), 1.0);
    // Resuming ten seconds later continues from ten seconds of log time.
    let resumed = start + Duration::from_secs(20);
    let anchor = schedule.anchor_log;
    schedule.reanchor(anchor, resumed);
    assert_eq!(schedule.virtual_log_time(resumed, 1.0), Some(10_000_000_000));
}
