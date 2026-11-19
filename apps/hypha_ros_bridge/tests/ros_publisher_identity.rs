//! Validation of the type identity a ROS 1 publisher announces in its connection header.

#![allow(clippy::expect_used)]

use hypha_ros_bridge::publisher_identity::{
    RosIdentity, agreed_identity, encode_probe_header, identity_from_header,
};
use hypha_ros_bridge::{BridgeError, strip_ros_packet};

const TYPE_NAME: &str = "positng_msg/VehKinem";
const MD5: &str = "4a849f3d44642e769fbedbf3b987cbcc";
const DEFINITION: &str = "float64 speed\nfloat64 distance\n";

fn header(fields: &[(&str, &str)]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for (key, value) in fields {
        let field = format!("{key}={value}");
        let field_len = u32::try_from(field.len()).expect("field length");
        bytes.extend_from_slice(&field_len.to_le_bytes());
        bytes.extend_from_slice(field.as_bytes());
    }
    bytes
}

fn publisher_header(type_name: &str, md5: &str, definition: &str) -> Vec<u8> {
    header(&[
        ("callerid", "/posit_ng"),
        ("latching", "0"),
        ("md5sum", md5),
        ("message_definition", definition),
        ("topic", "/posit_ng/vehkinem"),
        ("type", type_name),
    ])
}

fn identity(md5: &str) -> RosIdentity {
    RosIdentity { type_name: TYPE_NAME.into(), md5: md5.into(), definition: DEFINITION.into() }
}

fn is_identity_error(outcome: &Result<RosIdentity, BridgeError>) -> bool {
    matches!(outcome, Err(BridgeError::Identity(_)))
}

#[test]
fn a_complete_publisher_header_yields_its_identity() {
    let read = identity_from_header(&publisher_header(TYPE_NAME, MD5, DEFINITION))
        .expect("complete header");

    assert_eq!(read, identity(MD5));
}

#[test]
fn a_definition_containing_equals_signs_is_kept_whole() {
    let definition = "uint8 MODE_A=1\nuint8 mode\n";

    let read = identity_from_header(&publisher_header(TYPE_NAME, MD5, definition))
        .expect("header with constants");

    assert_eq!(read.definition, definition);
}

#[test]
fn a_message_without_fields_has_an_empty_definition() {
    let read = identity_from_header(&publisher_header(
        "std_msgs/Empty",
        "d41d8cd98f00b204e9800998ecf8427e",
        "",
    ))
    .expect("empty message");

    assert_eq!(read.definition, "");
}

#[test]
fn a_publisher_error_refuses_the_topic() {
    let refused = header(&[("error", "topic not published")]);

    assert!(is_identity_error(&identity_from_header(&refused)));
}

#[test]
fn each_missing_identity_field_refuses_the_topic() {
    let without_md5 = header(&[("type", TYPE_NAME), ("message_definition", DEFINITION)]);
    let without_type = header(&[("md5sum", MD5), ("message_definition", DEFINITION)]);
    let without_definition = header(&[("type", TYPE_NAME), ("md5sum", MD5)]);

    assert!(is_identity_error(&identity_from_header(&without_md5)));
    assert!(is_identity_error(&identity_from_header(&without_type)));
    assert!(is_identity_error(&identity_from_header(&without_definition)));
}

#[test]
fn a_wildcard_or_malformed_md5_refuses_the_topic() {
    for md5 in [
        "*",
        "",
        "00000000",
        "4a849f3d44642e769fbedbf3b987cbcz",
        "4a849f3d44642e769fbedbf3b987cbcc0",
    ] {
        let outcome = identity_from_header(&publisher_header(TYPE_NAME, md5, DEFINITION));
        assert!(is_identity_error(&outcome), "md5 '{md5}' must be refused");
    }
}

#[test]
fn a_wildcard_or_empty_type_refuses_the_topic() {
    for type_name in ["*", ""] {
        let outcome = identity_from_header(&publisher_header(type_name, MD5, DEFINITION));
        assert!(is_identity_error(&outcome), "type '{type_name}' must be refused");
    }
}

#[test]
fn a_truncated_or_malformed_header_refuses_the_topic() {
    let complete = publisher_header(TYPE_NAME, MD5, DEFINITION);
    let truncated = complete.get(..complete.len().saturating_sub(3)).expect("truncated header");
    let partial_length = [7_u8, 0];
    let mut without_separator = publisher_header(TYPE_NAME, MD5, DEFINITION);
    without_separator.extend_from_slice(&4_u32.to_le_bytes());
    without_separator.extend_from_slice(b"oops");

    assert!(is_identity_error(&identity_from_header(truncated)));
    assert!(is_identity_error(&identity_from_header(&partial_length)));
    assert!(is_identity_error(&identity_from_header(&without_separator)));
}

#[test]
fn a_repeated_field_refuses_the_topic() {
    let mut repeated = publisher_header(TYPE_NAME, MD5, DEFINITION);
    repeated.extend_from_slice(&header(&[("md5sum", "fedcba9876543210fedcba9876543210")]));

    assert!(is_identity_error(&identity_from_header(&repeated)));
}

#[test]
fn publishers_that_agree_give_their_identity() {
    let agreed = agreed_identity(TYPE_NAME, &[identity(MD5), identity(MD5)]).expect("agreement");

    assert_eq!(agreed, identity(MD5));
}

#[test]
fn a_master_without_a_listed_type_accepts_the_publisher_identity() {
    let agreed = agreed_identity("", &[identity(MD5)]).expect("agreement");

    assert_eq!(agreed.type_name, TYPE_NAME);
}

#[test]
fn publishers_with_different_schemas_refuse_the_topic() {
    let outcome =
        agreed_identity(TYPE_NAME, &[identity(MD5), identity("fedcba9876543210fedcba9876543210")]);

    assert!(is_identity_error(&outcome));
}

#[test]
fn a_master_type_that_differs_from_the_publisher_refuses_the_topic() {
    let outcome = agreed_identity("nav_msgs/Odometry", &[identity(MD5)]);

    assert!(is_identity_error(&outcome));
}

#[test]
fn no_publisher_identity_refuses_the_topic() {
    assert!(is_identity_error(&agreed_identity(TYPE_NAME, &[])));
}

#[test]
fn the_probe_header_asks_any_publisher_for_its_identity() {
    let packet = encode_probe_header("/hypha_ros_bridge", "/posit_ng/vehkinem").expect("probe");
    let body = strip_ros_packet(&packet).expect("length prefix");

    let expected = header(&[
        ("callerid", "/hypha_ros_bridge"),
        ("topic", "/posit_ng/vehkinem"),
        ("md5sum", "*"),
        ("type", "*"),
    ]);
    assert_eq!(body, expected.as_slice());
}
