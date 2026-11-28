//! The checks an application reports to its node reach `~/state` through the C ABI, and
//! the state follows them: an ERROR or STALE one degrades the node, a WARN one does not.
#![allow(clippy::expect_used)]

mod common;
use crate::common::{create_node, unique};

use std::ptr;

use hypha_core::message::DecodeOwned as _;
use hypha_ffi::*;
use hypha_msgs::hypha_msgs::{NodeCheck, NodeState};

/// A check that borrows `name` and `message`.
fn check(name: &str, level: u8, message: &str) -> HyphaNodeCheck {
    HyphaNodeCheck {
        name: name.as_ptr().cast(),
        name_len: name.len(),
        level,
        message: if message.is_empty() { ptr::null() } else { message.as_ptr().cast() },
        message_len: message.len(),
    }
}

fn report(node: *const HyphaNode, owner: u64, checks: &[HyphaNodeCheck]) -> HyphaStatus {
    unsafe { hypha_node_set_checks(node, owner, checks.as_ptr(), checks.len()) }
}

fn state(node: *const HyphaNode) -> NodeState {
    let mut buffer = vec![0_u8; 64 * 1024];
    let mut length = 0_usize;
    let status =
        unsafe { hypha_node_state(node, buffer.as_mut_ptr(), buffer.len(), &raw mut length) };
    assert_eq!(status, HyphaStatus::Ok);
    buffer.truncate(length);
    NodeState::decode_from(&buffer).expect("a NodeState")
}

#[test]
fn an_error_check_degrades_the_node_and_its_row_is_in_the_state() {
    let node = create_node(&unique("checks_error"));
    assert_eq!(state(node).state, NodeState::INPUTS_USABLE, "a node with no input and no check");

    let status = report(node, 1, &[check("Device", NodeCheck::ERROR, "serial port absent")]);
    assert_eq!(status, HyphaStatus::Ok);

    let reported = state(node);
    assert_eq!(reported.state, NodeState::DEGRADED);
    assert_eq!(
        reported.checks,
        [NodeCheck {
            name: "Device".to_owned(),
            level: NodeCheck::ERROR,
            message: "serial port absent".to_owned()
        }]
    );
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn a_warn_check_shows_and_leaves_the_node_usable_until_the_next_report_clears_it() {
    let node = create_node(&unique("checks_warn"));
    assert_eq!(report(node, 7, &[check("Rate", NodeCheck::WARN, "8 Hz")]), HyphaStatus::Ok);
    let warned = state(node);
    assert_eq!(warned.state, NodeState::INPUTS_USABLE);
    assert_eq!(warned.checks.len(), 1);

    assert_eq!(report(node, 7, &[check("Rate", NodeCheck::STALE, "")]), HyphaStatus::Ok);
    assert_eq!(state(node).state, NodeState::DEGRADED, "STALE counts as ERROR");

    assert_eq!(report(node, 7, &[]), HyphaStatus::Ok);
    let cleared = state(node);
    assert_eq!(cleared.state, NodeState::INPUTS_USABLE, "an empty report removes the checks");
    assert_eq!(cleared.checks.len(), 0);
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn owners_report_apart_and_a_refused_report_changes_nothing() {
    let node = create_node(&unique("checks_owners"));
    assert_eq!(report(node, 1, &[check("A", NodeCheck::OK, "ok")]), HyphaStatus::Ok);
    assert_eq!(report(node, 2, &[check("A", NodeCheck::WARN, "warm")]), HyphaStatus::Ok);
    assert_eq!(state(node).checks.len(), 2, "a name may repeat across owners");

    let long_name = "n".repeat(129);
    assert_eq!(
        report(node, 1, &[check(&long_name, NodeCheck::OK, "")]),
        HyphaStatus::InvalidArgument
    );
    assert_eq!(report(node, 1, &[check("A", 4, "")]), HyphaStatus::InvalidArgument);
    assert_eq!(report(node, 1, &[check("", NodeCheck::OK, "")]), HyphaStatus::InvalidArgument);
    let too_many: Vec<HyphaNodeCheck> = (0..64).map(|_| check("n", NodeCheck::OK, "")).collect();
    assert_eq!(report(node, 1, &too_many), HyphaStatus::TooLarge);
    assert!(hypha_last_error_text().contains("at most 64"), "{}", hypha_last_error_text());
    assert_eq!(state(node).checks.len(), 2, "the refused reports changed nothing");

    let message = "m".repeat(300);
    assert_eq!(report(node, 3, &[check("Long", NodeCheck::OK, &message)]), HyphaStatus::Ok);
    let long = state(node).checks.into_iter().find(|row| row.name == "Long").expect("reported");
    assert_eq!(long.message.len(), 256);
    assert!(long.message.ends_with("..."));
    unsafe { hypha_node_destroy(node) };
}

fn hypha_last_error_text() -> String {
    let mut buffer = vec![0_u8; 512];
    let mut length = 0_usize;
    let status =
        unsafe { hypha_last_error(buffer.as_mut_ptr().cast(), buffer.len(), &raw mut length) };
    assert_eq!(status, HyphaStatus::Ok);
    buffer.truncate(length);
    String::from_utf8_lossy(&buffer).into_owned()
}
