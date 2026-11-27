#![allow(clippy::arithmetic_side_effects, clippy::unwrap_used, clippy::expect_used)]

use super::*;

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|name| (*name).to_string()).collect()
}

#[test]
fn a_node_is_a_participant_from_its_first_report() {
    let start = Instant::now();
    let mut parts = Participants::new(start);
    assert_eq!(parts.len(), 0);
    assert!(parts.observe("a", 1, 10, start));
    assert!(!parts.observe("a", 2, 20, start));
    assert_eq!(parts.names(), names(&["a"]));
    assert_eq!(parts.get("a").map(|heard| heard.sent_nanos), Some(20));
    assert!(parts.get("b").is_none());
}

#[test]
fn an_ack_needs_a_send_time_at_or_after_the_floor() {
    let start = Instant::now();
    let required = names(&["a", "b"]);
    let mut parts = Participants::new(start);
    // A huge clock value sent before the floor is not an ack.
    parts.observe("a", u64::MAX, 99, start);
    parts.observe("b", 0, 100, start);
    assert_eq!(unacknowledged(&parts, &required, 100, 0), names(&["a"]));
    parts.observe("a", 0, 100, start);
    assert_eq!(unacknowledged(&parts, &required, 100, 0).len(), 0);
}

#[test]
fn a_partial_report_is_not_an_acknowledgement() {
    let start = Instant::now();
    let required = names(&["a"]);
    let mut parts = Participants::new(start);
    // Sent after the floor, but the clock is one short of the awaited value.
    parts.observe("a", 999, 100, start);
    assert_eq!(unacknowledged(&parts, &required, 100, 1_000), names(&["a"]));
    parts.observe("a", 1_000, 100, start);
    assert_eq!(unacknowledged(&parts, &required, 100, 1_000).len(), 0);
    parts.observe("a", 1_005, 100, start);
    assert_eq!(unacknowledged(&parts, &required, 100, 1_000).len(), 0);
}

#[test]
fn a_node_that_never_reported_is_unacknowledged() {
    let parts = Participants::new(Instant::now());
    assert_eq!(unacknowledged(&parts, &names(&["ghost"]), 0, 0), names(&["ghost"]));
    assert_eq!(unacknowledged(&parts, &[], 0, 0).len(), 0);
}

#[test]
fn a_node_that_answered_is_not_named_however_quiet_it_becomes() {
    let start = Instant::now();
    let mut parts = Participants::new(start);
    // "answered" acked the sample at once and then fell silent; "late" never answered.
    parts.observe("answered", 5, 100, start);
    parts.observe("late", 5, 1, start + Duration::from_secs(5));
    let at_six = start + Duration::from_secs(6);
    let timeout = Duration::from_secs(5);
    let required = names(&["answered", "late"]);

    // Only "late" is awaited, and it has been heard recently: nobody is silent.
    let pending = unacknowledged(&parts, &required, 100, 0);
    assert_eq!(pending, names(&["late"]));
    assert_eq!(first_silent(&parts, &pending, start, at_six, timeout), None);

    // Checked against all required nodes, the quiet acker would be named instead.
    let quiet = first_silent(&parts, &required, start, at_six, timeout);
    assert_eq!(quiet.map(|silent| silent.node), Some("answered".to_string()));
}

#[test]
fn a_silent_node_is_named_with_its_last_clock() {
    let start = Instant::now();
    let mut parts = Participants::new(start);
    parts.observe("quiet", 40, 1, start);
    let required = names(&["quiet", "ghost"]);
    let timeout = Duration::from_secs(5);

    let at_four = start + Duration::from_secs(4);
    assert_eq!(first_silent(&parts, &required, start, at_four, timeout), None);

    let at_six = start + Duration::from_secs(6);
    assert_eq!(
        first_silent(&parts, &required, start, at_six, timeout),
        Some(Silent { node: "quiet".to_string(), last_clock_nanos: Some(40) })
    );
}

#[test]
fn the_longest_quiet_node_is_named_first() {
    let start = Instant::now();
    let mut parts = Participants::new(start);
    // "a" was heard last, at +4 s; "b" at +1 s. Both are still awaited.
    parts.observe("a", 1, 1, start + Duration::from_secs(4));
    parts.observe("b", 1, 1, start + Duration::from_secs(1));
    let pending = names(&["a", "b"]);
    let timeout = Duration::from_secs(5);
    let at_seven = start + Duration::from_secs(7);
    assert_eq!(
        first_silent(&parts, &pending, start, at_seven, timeout).map(|silent| silent.node),
        Some("b".to_string())
    );
    // At +9 s "b" has been quiet longer and is named, though "a" comes first.
    let at_nine = start + Duration::from_secs(9);
    assert_eq!(
        first_silent(&parts, &pending, start, at_nine, timeout).map(|silent| silent.node),
        Some("b".to_string())
    );
}

