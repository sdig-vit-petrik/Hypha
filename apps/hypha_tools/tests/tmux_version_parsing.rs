//! Tests for reading the version that `tmux -V` prints.

use hypha_tools::launch_tmux::{MIN_TMUX_VERSION, parse_tmux_version};

#[test]
fn release_versions_are_read() {
    assert_eq!(parse_tmux_version("tmux 3.4"), Some((3, 4)));
    assert_eq!(parse_tmux_version("tmux 3.3a\n"), Some((3, 3)));
    assert_eq!(parse_tmux_version("tmux 2.9a"), Some((2, 9)));
    assert_eq!(parse_tmux_version("tmux 3.0"), Some((3, 0)));
}

#[test]
fn development_versions_are_read() {
    assert_eq!(parse_tmux_version("tmux next-3.6"), Some((3, 6)));
}

#[test]
fn unreadable_versions_are_refused() {
    assert_eq!(parse_tmux_version(""), None);
    assert_eq!(parse_tmux_version("tmux master"), None);
    assert_eq!(parse_tmux_version("screen 4.9"), None);
}

#[test]
fn minimum_version_is_three_zero() {
    assert_eq!(MIN_TMUX_VERSION, (3, 0));
    assert!(parse_tmux_version("tmux 2.9a").unwrap_or((0, 0)) < MIN_TMUX_VERSION);
    assert!(parse_tmux_version("tmux 3.0").unwrap_or((0, 0)) >= MIN_TMUX_VERSION);
}
