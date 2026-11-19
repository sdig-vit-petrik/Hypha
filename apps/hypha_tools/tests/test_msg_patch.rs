//! Override values and the byte-level message patcher.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use std::collections::{BTreeMap, BTreeSet};

use hypha_tools::msg_patch::{
    Override, Radix, ScalarValue, apply, encode_scalar, format_value, index_slots, parse_value,
    read_scalar,
};
use hypha_tools::msg_reflect::{MessageSchema, decode_message};

const ROOT: &str = "test_msgs/Battery";
const DEFINITION: &str = "uint16 status
string serial_number
test_msgs/Cell[] cells
uint8[] blob
float32 temperature
================================================================================
MSG: test_msgs/Cell
float32 voltage
int32 count
";

fn schema() -> MessageSchema {
    MessageSchema::from_definition(ROOT, DEFINITION).expect("definition parses")
}

fn battery(status: u16, serial: &str, cells: usize) -> Vec<u8> {
    let mut wire = status.to_le_bytes().to_vec();
    wire.extend_from_slice(&u32::try_from(serial.len()).unwrap().to_le_bytes());
    wire.extend_from_slice(serial.as_bytes());
    wire.extend_from_slice(&u32::try_from(cells).unwrap().to_le_bytes());
    for index in 0..cells {
        wire.extend_from_slice(&3.5_f32.to_le_bytes());
        wire.extend_from_slice(&i32::try_from(index).unwrap().to_le_bytes());
    }
    wire.extend_from_slice(&3_u32.to_le_bytes());
    wire.extend_from_slice(&[7, 8, 9]);
    wire.extend_from_slice(&21.5_f32.to_le_bytes());
    wire
}

fn overrides(entries: &[(&str, Override)]) -> BTreeMap<String, Override> {
    entries.iter().map(|(path, value)| ((*path).to_owned(), value.clone())).collect()
}

fn decoded(wire: &[u8]) -> String {
    let mut out = String::new();
    assert!(decode_message(&mut out, ROOT, &mut schema(), wire), "{out}");
    out
}

#[test]
fn parse_value_accepts_dec_hex_bin_and_negative() {
    assert_eq!(parse_value("int16", "-0x10"), Ok(ScalarValue::Int(-16)));
    assert_eq!(parse_value("uint8", "0b101"), Ok(ScalarValue::UInt(5)));
    assert_eq!(parse_value("int32", "42"), Ok(ScalarValue::Int(42)));
    assert_eq!(parse_value("uint64", "0xFFFFFFFFFFFFFFFF"), Ok(ScalarValue::UInt(u64::MAX)));
    assert_eq!(parse_value("float64", "-2.5"), Ok(ScalarValue::Float(-2.5)));
}

#[test]
fn parse_value_refuses_out_of_range() {
    for (type_name, text) in
        [("uint8", "256"), ("int8", "-129"), ("float32", "1e39"), ("bool", "2"), ("uint16", "-1")]
    {
        let error = parse_value(type_name, text).expect_err(text);
        assert!(error.contains(type_name), "{error}");
    }
}

#[test]
fn parse_value_parses_strings_time_and_bools() {
    assert_eq!(parse_value("string", "ZZZ\\x00"), Ok(ScalarValue::Text(b"ZZZ\0".to_vec())));
    assert_eq!(parse_value("time", "12.000000500"), Ok(ScalarValue::Time(12, 500)));
    assert_eq!(parse_value("duration", "-1.5"), Ok(ScalarValue::Duration(-1, -500_000_000)));
    assert_eq!(parse_value("bool", "true"), Ok(ScalarValue::Bool(true)));
    assert!(parse_value("string", "\\x4").is_err(), "an escape needs two hex digits");
}

#[test]
fn encode_then_read_round_trips_every_builtin() {
    let cases = [
        ("bool", ScalarValue::Bool(true)),
        ("int8", ScalarValue::Int(-8)),
        ("byte", ScalarValue::Int(-1)),
        ("uint8", ScalarValue::UInt(200)),
        ("char", ScalarValue::UInt(65)),
        ("int16", ScalarValue::Int(-300)),
        ("uint16", ScalarValue::UInt(60_000)),
        ("int32", ScalarValue::Int(-70_000)),
        ("uint32", ScalarValue::UInt(4_000_000_000)),
        ("int64", ScalarValue::Int(i64::MIN)),
        ("uint64", ScalarValue::UInt(u64::MAX)),
        ("float32", ScalarValue::Float(0.5)),
        ("float64", ScalarValue::Float(-1e300)),
        ("string", ScalarValue::Text(b"a\0b".to_vec())),
        ("time", ScalarValue::Time(1, 2)),
        ("duration", ScalarValue::Duration(-3, -4)),
    ];
    for (type_name, value) in cases {
        let wire = encode_scalar(type_name, &value).expect(type_name);
        assert_eq!(read_scalar(type_name, &wire), Ok(value), "{type_name}");
    }
    assert!(encode_scalar("uint8", &ScalarValue::UInt(256)).is_err());
}