#[test]
fn a_fresh_report_restarts_the_quiet_time() {
    let start = Instant::now();
    let mut parts = Participants::new(start);
    let later = start + Duration::from_secs(4);
    parts.observe("a", 1, 1, later);
    let at_six = start + Duration::from_secs(6);
    assert_eq!(first_silent(&parts, &names(&["a"]), start, at_six, Duration::from_secs(5)), None);
}

#[test]
fn the_start_barrier_settles_once_no_new_node_has_joined() {
    let start = Instant::now();
    let settle = Duration::from_secs(2);
    let parts = Participants::new(start);
    assert_eq!(
        start_state(&parts, &[], start, start + Duration::from_secs(3), settle, 0),
        StartState::NoneSeen
    );
    let mut parts = Participants::new(start);
    let first = start + Duration::from_millis(500);
    parts.observe("a", 0, 1, first);
    assert_eq!(
        start_state(&parts, &[], start, first + Duration::from_secs(1), settle, 1),
        StartState::Waiting
    );
    // A new node resets the settle time.
    let second = first + Duration::from_millis(1500);
    parts.observe("b", 0, 1, second);
    assert_eq!(
        start_state(&parts, &[], start, second + Duration::from_millis(1900), settle, 2),
        StartState::Waiting
    );
    assert_eq!(start_state(&parts, &[], start, second + settle, settle, 2), StartState::Ready);
}

#[test]
fn the_start_barrier_is_ready_once_every_connected_publisher_has_reported() {
    let start = Instant::now();
    let mut parts = Participants::new(start);
    parts.observe("a", 0, 1, start);
    parts.observe("b", 0, 1, start);
    let later = start + Duration::from_secs(3);
    assert_eq!(
        start_state(&parts, &names(&["a", "b"]), start, later, Duration::ZERO, 2),
        StartState::Ready
    );
    assert_eq!(
        start_state(&parts, &[], start, later, Duration::from_secs(2), 2),
        StartState::Ready
    );
}

#[test]
fn the_start_barrier_waits_while_a_connected_publisher_is_unheard_within_settle() {
    let start = Instant::now();
    let settle = Duration::from_secs(2);
    let mut parts = Participants::new(start);
    parts.observe("a", 0, 1, start);
    let named = names(&["a"]);
    let within = start + Duration::from_millis(1900);
    assert_eq!(start_state(&parts, &named, start, within, settle, 2), StartState::Waiting);
}

#[test]
fn the_start_barrier_gives_up_on_a_silent_publisher_after_settle() {
    let start = Instant::now();
    let settle = Duration::from_secs(2);
    let mut parts = Participants::new(start);
    parts.observe("a", 0, 1, start);
    let after = start + settle;
    assert_eq!(start_state(&parts, &names(&["a"]), start, after, settle, 2), StartState::Ready);
    assert_eq!(start_state(&parts, &[], start, after, settle, 2), StartState::Ready);
}

#[test]
fn the_start_barrier_with_named_nodes_waits_for_each_of_them() {
    let start = Instant::now();
    let named = names(&["a", "b"]);
    let mut parts = Participants::new(start);
    parts.observe("a", 0, 1, start);
    assert_eq!(
        start_state(&parts, &named, start, start + Duration::from_secs(10), Duration::ZERO, 0),
        StartState::Waiting
    );
    parts.observe("b", 0, 1, start);
    assert_eq!(
        start_state(&parts, &named, start, start + Duration::from_secs(10), Duration::ZERO, 0),
        StartState::Ready
    );
}

#[test]
fn a_named_node_that_never_appears_fails_the_barrier_after_thirty_seconds() {
    let start = Instant::now();
    let named = names(&["late"]);
    let parts = Participants::new(start);
    let state = start_state(&parts, &named, start, start + NAMED_START_TIMEOUT, Duration::ZERO, 0);
    assert_eq!(state, StartState::Missing(names(&["late"])));
    let before = start + Duration::from_secs(29);
    assert_eq!(start_state(&parts, &named, start, before, Duration::ZERO, 0), StartState::Waiting);
}

const MS: Duration = Duration::from_millis(1);

fn options(expected: &[&str]) -> DriverOptions {
    DriverOptions {
        start_nanos: 0,
        period_nanos: 10,
        timeout: Duration::from_secs(1),
        real_time_factor: None,
        expected: names(expected),
    }
}

