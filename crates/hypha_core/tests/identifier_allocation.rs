//! Identifier allocation wraps and never hands out zero or a live identifier.

use hypha_core::port::dispatch::next_free_identifier;

fn nothing_in_use(_candidate: u64) -> bool {
    false
}

#[test]
fn the_identifier_after_the_previous_one_is_handed_out() {
    assert_eq!(next_free_identifier(0, 0, nothing_in_use), Some(1));
    assert_eq!(next_free_identifier(41, 0, nothing_in_use), Some(42));
}

#[test]
fn the_identifier_after_the_maximum_value_wraps_past_zero() {
    assert_eq!(next_free_identifier(u64::MAX - 1, 0, nothing_in_use), Some(u64::MAX));
    assert_eq!(next_free_identifier(u64::MAX, 0, nothing_in_use), Some(1));
}

#[test]
fn an_identifier_still_in_use_is_skipped() {
    let live = [5_u64, 6];
    let in_use = |candidate: u64| live.contains(&candidate);
    assert_eq!(next_free_identifier(4, live.len(), in_use), Some(7));
}

#[test]
fn a_live_identifier_after_the_wrap_is_skipped() {
    let live = [u64::MAX, 1, 2];
    let in_use = |candidate: u64| live.contains(&candidate);
    assert_eq!(next_free_identifier(u64::MAX - 1, live.len(), in_use), Some(3));
}

#[test]
fn a_count_lower_than_the_identifiers_in_use_ends_the_search() {
    let every_identifier_in_use = |_candidate: u64| true;
    assert_eq!(next_free_identifier(0, 3, every_identifier_in_use), None);
}
