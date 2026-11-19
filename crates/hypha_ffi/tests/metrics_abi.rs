//! The metrics ABI: every name a node emits, in order. The names are part of the ABI.
#![allow(clippy::expect_used)]

mod common;
use crate::common::{create_node, node_metrics, unique};

use std::ptr;

use hypha_ffi::*;

const NODE_NAMES: [&str; 7] = [
    "node.callbacks_invoked",
    "node.callback_failures",
    "node.slow_callbacks",
    "node.sweeps",
    "node.last_sweep_end_nanos",
    "node.last_callback_status",
    "node.is_sim_time",
];
const TRANSPORT_NAMES: [&str; 1] = ["transport.lost_wakes"];
const SCHEMA_NAMES: [&str; 3] =
    ["schema.invalid_records", "schema.unavailable_lookups", "schema.resource_limit_failures"];
const LOG_NAMES: [&str; 3] = ["log.lines_written", "log.write_failures", "log.rejected_lines"];
fn names(list: &[(String, u64)]) -> Vec<&str> {
    list.iter().map(|(name, _)| name.as_str()).collect()
}

fn expected(parts: &[&[&'static str]]) -> Vec<&'static str> {
    parts.iter().flat_map(|part| part.iter().copied()).collect()
}

#[test]
fn node_metrics_list_node_schema_and_log_names_in_order() {
    let node = create_node(&unique("metrics"));

    let list = node_metrics(node);

    assert_eq!(names(&list), expected(&[&NODE_NAMES, &TRANSPORT_NAMES, &SCHEMA_NAMES, &LOG_NAMES]));
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn node_metrics_without_a_node_list_only_log_names() {
    let list = node_metrics(ptr::null());

    assert_eq!(names(&list), LOG_NAMES.to_vec());
}
