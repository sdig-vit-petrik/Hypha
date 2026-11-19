//! `hypha_param_topic` and `hypha_param_port_supervision`: an endpoint's port table.
#![allow(clippy::expect_used)]

mod common;
use crate::common::supervision::{bind_with_capacity, cycle_with_input};
use crate::common::unique;

use std::os::raw::c_char;
use std::ptr;

use hypha_ffi::*;

/// A name no other test of this process, or another process, uses.
struct Node {
    handle: *mut HyphaNode,
}

impl Node {
    fn with_parameters(text: &str) -> Self {
        let path = std::env::temp_dir().join(format!("{}.toml", unique("param_topic")));
        std::fs::write(&path, text).expect("parameter file");
        let name = unique("param_topic");
        let path_text = path.to_str().expect("UTF-8 path");
        let mut handle = ptr::null_mut();
        let status = unsafe {
            hypha_node_create(
                name.as_ptr().cast::<c_char>(),
                name.len(),
                path_text.as_ptr().cast::<c_char>(),
                path_text.len(),
                ptr::null(),
                &raw mut handle,
            )
        };
        std::fs::remove_file(&path).expect("remove parameter file");
        assert_eq!(status, HyphaStatus::Ok);
        Self { handle }
    }

    fn topic(
        &self,
        direction: u32,
        name: &str,
        rule: u32,
        capacity: usize,
    ) -> (HyphaStatus, String, usize) {
        let mut buffer = vec![0_u8; capacity];
        let mut length = 0_usize;
        let status = unsafe {
            hypha_param_topic(
                self.handle,
                direction,
                name.as_ptr().cast::<c_char>(),
                name.len(),
                rule,
                buffer.as_mut_ptr().cast::<c_char>(),
                buffer.len(),
                &raw mut length,
            )
        };
        let text = buffer
            .get(..length.min(capacity))
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned());
        (status, text.unwrap_or_default(), length)
    }
}

const OPTIONAL: u32 = HYPHA_PORT_TOPIC_OPTIONAL;
const REQUIRED: u32 = HYPHA_PORT_TOPIC_REQUIRED;

const MILLI: i64 = 1_000_000;
const HOUR: i64 = 3_600_000 * MILLI;

/// The range a port limit must fall in: one millisecond to one hour.
const RANGES: HyphaSupervisionRanges = HyphaSupervisionRanges {
    silence_min_nanos: MILLI,
    silence_max_nanos: HOUR,
    stagnation_min_nanos: MILLI,
    stagnation_max_nanos: HOUR,
};

/// The checks of one call to `hypha_param_port_supervision`.
#[derive(Clone, Copy)]
#[allow(clippy::struct_excessive_bools, reason = "the four flags of the C call")]
struct Check {
    silence: bool,
    stagnation: bool,
    takes_topic: bool,
    read_limits: bool,
    ranges: HyphaSupervisionRanges,
}

/// Both checks declared, a port that takes a topic, limits read, the usual range.
const ALL: Check =
    Check { silence: true, stagnation: true, takes_topic: true, read_limits: true, ranges: RANGES };

#[derive(Debug)]
struct Outcome {
    status: HyphaStatus,
    detail: String,
    length: usize,
    limits: HyphaSupervisionLimits,
    ignored: u8,
}

impl Node {
    fn supervision(&self, direction: u32, name: &str, check: Check, capacity: usize) -> Outcome {
        assert_eq!(direction, HYPHA_PORT_INPUT, "supervision is bound to inputs");
        let cycle = unsafe { cycle_with_input(self.handle, check.read_limits) };
        let bound = unsafe {
            bind_with_capacity(
                cycle,
                self.handle,
                0,
                name,
                (check.silence, check.stagnation, check.takes_topic),
                &check.ranges,
                capacity,
            )
        };
        Outcome {
            status: bound.status,
            detail: bound.detail,
            length: bound.length,
            limits: bound.limits,
            ignored: bound.ignored,
        }
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        unsafe { hypha_node_destroy(self.handle) };
    }
}

const PARAMETERS: &str = r#"[ports.inputs]
legacy = "/odom_speed"

[ports.inputs.speed]
topic = "/odom_speed"

[ports.inputs.count]
topic = 5

[ports.inputs.flag]
topic = true

[ports.inputs.spaced]
topic = "/has space"

[ports.inputs.own]
topic = "~/state_in"

[ports.inputs.switched_off]
topic = ""

[ports.inputs.bare]
silence_ms = 250

[ports.outputs]
legacy = "/odom_out"

[ports.outputs.command]
topic = "/speed_command"

[ports.outputs.speed]
topic = "/odom_out"
"#;