#[test]
fn format_value_radixes() {
    assert_eq!(format_value(&ScalarValue::UInt(5), Radix::Dec), "5");
    assert_eq!(format_value(&ScalarValue::UInt(5), Radix::Hex), "0x5");
    assert_eq!(format_value(&ScalarValue::UInt(5), Radix::Bin), "0b101");
    assert_eq!(format_value(&ScalarValue::Int(-16), Radix::Hex), "-0x10");
    assert_eq!(format_value(&ScalarValue::Float(3.5), Radix::Hex), "3.5");
    assert_eq!(format_value(&ScalarValue::Text(b"ZZZ\0".to_vec()), Radix::Dec), "ZZZ\\x00");
    assert_eq!(format_value(&ScalarValue::Time(12, 500), Radix::Dec), "12.000000500");
    assert_eq!(format_value(&ScalarValue::Duration(-1, -500_000_000), Radix::Dec), "-1.500000000");
}

#[test]
fn index_slots_names_paths_like_the_file() {
    let wire = battery(0, "AB", 3);
    let slots = index_slots(&mut schema(), ROOT, &wire, None).expect("index");
    let paths: Vec<&str> = slots.iter().map(|slot| slot.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "status",
            "serial_number",
            "cells[0].voltage",
            "cells[0].count",
            "cells[1].voltage",
            "cells[1].count",
            "cells[2].voltage",
            "cells[2].count",
            "temperature",
        ],
        "byte arrays have no slot"
    );
    assert_eq!(slots[1].span, 2..8, "a string span holds its length");

    let want = BTreeSet::from(["cells[1].count".to_owned()]);
    let only = index_slots(&mut schema(), ROOT, &wire, Some(&want)).expect("index");
    assert_eq!(only.len(), 1);
    assert_eq!(only[0].type_name, "int32");
}

#[test]
fn apply_value_and_bits() {
    let input = battery(0x00F0, "AB", 1);
    let wanted = overrides(&[
        ("status", Override::Bits { set: 0x0002, clear: 0x0010 }),
        ("cells[0].count", Override::Value("7".to_owned())),
    ]);
    let mut out = Vec::new();
    let report = apply(&mut schema(), ROOT, &input, &wanted, &mut out).expect("apply");
    assert_eq!(report.applied, 2);
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    let mut expected = battery(0x00E2, "AB", 1);
    expected[16..20].copy_from_slice(&7_i32.to_le_bytes());
    assert_eq!(out, expected);
}

#[test]
fn bits_on_a_non_integer_fails_that_path_only() {
    let input = battery(1, "AB", 1);
    let wanted = overrides(&[
        ("temperature", Override::Bits { set: 1, clear: 0 }),
        ("status", Override::Value("3".to_owned())),
    ]);
    let mut out = Vec::new();
    let report = apply(&mut schema(), ROOT, &input, &wanted, &mut out).expect("apply");
    assert_eq!(report.applied, 1);
    assert_eq!(report.failed.len(), 1);
    assert_eq!(report.failed[0].0, "temperature");
    assert_eq!(out, battery(3, "AB", 1));
}

#[test]
fn missing_path_is_reported_and_other_overrides_still_apply() {
    let input = battery(0, "AB", 3);
    let wanted = overrides(&[
        ("cells[5].voltage", Override::Value("0.0".to_owned())),
        ("status", Override::Value("9".to_owned())),
    ]);
    let mut out = Vec::new();
    let report = apply(&mut schema(), ROOT, &input, &wanted, &mut out).expect("apply");
    assert_eq!(report.applied, 1);
    assert_eq!(
        report.failed,
        vec![("cells[5].voltage".to_owned(), "<no such field in this message>".to_owned())]
    );
    assert_eq!(out, battery(9, "AB", 3));
}

#[test]
fn relay_publishes_a_string_longer_than_the_input() {
    let input = battery(4, "AB", 2);
    let wanted = overrides(&[("serial_number", Override::Value("ABCDEFGH".to_owned()))]);
    let mut out = Vec::new();
    let report = apply(&mut schema(), ROOT, &input, &wanted, &mut out).expect("apply");
    assert_eq!(report.applied, 1);
    assert_eq!(out.len(), input.len() + 6);
    assert_eq!(out, battery(4, "ABCDEFGH", 2));
    let text = decoded(&out);
    assert!(text.contains("serial_number: \"ABCDEFGH\"\n"), "{text}");
    assert!(text.contains("temperature: 21.5\n"), "{text}");
}

#[test]
fn apply_on_a_truncated_payload_is_err() {
    let input = battery(4, "AB", 2);
    let wanted = overrides(&[("status", Override::Value("1".to_owned()))]);
    let mut out = Vec::new();
    assert!(apply(&mut schema(), ROOT, &input[..input.len() - 2], &wanted, &mut out).is_err());
    let mut longer = input.clone();
    longer.push(0);
    assert!(apply(&mut schema(), ROOT, &longer, &wanted, &mut out).is_err(), "trailing bytes");
}
