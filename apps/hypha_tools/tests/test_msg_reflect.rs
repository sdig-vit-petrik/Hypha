//! Tests for dynamic message reflection and wire decoding.

#![allow(clippy::expect_used)]

use std::fmt::Write as _;

use hypha_tools::msg_reflect::{MessageSchema, WalkEvent, decode_message, walk};

fn append_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn append_f64(buf: &mut Vec<u8>, v: f64) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn append_f32(buf: &mut Vec<u8>, v: f32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn append_string(buf: &mut Vec<u8>, s: &str) {
    #[allow(clippy::cast_possible_truncation)]
    let len = s.len() as u32;
    append_u32(buf, len);
    buf.extend_from_slice(s.as_bytes());
}

#[test]
fn decodes_flat_primitives() {
    let mut schema = MessageSchema::from_definition(
        "test_msgs/Simple",
        "uint32 a
float64 b
string c
bool d
",
    )
    .expect("definition parses");

    let mut wire = Vec::new();
    append_u32(&mut wire, 42);
    append_f64(&mut wire, 3.5);
    append_string(&mut wire, "hi");
    wire.push(1); // d = true

    let mut out = String::new();
    let ok = decode_message(&mut out, "test_msgs/Simple", &mut schema, &wire);

    assert!(ok);
    assert!(
        out.contains(
            "a: 42
"
        ),
        "got:
{out}"
    );
    assert!(
        out.contains(
            "b: 3.5
"
        ),
        "got:
{out}"
    );
    assert!(
        out.contains(
            "c: \"hi\"
"
        ),
        "got:
{out}"
    );
    assert!(
        out.contains(
            "d: true
"
        ),
        "got:
{out}"
    );
}

#[test]
fn decodes_nested_header() {
    let mut schema = MessageSchema::from_definition(
        "test_msgs/WithHeader",
        "std_msgs/Header header
\
         float64 x
\
         ================================================================================
\
         MSG: std_msgs/Header
\
         uint32 seq
\
         time stamp
\
         string frame_id
",
    )
    .expect("definition parses");

    let mut wire = Vec::new();
    append_u32(&mut wire, 7);
    append_u32(&mut wire, 100);
    append_u32(&mut wire, 200);
    append_string(&mut wire, "map");
    append_f64(&mut wire, 1.25);

    let mut out = String::new();
    let ok = decode_message(&mut out, "test_msgs/WithHeader", &mut schema, &wire);

    assert!(ok);
    assert!(
        out.contains(
            "header:
"
        ),
        "got:
{out}"
    );
    assert!(
        out.contains(
            "  seq: 7
"
        ),
        "got:
{out}"
    );
    assert!(
        out.contains(
            "  stamp: 100.200
"
        ),
        "got:
{out}"
    );
    assert!(
        out.contains(
            "  frame_id: \"map\"
"
        ),
        "got:
{out}"
    );
    assert!(
        out.contains(
            "x: 1.25
"
        ),
        "got:
{out}"
    );
}

#[test]
fn decodes_fixed_and_variable_arrays() {
    let mut schema = MessageSchema::from_definition(
        "test_msgs/Arrays",
        "float32[3] fixed
int32[] var
",
    )
    .expect("definition parses");

    let mut wire = Vec::new();
    append_f32(&mut wire, 1.5);
    append_f32(&mut wire, 2.5);
    append_f32(&mut wire, 3.5);
    append_u32(&mut wire, 2);
    append_u32(&mut wire, 10);
    append_u32(&mut wire, 20);

    let mut out = String::new();
    let ok = decode_message(&mut out, "test_msgs/Arrays", &mut schema, &wire);

    assert!(ok);
    assert!(
        out.contains(
            "fixed: [1.5, 2.5, 3.5]
"
        ),
        "got:
{out}"
    );
    assert!(
        out.contains(
            "var: [10, 20]
"
        ),
        "got:
{out}"
    );
}

#[test]
fn a_definition_the_parser_rejects_is_an_error() {
    let rejected = MessageSchema::from_definition("test_msgs/Bad", "int32[ data\n");
    assert!(rejected.is_err());
    assert!(MessageSchema::from_definition("no_package", "int32 data\n").is_err());
}

const MIX_DEFINITION: &str = "string name
float64[3] xyz
test_msgs/Point[] cells
uint8[] blob
uint16[] many
int16 tail
================================================================================
MSG: test_msgs/Point
float32 x
float32 y
";

fn mix_wire() -> Vec<u8> {
    let mut wire = Vec::new();
    append_string(&mut wire, "ab");
    for value in [1.0, 2.0, 3.0] {
        append_f64(&mut wire, value);
    }
    append_u32(&mut wire, 2);
    for _ in 0..2 {
        append_f32(&mut wire, 0.5);
        append_f32(&mut wire, -1.0);
    }
    append_u32(&mut wire, 3);
    wire.extend_from_slice(&[1, 2, 3]);
    append_u32(&mut wire, 18);
    for value in 0_u16..18 {
        wire.extend_from_slice(&value.to_le_bytes());
    }
    wire.extend_from_slice(&(-7_i16).to_le_bytes());
    wire
}

#[test]
fn walk_spans_cover_every_byte_once() {
    let mut schema =
        MessageSchema::from_definition("test_msgs/Mix", MIX_DEFINITION).expect("definition parses");
    let wire = mix_wire();
    let mut spans = Vec::new();
    let consumed = walk(&mut schema, "test_msgs/Mix", &wire, &mut |event| match event {
        WalkEvent::Scalar { span, .. } | WalkEvent::Bytes { span, .. } => spans.push(span),
        WalkEvent::ArrayStart { prefix, .. } if !prefix.is_empty() => spans.push(prefix),
        _ => {}
    })
    .expect("walk");
    assert_eq!(consumed, wire.len());
    spans.sort_by_key(|span| span.start);
    let mut next = 0;
    for span in spans {
        assert_eq!(span.start, next, "gap or overlap before {span:?}");
        next = span.end;
    }
    assert_eq!(next, wire.len());
}

const MIX_OUTPUT: &str = "name: \"ab\"
xyz: [1, 2, 3]
cells:
  -
    x: 0.5
    y: -1
  -
    x: 0.5
    y: -1
blob: <3 bytes>
many: [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, ... (18 total)]
tail: -7
";

#[test]
fn decode_output_is_unchanged_by_the_walker() {
    let mut schema =
        MessageSchema::from_definition("test_msgs/Mix", MIX_DEFINITION).expect("definition parses");
    let wire = mix_wire();
    let mut out = String::new();
    assert!(decode_message(&mut out, "test_msgs/Mix", &mut schema, &wire));
    assert_eq!(out, MIX_OUTPUT);

    let truncated = wire.get(..wire.len() - 30).expect("prefix");
    let mut out = String::new();
    assert!(!decode_message(&mut out, "test_msgs/Mix", &mut schema, truncated));
    assert_eq!(out, MIX_TRUNCATED_OUTPUT);
}

#[test]
fn message_arrays_past_the_cap_are_walked_not_shown() {
    let definition = "test_msgs/Point[] cells
int16 tail
================================================================================
MSG: test_msgs/Point
float32 x
float32 y
";
    let mut schema =
        MessageSchema::from_definition("test_msgs/Cells", definition).expect("definition parses");
    let mut wire = Vec::new();
    append_u32(&mut wire, 17);
    for _ in 0..17 {
        append_f32(&mut wire, 1.0);
        append_f32(&mut wire, 2.0);
    }
    wire.extend_from_slice(&5_i16.to_le_bytes());
    let mut out = String::new();
    assert!(decode_message(&mut out, "test_msgs/Cells", &mut schema, &wire));
    let mut expected = String::from("cells:\n");
    for _ in 0..16 {
        expected.push_str("  -\n    x: 1\n    y: 2\n");
    }
    expected.push_str("  ... (17 total)\ntail: 5\n");
    assert_eq!(out, expected);
}

const MIX_TRUNCATED_OUTPUT: &str = "name: \"ab\"\nxyz: [1, 2, 3]\ncells:\n  -\n    x: 0.5\n    y: -1\n  -\n    x: 0.5\n    y: -1\nblob: <3 bytes>\nmany:<payload does not match the schema: field length 18 out of range for remaining buffer>\n";

/// Guard for the ROS 1 rule: `byte` is int8, `char` is uint8. The reflective printer already
/// reads them that way, so this test keeps it so.
#[test]
fn byte_prints_signed_and_char_unsigned() {
    let mut schema =
        MessageSchema::from_definition("pkg/Bytes", "byte b\nchar c\n").expect("definition parses");
    let mut out = String::new();
    assert!(decode_message(&mut out, "pkg/Bytes", &mut schema, &[0xC8, 0xC8]));
    assert!(out.contains("b: -56"), "{out}");
    assert!(out.contains("c: 200"), "{out}");
}

/// A recursive definition from the bus ends the walk with an error.
#[test]
fn a_recursive_definition_is_refused_not_overflowed() {
    for definition in [
        "Root child\nint32 a\n",
        "Root[] kids\n",
        "A a\n================================================================================\nMSG: pkg/A\nRoot r\n",
    ] {
        // A cycle is refused when the schema is built, before any walk.
        let Ok(mut schema) = MessageSchema::from_definition("pkg/Root", definition) else {
            continue;
        };
        // One element per level for the array case; the scalar cases read nothing.
        let wire: Vec<u8> = std::iter::repeat_n(1_u32.to_le_bytes(), 100_000).flatten().collect();
        let mut out = String::new();
        assert!(!decode_message(&mut out, "pkg/Root", &mut schema, &wire), "{definition}");
        assert!(out.contains("nest deeper than 64 levels"), "{definition}: {out}");
    }
}

/// A walk accepts the deepest nesting it allows and refuses one level more.
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
    assert_eq!(walk(&mut schema, "pkg/Root", &[7], &mut |_| {}), Ok(1));
    // One level more is refused when the schema is built, before any walk.
    let refused = chain(MAX_NESTING_DEPTH).map(|_| ()).expect_err("too deep");
    assert!(
        refused.contains(&format!("nests deeper than {MAX_NESTING_DEPTH} levels")),
        "{refused}"
    );
}

