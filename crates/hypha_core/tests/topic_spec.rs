//! Tests for topic specification construction and validation.

use hypha_core::port::TopicSpec;

const VALID_MD5: &str = "0123456789abcdef0123456789abcdef";

#[test]
fn new_topic_has_documented_defaults() {
    let spec = TopicSpec::new("/sensor/data");

    assert_eq!(spec.name, "/sensor/data");
    assert_eq!(spec.buffer_size, None);
    assert_eq!(spec.data_type, "");
    assert_eq!(spec.md5, "");
    assert_eq!(spec.definition, "");
    assert!(!spec.latch);
}

#[test]
fn builders_set_each_configurable_value() {
    let spec = TopicSpec::new("/sensor/data")
        .latched()
        .keeping(8)
        .with_type("std_msgs/String")
        .with_md5(VALID_MD5)
        .with_definition("string data");

    assert!(spec.latch);
    assert_eq!(spec.buffer_size, Some(8));
    assert_eq!(spec.data_type, "std_msgs/String");
    assert_eq!(spec.md5, VALID_MD5);
    assert_eq!(spec.definition, "string data");
}

#[test]
fn valid_public_topic_names_are_accepted() {
    for name in ["/cmd_vel", "/robot/left_arm/joint_states", "/sensor_1/scan_raw", "/hypha/node"] {
        assert!(TopicSpec::new(name).has_valid_public_name(), "{name:?} must be accepted");
    }
}

#[test]
fn invalid_public_topic_names_are_rejected() {
    let invalid_names = [
        "",
        "/",
        "cmd_vel",
        "/hypha",
        "/*",
        "/robot/*",
        "//",
        "/robot//cmd",
        "/cmd/",
        "/.",
        "/robot/./cmd",
        "/..",
        "/robot/../cmd",
        "/cmd vel",
        "/cmd\tvel",
        "/cmd\nvel",
        "/cmd\0vel",
    ];

    for name in invalid_names {
        assert!(!TopicSpec::new(name).has_valid_public_name(), "{name:?} must be rejected");
    }
}

#[test]
fn schema_identity_requires_a_type_and_lowercase_hex_md5() {
    let valid = TopicSpec::new("/schema").with_type("pkg/Message").with_md5(VALID_MD5);
    assert!(valid.has_valid_schema_identity());

    let invalid_specs = [
        TopicSpec::new("/schema").with_md5(VALID_MD5),
        TopicSpec::new("/schema").with_type("pkg/Message"),
        TopicSpec::new("/schema")
            .with_type("pkg/Message")
            .with_md5("0123456789abcdef0123456789abcde"),
        TopicSpec::new("/schema")
            .with_type("pkg/Message")
            .with_md5("0123456789abcdef0123456789abcdef0"),
        TopicSpec::new("/schema")
            .with_type("pkg/Message")
            .with_md5("0123456789abcdef0123456789abcdeF"),
        TopicSpec::new("/schema")
            .with_type("pkg/Message")
            .with_md5("0123456789abcdef0123456789abcdeg"),
    ];

    for spec in invalid_specs {
        assert!(!spec.has_valid_schema_identity());
    }
}
