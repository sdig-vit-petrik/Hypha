//! Verifies which subscription patterns the matcher can follow.

use hypha_core::port::is_valid_topic_pattern;

#[test]
fn a_pattern_with_whole_star_segments_is_accepted() {
    for pattern in ["/a/*", "/*/b", "/*", "/a/*/c", "/a/**", "/a/*/**", "/*/**", "**", "a/*"] {
        assert!(is_valid_topic_pattern(pattern), "{pattern:?} must be accepted");
    }
}

#[test]
fn a_literal_name_is_accepted_as_a_pattern() {
    assert!(is_valid_topic_pattern("/a/b"));
    assert!(is_valid_topic_pattern("/a"));
}

#[test]
fn a_pattern_with_a_star_inside_a_segment_is_refused() {
    for pattern in ["/a/b*", "/a/*b", "/a/x*y", "*b", "/a/b**", "/a/***"] {
        assert!(!is_valid_topic_pattern(pattern), "{pattern:?} must be refused");
    }
}

#[test]
fn a_pattern_with_a_double_star_that_is_not_last_is_refused() {
    for pattern in ["/**/b", "/a/**/b", "/**/*", "/a/**x"] {
        assert!(!is_valid_topic_pattern(pattern), "{pattern:?} must be refused");
    }
}

#[test]
fn an_empty_pattern_is_refused() {
    assert!(!is_valid_topic_pattern(""));
    assert!(!is_valid_topic_pattern("/"));
}
