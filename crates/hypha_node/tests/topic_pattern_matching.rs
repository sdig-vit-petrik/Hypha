//! The pattern rules a node sees match the library's own.

use hypha_node::sdk::topic_matches_pattern;

#[test]
fn one_star_matches_one_segment_only() {
    assert!(topic_matches_pattern("/fluid/p/*", "/fluid/p/7"));
    assert!(!topic_matches_pattern("/fluid/p/*", "/fluid/p/7/extra"));
    assert!(!topic_matches_pattern("/fluid/p/*", "/other/p/7"));
}

#[test]
fn an_unmatched_topic_is_refused() {
    assert!(!topic_matches_pattern("/a/b", "/a/c"));
    assert!(!topic_matches_pattern("", "/a"));
}