fn driver(expected: &[&str], now: Instant) -> DriverSchedule {
    DriverSchedule::new(options(expected), now).unwrap()
}

#[test]
fn new_refuses_a_non_positive_period_timeout_or_factor() {
    let now = Instant::now();
    let bad = [
        DriverOptions { period_nanos: 0, ..options(&["a"]) },
        DriverOptions { timeout: Duration::ZERO, ..options(&["a"]) },
        DriverOptions { real_time_factor: Some(0.0), ..options(&["a"]) },
        DriverOptions { real_time_factor: Some(-1.0), ..options(&["a"]) },
        DriverOptions { real_time_factor: Some(f64::NAN), ..options(&["a"]) },
        DriverOptions { expected: names(&[""]), ..options(&["a"]) },
        options(&[]),
    ];
    for options in bad {
        assert!(DriverSchedule::new(options, now).is_err());
    }
}

#[test]
fn the_start_value_goes_out_with_no_step_and_the_first_step_is_one_period_later() {
    let now = Instant::now();
    let mut schedule = driver(&["a"], now);
    assert_eq!(schedule.next(now), DriverAction::Announce { value: 0 });
    schedule.sent(0, 5, now);
    assert_eq!(schedule.next(now), DriverAction::Wait);
    schedule.observe("a", 0, 5, now);
    assert_eq!(schedule.next(now), DriverAction::Step { deadline: 10 });
}

#[test]
fn no_step_runs_while_an_expected_participant_never_reports() {
    let t0 = Instant::now();
    let mut schedule = driver(&["a"], t0);
    assert_eq!(schedule.next(t0), DriverAction::Announce { value: 0 });
    schedule.sent(0, 1, t0);
    let mut now = t0;
    while now < t0 + NAMED_START_TIMEOUT {
        let action = schedule.next(now);
        assert!(
            matches!(action, DriverAction::Wait | DriverAction::Announce { value: 0 }),
            "{action:?} at {:?}",
            now - t0
        );
        if action != DriverAction::Wait {
            schedule.sent(0, 2, now);
        }
        now += 100 * MS;
    }
    assert_eq!(
        schedule.next(now),
        DriverAction::Fail { silent: names(&["a"]), last_clock_nanos: None }
    );
}

#[test]
fn the_driver_waits_for_every_required_participant() {
    let now = Instant::now();
    let mut schedule = driver(&["a", "b"], now);
    assert_eq!(schedule.next(now), DriverAction::Announce { value: 0 });
    schedule.sent(0, 5, now);
    schedule.observe("a", 0, 5, now);
    assert_eq!(schedule.next(now), DriverAction::Wait);
    schedule.observe("b", 0, 6, now);
    assert_eq!(schedule.next(now), DriverAction::Step { deadline: 10 });
}

#[test]
fn a_partial_report_holds_the_driver() {
    let now = Instant::now();
    let mut schedule = driver(&["a"], now);
    schedule.sent(0, 1, now);
    schedule.observe("a", 0, 1, now);
    assert_eq!(schedule.next(now), DriverAction::Step { deadline: 10 });
    schedule.sent(10, 5, now);
    schedule.observe("a", 0, 6, now);
    assert_eq!(schedule.next(now), DriverAction::Wait);
}

#[test]
fn an_ack_before_the_floor_does_not_count() {
    let now = Instant::now();
    let mut schedule = driver(&["a"], now);
    schedule.sent(0, 5, now);
    schedule.observe("a", 0, 4, now);
    assert_eq!(schedule.next(now), DriverAction::Wait);
}

#[test]
fn a_late_participant_becomes_required_from_its_first_report() {
    let now = Instant::now();
    let mut schedule = driver(&["a"], now);
    assert_eq!(schedule.next(now), DriverAction::Announce { value: 0 });
    schedule.sent(0, 1, now);
    schedule.observe("a", 0, 1, now);
    for deadline in [10, 20] {
        assert_eq!(schedule.next(now), DriverAction::Step { deadline });
        schedule.sent(deadline, 1, now);
        schedule.observe("a", u64::try_from(deadline).unwrap(), 1, now);
    }
    schedule.observe("c", 10, 1, now);
    assert_eq!(schedule.next(now), DriverAction::Wait);
    schedule.observe("c", 20, 1, now);
    assert_eq!(schedule.next(now), DriverAction::Step { deadline: 30 });
}

