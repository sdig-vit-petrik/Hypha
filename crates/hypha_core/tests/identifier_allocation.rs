//! Identifier allocation wraps, and never hands out zero, a live identifier or one of
//! the dispatcher range.

use hypha_core::port::dispatch::{
    FIRST_DISPATCHER_IDENTIFIER, LAST_NATIVE_IDENTIFIER, next_free_identifier,
};

fn nothing_in_use(_candidate: u64) -> bool {
    false
}

#[test]
fn the_identifier_after_the_previous_one_is_handed_out() {
    assert_eq!(next_free_identifier(0, 0, nothing_in_use), Some(1));
    assert_eq!(next_free_identifier(41, 0, nothing_in_use), Some(42));
}

#[test]
fn the_identifier_after_the_last_native_one_wraps_past_zero() {
    let before_last = LAST_NATIVE_IDENTIFIER - 1;
    assert_eq!(next_free_identifier(before_last, 0, nothing_in_use), Some(LAST_NATIVE_IDENTIFIER));
    assert_eq!(next_free_identifier(LAST_NATIVE_IDENTIFIER, 0, nothing_in_use), Some(1));
}

#[test]
fn no_identifier_of_the_dispatcher_range_is_handed_out() {
    assert_eq!(FIRST_DISPATCHER_IDENTIFIER, LAST_NATIVE_IDENTIFIER + 1);
    for previous in [FIRST_DISPATCHER_IDENTIFIER - 1, FIRST_DISPATCHER_IDENTIFIER, u64::MAX] {
        let next = next_free_identifier(previous, 0, nothing_in_use);
        assert_eq!(next, Some(1), "after {previous:#x}");
    }
}

#[test]
fn an_identifier_still_in_use_is_skipped() {
    let live = [5_u64, 6];
    let in_use = |candidate: u64| live.contains(&candidate);
    assert_eq!(next_free_identifier(4, live.len(), in_use), Some(7));
}

#[test]
fn a_live_identifier_after_the_wrap_is_skipped() {
    let live = [LAST_NATIVE_IDENTIFIER, 1, 2];
    let in_use = |candidate: u64| live.contains(&candidate);
    assert_eq!(next_free_identifier(LAST_NATIVE_IDENTIFIER - 1, live.len(), in_use), Some(3));
}

#[test]
fn a_count_lower_than_the_identifiers_in_use_ends_the_search() {
    let every_identifier_in_use = |_candidate: u64| true;
    assert_eq!(next_free_identifier(0, 3, every_identifier_in_use), None);
}
