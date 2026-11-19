//! The verdict on the newest value of one input at one instant.

use hypha_cycle::{InputVerdict, judge_value};

#[test]
fn a_value_that_never_arrived_is_never_arrived() {
    assert_eq!(judge_value(0, None, 1_000), InputVerdict::NeverArrived);
    assert_eq!(judge_value(500, None, 1_000), InputVerdict::NeverArrived);
}

#[test]
fn a_value_inside_its_age_limit_is_usable() {
    assert_eq!(judge_value(500, Some(600), 1_000), InputVerdict::Usable);
    assert_eq!(judge_value(500, Some(500), 1_000), InputVerdict::Usable, "the limit is inclusive");
}

#[test]
fn a_value_past_its_age_limit_is_stale() {
    assert_eq!(judge_value(500, Some(499), 1_000), InputVerdict::Stale);
}

#[test]
fn a_zero_age_limit_keeps_every_earlier_value_usable() {
    assert_eq!(judge_value(0, Some(1), 1_000_000), InputVerdict::Usable);
}

#[test]
fn a_value_sent_after_the_instant_is_stale() {
    assert_eq!(judge_value(0, Some(1_001), 1_000), InputVerdict::Stale);
}
