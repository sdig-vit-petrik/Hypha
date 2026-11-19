//! The `[cycle]` and `[ports.inputs.<name>]` tables of a class-based cycle.
#![allow(clippy::expect_used)]

mod common;
use crate::common::introspection::introspection_text;
use crate::common::supervision::{Bound, bind, cycle_with_input_mode, node_with_parameter_text};
use crate::common::unique;

use hypha_ffi::*;

const NO_CHECKS: (bool, bool, bool) = (false, false, true);

/// A queued cycle `name` whose only input reads the table `[ports.inputs.speed]`.
fn queued_cycle(node: *mut HyphaNode, name: &str) -> *mut HyphaCycle {
    unsafe { cycle_with_input_mode(node, true, name, HyphaCycleInputMode::Queued) }
}

fn bind_speed(node: *mut HyphaNode, cycle: *mut HyphaCycle) -> Bound {
    unsafe { bind(cycle, node, 0, "speed", NO_CHECKS) }
}

fn input_table(policy_line: &str) -> String {
    format!("[ports.inputs.speed]\ntopic = \"/speed\"\n{policy_line}\n")
}

fn policy_in_snapshot(policy_value: &str) -> String {
    let node = node_with_parameter_text("overflow_valid", &input_table(policy_value));
    let cycle = queued_cycle(node, &unique("policy_cycle"));
    let bound = bind_speed(node, cycle);
    assert_eq!(bound.status, HyphaStatus::Ok, "detail: {}", bound.detail);
    let text = introspection_text(node);
    unsafe {
        hypha_cycle_destroy(cycle);
        hypha_node_destroy(node);
    }
    text
}

#[test]
fn an_overflow_policy_key_is_unknown() {
    let bound = refused_policy("overflow_policy = \"drop_oldest\"");
    assert_eq!(bound.status, HyphaStatus::InvalidArgument);
    assert!(bound.detail.contains("unknown key 'overflow_policy'"), "detail: {}", bound.detail);
    assert_eq!(bound.attached, 0);
}

#[test]
fn a_queued_input_reaches_the_snapshot_without_a_policy() {
    // A queued input always drops its oldest work, so the snapshot names no policy.
    let text = policy_in_snapshot("");
    assert!(text.contains("queued="), "{text}");
    assert!(!text.contains("overflow="), "{text}");
}

fn refused_policy(policy_line: &str) -> Bound {
    let node = node_with_parameter_text("overflow_refused", &input_table(policy_line));
    let cycle = queued_cycle(node, &unique("refused_cycle"));
    let bound = bind_speed(node, cycle);
    unsafe {
        hypha_cycle_destroy(cycle);
        hypha_node_destroy(node);
    }
    bound
}

fn budget_commit(table_value: &str, class_based: bool) -> (HyphaStatus, String) {
    let name = unique("budget");
    let text = format!("[cycle]\n{table_value}\n");
    let node = node_with_parameter_text("budget_table", &text);
    let cycle = queued_cycle(node, &name);
    assert_eq!(
        unsafe { hypha_cycle_set_class_based(cycle, u8::from(class_based)) },
        HyphaStatus::Ok
    );
    let status = unsafe { hypha_cycle_commit(cycle) };
    let text = introspection_text(node);
    unsafe {
        hypha_cycle_destroy(cycle);
        hypha_node_destroy(node);
    }
    (status, text)
}

#[test]
fn a_class_based_cycle_reads_its_arrival_budget_from_the_table() {
    let (status, text) = budget_commit("arrival_budget = 3", true);
    assert_eq!(status, HyphaStatus::Ok);
    let row = text.lines().find(|line| line.contains("arrival_budget")).expect("budget row");
    assert!(row.split_whitespace().any(|cell| cell == "3"), "{text}");
}

#[test]
fn an_invalid_arrival_budget_refuses_the_commit() {
    for value in ["arrival_budget = 0", "arrival_budget = -4", "arrival_budget = \"many\""] {
        let (status, _) = budget_commit(value, true);
        assert_eq!(status, HyphaStatus::InvalidArgument, "{value}");
    }
}

#[test]
fn the_cycle_table_accepts_a_positive_rate_and_refuses_any_other() {
    for value in ["rate_hz = 10", "rate_hz = 2.5"] {
        let (status, _) = budget_commit(value, true);
        assert_eq!(status, HyphaStatus::Ok, "{value}");
    }
    for value in ["rate_hz = 0", "rate_hz = -1.0", "rate_hz = \"fast\""] {
        let (status, _) = budget_commit(value, true);
        assert_eq!(status, HyphaStatus::InvalidArgument, "{value}");
    }
}

#[test]
fn an_unknown_key_in_the_cycle_table_refuses_the_commit() {
    let (status, _) = budget_commit("arrival_limit = 3", true);
    assert_eq!(status, HyphaStatus::InvalidArgument);
}

#[test]
fn a_cycle_that_is_not_class_based_ignores_the_cycle_table() {
    let (status, text) = budget_commit("arrival_budget = 0", false);
    assert_eq!(status, HyphaStatus::Ok);
    assert!(!text.contains("arrival_budget"), "{text}");
}

#[test]
fn the_class_call_refuses_values_outside_their_range() {
    let node = node_with_parameter_text("budget_range", "");
    let cycle = queued_cycle(node, &unique("range_cycle"));
    assert_eq!(unsafe { hypha_cycle_set_class_based(cycle, 2) }, HyphaStatus::InvalidArgument);
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_cycle_set_class_based(cycle, 1) }, HyphaStatus::InvalidState);
    unsafe {
        hypha_cycle_destroy(cycle);
        hypha_node_destroy(node);
    }
}
