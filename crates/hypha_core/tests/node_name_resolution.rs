//! Node names and the `~/` names a node owns.

use hypha_core::port::{PortError, is_cli_node_name, is_node_name, resolve_node_name};

#[test]
fn a_node_scoped_name_resolves_under_the_node() {
    assert_eq!(resolve_node_name("~/speed", "ctrl_front"), Ok("/ctrl_front/speed".to_string()));
    assert_eq!(resolve_node_name("~/a/b", "n"), Ok("/n/a/b".to_string()));
}

#[test]
fn other_names_are_returned_unchanged() {
    assert_eq!(resolve_node_name("/odom_speed", "n"), Ok("/odom_speed".to_string()));
    assert_eq!(resolve_node_name("add_two_ints", "n"), Ok("add_two_ints".to_string()));
    assert_eq!(resolve_node_name("", "n"), Ok(String::new()));
}

#[test]
fn a_tilde_without_a_slash_is_refused() {
    assert_eq!(resolve_node_name("~", "n"), Err(PortError::InvalidTopic));
    assert_eq!(resolve_node_name("~x", "n"), Err(PortError::InvalidTopic));
    assert_eq!(resolve_node_name("~/", "n"), Err(PortError::InvalidTopic));
}

#[test]
fn node_names_follow_the_identifier_rule() {
    assert!(is_node_name("speed_controller"));
    assert!(is_node_name("a"));
    assert!(is_node_name(&"a".repeat(64)));
    assert!(!is_node_name(""));
    assert!(!is_node_name(&"a".repeat(65)));
    assert!(!is_node_name("Speed"));
    assert!(!is_node_name("1speed"));
    assert!(!is_node_name("speed-controller"));
    assert!(!is_node_name("/speed"));
}

#[test]
fn a_cli_tool_name_is_the_prefix_and_a_node_name() {
    assert!(is_cli_node_name("_hyphacli_top"));
    assert!(is_cli_node_name("_hyphacli_top_schema_resolver_0"));
    assert!(is_cli_node_name(&format!("_hyphacli_{}", "a".repeat(54))));
    assert!(!is_cli_node_name(&format!("_hyphacli_{}", "a".repeat(55))));
    assert!(!is_cli_node_name("_hyphacli_"));
    assert!(!is_cli_node_name("_hyphacli_Top"));
    assert!(!is_cli_node_name("_hyphacli_1top"));
    assert!(!is_cli_node_name("_other_top"));
    assert!(!is_cli_node_name("hyphacli_top"));
    assert!(!is_cli_node_name("hypha_top"));
    // A node name never starts with the prefix, so the two rules never overlap.
    assert!(!is_node_name("_hyphacli_top"));
}