#[test]
fn a_valid_topic_in_the_table_is_returned() {
    let node = Node::with_parameters(PARAMETERS);
    assert_eq!(
        node.topic(HYPHA_PORT_INPUT, "speed", REQUIRED, 64),
        (HyphaStatus::Ok, "/odom_speed".to_string(), 11)
    );
}

#[test]
fn a_required_port_refuses_an_empty_entry_and_an_absent_key() {
    let node = Node::with_parameters(PARAMETERS);
    assert_eq!(
        node.topic(HYPHA_PORT_INPUT, "switched_off", REQUIRED, 64),
        (HyphaStatus::InvalidArgument, String::new(), 0)
    );
    assert_eq!(node.topic(HYPHA_PORT_INPUT, "command", REQUIRED, 64).0, HyphaStatus::NotAvailable);
    assert_eq!(
        node.topic(HYPHA_PORT_INPUT, "speed", REQUIRED, 64),
        (HyphaStatus::Ok, "/odom_speed".to_string(), 11)
    );
}

#[test]
fn an_unknown_topic_rule_is_refused() {
    let node = Node::with_parameters(PARAMETERS);
    assert_eq!(node.topic(HYPHA_PORT_INPUT, "speed", 2, 64).0, HyphaStatus::InvalidArgument);
    assert_eq!(node.topic(HYPHA_PORT_INPUT, "speed", u32::MAX, 64).0, HyphaStatus::InvalidArgument);
}

#[test]
fn an_output_reads_the_outputs_table_and_an_input_reads_the_inputs_table() {
    let node = Node::with_parameters(PARAMETERS);
    assert_eq!(node.topic(HYPHA_PORT_OUTPUT, "speed", REQUIRED, 64).1, "/odom_out");
    assert_eq!(node.topic(HYPHA_PORT_INPUT, "speed", REQUIRED, 64).1, "/odom_speed");
}

#[test]
fn a_name_in_the_other_table_is_not_found() {
    let node = Node::with_parameters(PARAMETERS);
    assert_eq!(node.topic(HYPHA_PORT_INPUT, "command", REQUIRED, 64).0, HyphaStatus::NotAvailable);
    assert_eq!(node.topic(HYPHA_PORT_OUTPUT, "own", REQUIRED, 64).0, HyphaStatus::NotAvailable);
}

#[test]
fn an_unknown_direction_is_refused() {
    let node = Node::with_parameters(PARAMETERS);
    assert_eq!(node.topic(7, "speed", REQUIRED, 64).0, HyphaStatus::InvalidArgument);
}

#[test]
fn a_number_or_a_bool_is_refused() {
    let node = Node::with_parameters(PARAMETERS);
    assert_eq!(node.topic(HYPHA_PORT_INPUT, "count", REQUIRED, 64).0, HyphaStatus::InvalidArgument);
    assert_eq!(node.topic(HYPHA_PORT_INPUT, "flag", OPTIONAL, 64).0, HyphaStatus::InvalidArgument);
}

#[test]
fn an_invalid_topic_name_is_refused() {
    let node = Node::with_parameters(PARAMETERS);
    assert_eq!(node.topic(HYPHA_PORT_INPUT, "spaced", REQUIRED, 64).0, HyphaStatus::InvalidTopic);
}

#[test]
fn a_too_small_buffer_reports_the_length() {
    let node = Node::with_parameters(PARAMETERS);
    let (status, _, length) = node.topic(HYPHA_PORT_INPUT, "speed", REQUIRED, 4);
    assert_eq!((status, length), (HyphaStatus::TooLarge, 11));
}

#[test]
fn a_node_scoped_name_is_returned_as_written() {
    let node = Node::with_parameters(PARAMETERS);
    let (status, topic, _) = node.topic(HYPHA_PORT_INPUT, "own", REQUIRED, 64);
    assert_eq!((status, topic.as_str()), (HyphaStatus::Ok, "~/state_in"));
}

#[test]
fn a_string_entry_is_refused_because_the_entry_must_be_a_table() {
    let node = Node::with_parameters(PARAMETERS);
    assert_eq!(
        node.topic(HYPHA_PORT_INPUT, "legacy", REQUIRED, 64),
        (HyphaStatus::InvalidArgument, String::new(), 0)
    );
}

#[test]
fn a_table_without_topic_is_not_found() {
    let node = Node::with_parameters(PARAMETERS);
    assert_eq!(node.topic(HYPHA_PORT_INPUT, "bare", REQUIRED, 64).0, HyphaStatus::NotAvailable);
    assert_eq!(
        node.topic(HYPHA_PORT_INPUT, "bare", OPTIONAL, 64),
        (HyphaStatus::Ok, String::new(), 0)
    );
}

#[path = "param_topic_boundary/port_keys.rs"]
mod port_keys;