#[test]
fn an_unacknowledged_value_is_resent_every_interval() {
    let t0 = Instant::now();
    let mut schedule = driver(&["a"], t0);
    schedule.sent(0, 5, t0);
    assert_eq!(schedule.next(t0 + 199 * MS), DriverAction::Wait);
    assert_eq!(schedule.next(t0 + 200 * MS), DriverAction::Announce { value: 0 });
    schedule.sent(0, 9, t0 + 200 * MS);
    assert_eq!(schedule.next(t0 + 399 * MS), DriverAction::Wait);
    // A resend keeps the first floor: an answer to the first sample still counts.
    schedule.observe("a", 0, 5, t0 + 399 * MS);
    assert_eq!(schedule.next(t0 + 399 * MS), DriverAction::Step { deadline: 10 });
}

#[test]
fn a_silent_participant_fails_the_driver_naming_it() {
    let t0 = Instant::now();
    let mut schedule = driver(&["a"], t0);
    schedule.sent(0, 1, t0);
    schedule.observe("a", 0, 1, t0);
    assert_eq!(schedule.next(t0), DriverAction::Step { deadline: 10 });
    schedule.sent(10, 2, t0);
    assert!(!matches!(schedule.next(t0 + 999 * MS), DriverAction::Fail { .. }));
    assert_eq!(
        schedule.next(t0 + 1_000 * MS),
        DriverAction::Fail { silent: names(&["a"]), last_clock_nanos: Some(0) }
    );
}

#[test]
fn an_expected_participant_that_never_reports_fails_after_thirty_seconds() {
    let t0 = Instant::now();
    let mut schedule = driver(&["a"], t0);
    schedule.sent(0, 1, t0);
    let waited = t0 + Duration::from_secs(29);
    schedule.sent(0, 2, waited);
    assert_eq!(schedule.next(waited), DriverAction::Wait);
    assert_eq!(
        schedule.next(t0 + NAMED_START_TIMEOUT),
        DriverAction::Fail { silent: names(&["a"]), last_clock_nanos: None }
    );
}

#[test]
fn a_real_time_factor_paces_the_steps() {
    let t0 = Instant::now();
    let options =
        DriverOptions { period_nanos: 10_000_000, real_time_factor: Some(1.0), ..options(&["a"]) };
    let mut schedule = DriverSchedule::new(options, t0).unwrap();
    assert_eq!(schedule.next(t0), DriverAction::Announce { value: 0 });
    schedule.sent(0, 1, t0);
    schedule.observe("a", 0, 1, t0);
    assert_eq!(schedule.next(t0 + 9 * MS), DriverAction::Wait);
    assert_eq!(schedule.next(t0 + 10 * MS), DriverAction::Step { deadline: 10_000_000 });
}

#[test]
fn the_pace_counts_from_the_acknowledged_start_not_the_start_wait() {
    let t0 = Instant::now();
    let options =
        DriverOptions { period_nanos: 10_000_000, real_time_factor: Some(1.0), ..options(&["a"]) };
    let mut schedule = DriverSchedule::new(options, t0).unwrap();
    schedule.sent(0, 1, t0);
    let acked = t0 + Duration::from_secs(5);
    schedule.observe("a", 0, 1, acked);
    // The start wait is not charged to the pace: the next step is one period away.
    assert_eq!(schedule.next(acked), DriverAction::Wait);
    assert_eq!(schedule.next(acked + 10 * MS), DriverAction::Step { deadline: 10_000_000 });
}

#[test]
fn new_refuses_a_negative_start() {
    let options = DriverOptions { start_nanos: -1, ..options(&["a"]) };
    assert!(DriverSchedule::new(options, Instant::now()).is_err());
}

#[test]
fn a_deadline_past_the_clock_range_fails_the_driver() {
    let now = Instant::now();
    let options = DriverOptions { start_nanos: i64::MAX - 5, ..options(&["a"]) };
    let mut schedule = DriverSchedule::new(options, now).unwrap();
    schedule.sent(i64::MAX - 5, 1, now);
    schedule.observe("a", u64::try_from(i64::MAX - 5).unwrap(), 1, now);
    assert_eq!(schedule.next(now), DriverAction::Exhausted);
}

#[test]
fn the_clock_payload_is_seconds_then_nanoseconds_little_endian() {
    assert_eq!(clock_payload(0), Some([0; 8]));
    assert_eq!(clock_payload(1_500_000_000), Some([1, 0, 0, 0, 0x00, 0x65, 0xCD, 0x1D]));
    let last = i64::from(u32::MAX) * 1_000_000_000 + 999_999_999;
    assert_eq!(clock_payload(last), Some([0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xC9, 0x9A, 0x3B]));
    assert_eq!(clock_payload(last + 1), None, "the seconds no longer fit a u32");
    assert_eq!(clock_payload(-1), None, "a negative clock is not representable");
}
