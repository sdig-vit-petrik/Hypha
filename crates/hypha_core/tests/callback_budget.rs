//! The budget a callback is judged against, and the line that names a slow one (#128).

use std::time::Duration;

use hypha_core::dispatch::{
    CallbackBudget, DEFAULT_CALLBACK_BUDGET_NANOS, callback_budget, is_slow_callback,
    slow_callback_line,
};

const MS: u64 = 1_000_000;

#[test]
fn a_subscription_without_a_setting_has_the_default_budget() {
    let budget = callback_budget(None, None);
    assert_eq!(budget, CallbackBudget { nanos: DEFAULT_CALLBACK_BUDGET_NANOS, warns: true });
    assert_eq!(DEFAULT_CALLBACK_BUDGET_NANOS, 10 * MS);
}

#[test]
fn a_timer_or_cycle_without_a_setting_is_judged_against_its_own_period() {
    assert_eq!(callback_budget(None, Some(100 * MS)).nanos, 100 * MS);
    assert_eq!(callback_budget(None, Some(MS)).nanos, MS);
}

#[test]
fn a_ten_hertz_callback_that_takes_45_ms_is_not_slow() {
    let budget = callback_budget(None, Some(100 * MS));
    assert!(!is_slow_callback(Duration::from_millis(45), budget.nanos));
    assert!(is_slow_callback(Duration::from_millis(101), budget.nanos));
}

#[test]
fn a_configured_budget_replaces_the_default_for_every_kind() {
    assert_eq!(callback_budget(Some(50), None), CallbackBudget { nanos: 50 * MS, warns: true });
    assert_eq!(callback_budget(Some(50), Some(MS)).nanos, 50 * MS);
}

#[test]
fn a_configured_budget_moves_the_line_a_callback_crosses() {
    let configured = callback_budget(Some(50), None).nanos;
    assert!(!is_slow_callback(Duration::from_millis(46), configured));
    assert!(is_slow_callback(Duration::from_millis(60), configured));
    assert!(is_slow_callback(Duration::from_millis(46), callback_budget(None, None).nanos));
}

#[test]
fn zero_writes_no_line_and_the_counter_judges_against_the_default() {
    let silenced = callback_budget(Some(0), None);
    assert_eq!(silenced, CallbackBudget { nanos: DEFAULT_CALLBACK_BUDGET_NANOS, warns: false });
    assert_eq!(callback_budget(Some(0), Some(100 * MS)).nanos, 100 * MS);
}

#[test]
fn the_line_names_what_was_slow_how_slow_and_the_budget() {
    let line = slow_callback_line("topic '/sensing/lidar/points'", 46_000_000, 10 * MS, 9);
    assert_eq!(
        line,
        "hypha: callback of topic '/sensing/lidar/points' took 46 ms, budget 10 ms, \
         9 warnings suppressed"
    );
}

#[test]
fn the_line_keeps_the_fraction_that_makes_a_callback_slow() {
    let line = slow_callback_line("timer 2.5 ms", 2_600_000, 2_500_000, 0);
    assert_eq!(
        line,
        "hypha: callback of timer 2.5 ms took 2.6 ms, budget 2.5 ms, 0 warnings suppressed"
    );
}
