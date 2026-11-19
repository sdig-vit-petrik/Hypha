//! Verifies which topics a subscription pattern matches.

use hypha_core::port::topic_matches_pattern;

#[test]
fn an_identical_name_matches() {
    assert!(topic_matches_pattern("/a/b", "/a/b"));
    assert!(topic_matches_pattern("/a", "/a"));
}

#[test]
fn the_catch_all_matches_every_topic() {
    assert!(topic_matches_pattern("**", "/a"));
    assert!(topic_matches_pattern("**", "/a/b/c"));
    assert!(topic_matches_pattern("**", "/"));
}

#[test]
fn a_single_star_matches_exactly_one_segment() {
    assert!(topic_matches_pattern("/a/*", "/a/b"));
    assert!(!topic_matches_pattern("/a/*", "/a/b/c"), "one star must not span two segments");
    assert!(!topic_matches_pattern("/a/*", "/a"), "one star needs a segment to match");
    assert!(topic_matches_pattern("/*/b", "/a/b"));
    assert!(!topic_matches_pattern("/*/b", "/a/c"));
}

#[test]
fn a_trailing_double_star_matches_the_rest() {
    assert!(topic_matches_pattern("/a/**", "/a"));
    assert!(topic_matches_pattern("/a/**", "/a/b"));
    assert!(topic_matches_pattern("/a/**", "/a/b/c/d"));
    assert!(!topic_matches_pattern("/a/**", "/b/c"), "the prefix must still match");
}

#[test]
fn a_different_name_does_not_match() {
    assert!(!topic_matches_pattern("/a/b", "/a/c"));
    assert!(!topic_matches_pattern("/a/b", "/a"));
    assert!(!topic_matches_pattern("/a", "/a/b"));
    assert!(!topic_matches_pattern("", "/a"));
}

#[test]
fn a_leading_slash_is_not_significant() {
    // Empty segments are ignored, so the same topic matches either spelling.
    assert!(topic_matches_pattern("a/b", "/a/b"));
    assert!(topic_matches_pattern("/a/b", "a/b"));
}
