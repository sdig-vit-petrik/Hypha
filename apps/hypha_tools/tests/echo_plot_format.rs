//! Tests for the `hypha topic echo -p` row format.
#![allow(clippy::expect_used)]

use std::fmt::Write as _;

use hypha_tools::msg_plot::{plot_header, plot_row, plot_values};
use hypha_tools::msg_reflect::MessageSchema;

fn append_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn append_string(buf: &mut Vec<u8>, s: &str) {
    append_u32(buf, u32::try_from(s.len()).unwrap_or(u32::MAX));
    buf.extend_from_slice(s.as_bytes());
}

const NESTED: &str = "Header header
float64[] samples
float32[2] pair
bool ok
string state
================================================================================
MSG: std_msgs/Header
uint32 seq
time stamp
string frame_id
";

fn nested_wire(state: &str) -> Vec<u8> {
    let mut wire = Vec::new();
    append_u32(&mut wire, 7); // header.seq
    append_u32(&mut wire, 12); // header.stamp.secs
    append_u32(&mut wire, 500); // header.stamp.nsecs
    append_string(&mut wire, "map");
    append_u32(&mut wire, 3); // samples length
    for v in [0.1_f64, -2.0, 1e-9] {
        wire.extend_from_slice(&v.to_le_bytes());
    }
    for v in [1.5_f32, 2.5] {
        wire.extend_from_slice(&v.to_le_bytes());
    }
    wire.push(1);
    append_string(&mut wire, state);
    wire
}

fn schema() -> MessageSchema {
    MessageSchema::from_definition("test_msgs/Nested", NESTED).expect("definition parses")
}

#[test]
fn header_names_flatten_nested_fields_and_array_elements() {
    let columns = plot_values("test_msgs/Nested", &mut schema(), &nested_wire("idle"))
        .expect("wire is complete");
    assert_eq!(
        plot_header(&columns),
        "%time,header.seq,header.stamp.secs,header.stamp.nsecs,header.frame_id,\
         samples0,samples1,samples2,pair0,pair1,ok,state"
    );
}

#[test]
fn row_starts_with_the_publish_time_and_keeps_full_float_precision() {
    let columns = plot_values("test_msgs/Nested", &mut schema(), &nested_wire("idle"))
        .expect("wire is complete");
    assert_eq!(
        plot_row(123_456_789, &columns),
        "123456789,7,12,500,map,0.1,-2,0.000000001,1.5,2.5,1,idle"
    );
}

#[test]
fn string_with_escape_codes_is_printed_verbatim() {
    let colored = "\u{1b}[31mFAULT\u{1b}[0m x=1";
    let columns = plot_values("test_msgs/Nested", &mut schema(), &nested_wire(colored))
        .expect("wire is complete");
    assert!(plot_row(0, &columns).ends_with(&format!(",1,{colored}")));
}

#[test]
fn short_payloads_give_no_row() {
    // A payload cut short of its last field.
    let wire = nested_wire("idle");
    let cut = wire.get(..wire.len().saturating_sub(3)).expect("wire is long enough");
    assert!(plot_values("test_msgs/Nested", &mut schema(), cut).is_err());

    // An array length the payload cannot hold.
    let mut wire = Vec::new();
    append_u32(&mut wire, u32::MAX);
    let mut series = MessageSchema::from_definition("test_msgs/Series", "float64[] v\n")
        .expect("definition parses");
    assert!(plot_values("test_msgs/Series", &mut series, &wire).is_err());
}

#[test]
fn unknown_type_gives_no_row() {
    assert!(plot_values("test_msgs/Missing", &mut schema(), &[]).is_err());
}

/// A type that holds itself reads no bytes per level, so only the depth limit stops it.
#[test]
fn a_recursive_definition_is_refused_at_the_depth_limit() {
    // A cycle is refused when the schema is built, before any plot walks it.
    let cause = MessageSchema::from_definition("pkg/Root", "Root child\nint32 a\n")
        .map(|_| ())
        .expect_err("refused");
    assert!(cause.contains("depends on itself"), "{cause}");
}

/// A plot accepts the deepest nesting it allows and refuses one level more.
#[test]
fn nesting_is_limited_to_max_nesting_depth() {
    use hypha_node::sdk::msgdef::spec::MAX_NESTING_DEPTH;
    // `levels` nested messages below the root: pkg/T1 .. pkg/T{levels}.
    let chain = |levels: usize| {
        let separator = "=".repeat(80);
        let mut sections = vec![String::from("T1 x\n")];
        for (level, next) in (1..levels).zip(2..) {
            sections.push(format!("{separator}\nMSG: pkg/T{level}\nT{next} x\n"));
        }
        sections.push(format!("{separator}\nMSG: pkg/T{levels}\nuint8 v\n"));
        MessageSchema::from_definition("pkg/Root", &sections.concat())
    };
    // The root plus MAX_NESTING_DEPTH - 1 nested messages.
    let mut schema = chain(MAX_NESTING_DEPTH.saturating_sub(1)).expect("definition parses");
    assert!(plot_values("pkg/Root", &mut schema, &[7]).is_ok());
    // One level more is refused when the schema is built, before any plot walks it.
    let cause = chain(MAX_NESTING_DEPTH).map(|_| ()).expect_err("refused");
    assert!(cause.contains("nests deeper than 64 levels"), "{cause}");
}

/// The shape that defeated the old depth check: `pkg/Root` holds `pkg/T{j}` for every multiple of
/// 60 below `levels`, descending, and `pkg/T{i}` holds `pkg/T{i+1}`, the last holding a `uint8`.
#[test]
fn a_shared_chain_past_the_depth_limit_is_refused() {
    let separator = "=".repeat(80);
    let levels: usize = 200;
    let mut text = String::new();
    for index in (0..levels).step_by(60).rev() {
        let _ = writeln!(text, "pkg/T{index} f{index}");
    }
    for level in 0..levels {
        let _ = write!(text, "\n{separator}\nMSG: pkg/T{level}\n");
        let next = level.saturating_add(1);
        if next < levels {
            let _ = writeln!(text, "pkg/T{next} child");
        } else {
            text.push_str("uint8 v\n");
        }
    }
    // The deepest path is checked when the schema is built, before any plot walks it.
    let cause = MessageSchema::from_definition("pkg/Root", &text).map(|_| ()).expect_err("refused");
    assert!(cause.contains("nests deeper than 64 levels"), "{cause}");
}