/// `pkg/Root` holds `pkg/T{j}` for every multiple of 60 below `levels`, descending, and
/// `pkg/T{i}` holds `pkg/T{i+1}`, the last holding a `uint8`.
fn shared_chain_schema(levels: usize) -> Result<MessageSchema, String> {
    let separator = "=".repeat(80);
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
    MessageSchema::from_definition("pkg/Root", &text)
}

#[test]
fn a_shared_chain_past_the_nesting_limit_is_refused_by_the_walk() {
    use hypha_node::sdk::msgdef::spec::MAX_NESTING_DEPTH;
    // The deepest path is checked when the schema is built, before any walk.
    let refused = shared_chain_schema(200).map(|_| ()).expect_err("too deep");
    assert!(
        refused.contains(&format!("nests deeper than {MAX_NESTING_DEPTH} levels")),
        "{refused}"
    );
}

#[test]
fn a_ladder_of_empty_messages_is_refused_before_walking() {
    let separator = "=".repeat(80);
    let mut text = String::new();
    for level in 0..64_usize {
        if level > 0 {
            let _ = write!(text, "\n{separator}\nMSG: p/T{level}\n");
        }
        let next = level + 1;
        if next < 64 {
            let _ = writeln!(text, "p/T{next} a\np/T{next} b");
        }
    }
    let refused = MessageSchema::from_definition("p/T0", &text);
    let message = refused.map(|_| ()).err().unwrap_or_default();
    assert!(message.contains("expands to more than 65536 nested messages"), "{message}");
}
