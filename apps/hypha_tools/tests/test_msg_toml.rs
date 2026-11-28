//! A message as TOML against a runtime schema, as `hypha service call` takes a request,
//! prints its template and prints the response.
#![allow(clippy::expect_used)]

use hypha_tools::msg_reflect::MessageSchema;
use hypha_tools::msg_toml::{decode_message, encode_message, parse_request, template};

const GOAL: &str = "string frame\nPoint position\nfloat64[2] range\nint32[] ids\ntime stamp\n\
bool armed\nuint8[] blob\nPoint[] path\nfloat32 gain\n\
================================================================================\n\
MSG: test_msgs/Point\nfloat64 x\nfloat64 y\n";

fn schema() -> MessageSchema {
    MessageSchema::from_definition("test_msgs/Goal", GOAL).expect("schema")
}

/// The request `text` encoded, then printed back.
fn shown(text: &str) -> Result<String, String> {
    let mut schema = schema();
    let bytes = encode_message(&mut schema, "test_msgs/Goal", &parse_request(text)?)?;
    decode_message(&mut schema, "test_msgs/Goal", &bytes)
}

const FULL: &str = "frame = \"map\"\nposition.x = 1.5\nposition.y = -2.0\nrange = [0.0, 10.0]\n\
ids = [3, 4]\nstamp = { secs = 7, nsecs = 500000000 }\narmed = true\nblob = [104, 105]\n\
path = [{ x = 1.0, y = 2.0 }, { x = 3.0, y = 4.0 }]\ngain = 0.1\n";

#[test]
fn inline_tables_and_dotted_keys_encode_the_same_message() {
    let dotted = "frame = 'map'\nposition.x = 1.5\nposition.y = -2   # metres\n\
range = [0, 10]\nids = [3, 4]\nstamp.secs = 7\nstamp.nsecs = 500_000_000\narmed = true\n\
blob = 'hi'\npath = [{ x = 1, y = 2 }, { x = 3, y = 4 }]\ngain = 0.1";
    let inline = "frame = \"map\"\nposition = { x = 1.5, y = -2.0 }\nrange = [0.0, 10.0]\n\
ids = [3, 4]\nstamp = { secs = 7, nsecs = 500000000 }\narmed = true\nblob = [104, 105]\n\
[[path]]\nx = 1.0\ny = 2.0\n[[path]]\nx = 3.0\ny = 4.0\n";
    assert_eq!(shown(dotted).expect("dotted"), FULL);
    assert_eq!(shown(&format!("gain = 0.1\n{inline}")).expect("inline"), FULL);
    // What is printed is a request again.
    assert_eq!(shown(FULL).expect("printed"), FULL);
}

#[test]
fn a_field_left_out_takes_its_zero_value() {
    assert_eq!(
        shown("position = { x = 1 }").expect("encodes"),
        "frame = \"\"\nposition.x = 1.0\nposition.y = 0.0\nrange = [0.0, 0.0]\nids = []\n\
stamp = { secs = 0, nsecs = 0 }\narmed = false\nblob = []\npath = []\ngain = 0.0\n"
    );
    assert!(parse_request("  ").expect("empty").is_empty());
}

#[test]
fn a_request_that_does_not_fit_names_the_field() {
    let refused = |text: &str| shown(text).expect_err(text);
    assert_eq!(
        refused("position = { z = 1 }"),
        "test_msgs/Point has no field 'position.z'; its fields: x, y"
    );
    assert_eq!(refused("range = [1, 2, 3]"), "range: a float64[2] takes 2 values, not 3");
    assert_eq!(
        refused("ids = [1, 'x']"),
        "ids[1]: expected an integer (int32), found the string \"x\""
    );
    assert_eq!(refused("ids = [2147483648]"), "ids[0]: 2147483648 is out of range for int32");
    assert_eq!(refused("ids = [1.5]"), "ids[0]: expected an integer (int32), found the float 1.5");
    assert_eq!(
        refused("armed = 'true'"),
        "armed: expected true or false, found the string \"true\""
    );
    assert_eq!(refused("frame = 3"), "frame: expected a string in quotes, found the integer 3");
    assert_eq!(
        refused("position = 3"),
        "position: a test_msgs/Point is a table of its fields, as { x = 1.0 }, not the integer 3"
    );
    assert_eq!(refused("stamp = { sec = 1 }"), "stamp: a time has secs and nsecs, not 'sec'");
    assert_eq!(refused("stamp = { secs = -1 }"), "stamp: secs is out of range for a time");
    assert!(refused("gain = 1e39").contains("is out of range for float32"));
    // Not TOML: the crate's reason, with the line.
    let not_toml = refused("data: true");
    assert!(not_toml.contains("line 1"), "{not_toml}");
}

#[test]
fn a_string_is_toml_text_with_its_escapes() {
    let shown = shown(r#"frame = "a\"b\u00e9\n\u0001""#).expect("encodes");
    assert!(shown.starts_with(r#"frame = "a\"bé\n\u0001""#), "{shown}");
    assert!(self::shown("frame = 'it''s'").is_err(), "TOML has no '' escape");
}

#[test]
fn the_template_lists_every_field_at_its_zero_and_encodes() {
    let mut schema = schema();
    let text = template(&mut schema, "test_msgs/Goal");
    assert_eq!(
        text,
        "frame = \"\"\nposition.x = 0.0\nposition.y = 0.0\nrange = [0.0, 0.0]\nids = []\n\
stamp = { secs = 0, nsecs = 0 }\narmed = false\nblob = []\npath = []\ngain = 0.0\n"
    );
    assert_eq!(shown(&text).expect("the template encodes"), text);
    let mut empty = MessageSchema::from_definition("std_srvs/TriggerRequest", "").expect("schema");
    assert_eq!(template(&mut empty, "std_srvs/TriggerRequest"), "");
    let mut fixed = MessageSchema::from_definition(
        "test_msgs/Pair",
        "test_msgs/Point[2] ends\n\
================================================================================\n\
MSG: test_msgs/Point\nfloat64 x\nfloat64 y\n",
    )
    .expect("schema");
    assert_eq!(
        template(&mut fixed, "test_msgs/Pair"),
        "ends = [{ x = 0.0, y = 0.0 }, { x = 0.0, y = 0.0 }]\n"
    );
}

#[test]
fn a_payload_that_is_not_the_message_is_refused() {
    let mut schema = schema();
    assert!(decode_message(&mut schema, "test_msgs/Goal", &[1, 0]).is_err());
}
