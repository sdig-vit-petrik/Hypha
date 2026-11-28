//! A message from YAML or JSON text, encoded against a runtime schema, as
//! `hypha service call` takes a request.
#![allow(clippy::expect_used)]

use hypha_tools::msg_reflect::{MessageSchema, decode_message};
use hypha_tools::msg_text::{TextValue, encode_message, parse_text, template};

const POSE: &str = "string frame\nPoint position\nfloat64[2] range\nint32[] ids\ntime stamp\n\
================================================================================\n\
MSG: test_msgs/Point\nfloat64 x\nfloat64 y\n";

fn schema() -> MessageSchema {
    MessageSchema::from_definition("test_msgs/Goal", POSE).expect("schema")
}

fn shown(text: &str) -> Result<String, String> {
    let mut schema = schema();
    let bytes = encode_message(&mut schema, "test_msgs/Goal", &parse_text(text)?)?;
    let mut out = String::new();
    assert!(decode_message(&mut out, "test_msgs/Goal", &mut schema, &bytes));
    Ok(out)
}

const FULL: &str =
    "frame: \"map\"\nposition:\n  x: 1.5\n  y: -2\nrange: [0, 10]\nids: [3, 4]\nstamp: 7.5\n";

#[test]
fn block_yaml_flow_yaml_and_json_encode_the_same_message() {
    let block = "frame: map\nposition:\n  x: 1.5\n  y: -2   # metres\nrange: [0, 10]\nids:\n  - 3\n  - 4\nstamp: 7.5";
    let flow = "{frame: map, position: {x: 1.5, y: -2}, range: [0, 10], ids: [3, 4], stamp: {secs: 7, nsecs: 500000000}}";
    let json = r#"{"frame": "map", "position": {"x": 1.5, "y": -2}, "range": [0, 10], "ids": [3, 4], "stamp": 7.5}"#;
    for text in [block, flow, json] {
        assert_eq!(shown(text).expect(text).replace("7.500000000", "7.5"), FULL, "{text}");
    }
}

#[test]
fn a_field_left_out_takes_its_zero_value() {
    let shown = shown("position: {x: 1}").expect("encodes");
    assert_eq!(
        shown,
        "frame: \"\"\nposition:\n  x: 1\n  y: 0\nrange: [0, 0]\nids: []\nstamp: 0.0\n"
    );
    assert_eq!(parse_text("  ").expect("empty"), TextValue::Map(Vec::new()));
}

#[test]
fn a_text_that_does_not_fit_names_the_field() {
    let refused = |text: &str| shown(text).expect_err(text);
    assert_eq!(
        refused("{position: {z: 1}}"),
        "test_msgs/Point has no field 'position.z'; its fields: x, y"
    );
    assert!(refused("range: [1, 2, 3]").contains("range: a float64[2] takes 2 values, not 3"));
    assert!(refused("ids: [1, x]").contains("ids[1]: 'x' is not a int32"));
    assert!(refused("position: 3").contains("position: a test_msgs/Point is a mapping"));
    assert!(refused("{frame: map").contains("expected"));
}

#[test]
fn a_quoted_value_is_text_and_json_escapes_are_read() {
    let shown = shown(r#"{"frame": "a\"bé"}"#).expect("encodes");
    assert!(shown.starts_with("frame: \"a\"bé\"\n"), "{shown}");
    assert!(self::shown("frame: 'it''s'").expect("encodes").starts_with("frame: \"it's\"\n"));
}

#[test]
fn the_template_lists_every_field_at_its_zero() {
    let mut schema = schema();
    let text = template(&mut schema, "test_msgs/Goal");
    assert_eq!(
        text,
        "frame: \"\"\nposition:\n  x: 0.0\n  y: 0.0\nrange: [0.0, 0.0]\nids: []\nstamp: {secs: 0, nsecs: 0}\n"
    );
    assert_eq!(shown(&text).expect("the template encodes"), shown("").expect("empty"));
    let mut empty = MessageSchema::from_definition("std_srvs/TriggerRequest", "").expect("schema");
    assert_eq!(template(&mut empty, "std_srvs/TriggerRequest"), "{}\n");
}
