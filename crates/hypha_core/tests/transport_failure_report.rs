//! A transport failure is reported when it starts, not on every turn it lasts.

use hypha_core::dispatch::transport_failure_starts;

#[test]
fn a_failure_that_starts_is_reported() {
    assert!(transport_failure_starts(false, true));
}

#[test]
fn a_failure_that_continues_is_not_reported_again() {
    assert!(!transport_failure_starts(true, true));
}

#[test]
fn a_healthy_turn_reports_nothing() {
    assert!(!transport_failure_starts(false, false));
    assert!(!transport_failure_starts(true, false));
}
