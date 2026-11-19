//! Reading TOML lists through the C ABI: the length, and each element or table field as stored.
#![allow(clippy::expect_used)]

mod common;
use crate::common::{
    config_list_value, create_node_with_file, empty_config_value, stored_text, unique,
};

use std::ffi::CString;

use hypha_ffi::*;

const LISTS: &str = "names = [\"imu\", \"gps\"]
counts = [1, -2]
rates = [0.5, 1.5]
flags = [true, false]
empty = []
port = 7

[[sensors]]
topic = \"/imu\"
rate = 100
limits.max = 5
mode = \"fast\"

[[sensors]]
topic = \"/gps\"
rate = 10
";

fn create_node(label: &str, parameters: &str) -> *mut HyphaNode {
    let path = std::env::temp_dir().join(unique("parameter_list.toml"));
    std::fs::write(&path, parameters).expect("write parameter fixture");
    let node = create_node_with_file(&unique(label), Some(&path));
    std::fs::remove_file(&path).expect("remove parameter fixture");
    node
}

fn key(text: &str) -> CString {
    CString::new(text).expect("name")
}

fn length(node: *const HyphaNode, list: &str) -> (HyphaParamResult, usize) {
    let list = key(list);
    let mut out = 77_usize;
    let result = unsafe {
        hypha_config_list_len(
            hypha_node_config(node),
            list.as_ptr(),
            list.as_bytes().len(),
            &raw mut out,
        )
    };
    (result, out)
}

/// Element `index` of `list`, or its `field`, as the C ABI stores it.
fn element(
    node: *const HyphaNode,
    list: &str,
    index: usize,
    field: &str,
) -> (HyphaParamResult, HyphaConfigValue) {
    config_list_value(unsafe { hypha_node_config(node) }, list, index, field)
}

#[test]
fn list_length_counts_tables_scalars_and_empty_lists() {
    let node = create_node("list_length", LISTS);
    assert_eq!(length(node, "sensors"), (HyphaParamResult::Ok, 2));
    assert_eq!(length(node, "names"), (HyphaParamResult::Ok, 2));
    assert_eq!(length(node, "empty"), (HyphaParamResult::Ok, 0), "an empty list has length zero");
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn a_missing_name_is_absent_and_a_scalar_name_is_malformed() {
    let node = create_node("list_missing", LISTS);
    assert_eq!(length(node, "never_set").0, HyphaParamResult::Absent);
    assert_eq!(length(node, "port"), (HyphaParamResult::Malformed, 77), "a scalar is not a list");
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn a_list_name_read_as_a_scalar_is_malformed() {
    let node = create_node("list_as_scalar", LISTS);
    let config = unsafe { hypha_node_config(node) };
    let name = "counts";
    let mut value = empty_config_value();
    value.kind = 99;
    let read =
        unsafe { hypha_config_value(config, name.as_ptr().cast(), name.len(), &raw mut value) };
    assert_eq!(read, HyphaParamResult::Malformed);
    assert_eq!(value.kind, 99, "a malformed read writes nothing");
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn scalar_elements_are_returned_with_their_stored_kind() {
    let node = create_node("scalar_elements", LISTS);
    let (result, int) = element(node, "counts", 1, "");
    assert_eq!(
        (result, int.kind, int.int_value),
        (HyphaParamResult::Ok, HYPHA_CONFIG_VALUE_INT, -2)
    );
    let (result, float) = element(node, "rates", 1, "");
    assert_eq!(
        (result, float.kind, float.float_value),
        (HyphaParamResult::Ok, HYPHA_CONFIG_VALUE_FLOAT, 1.5)
    );
    let (result, whole) = element(node, "counts", 0, "");
    assert_eq!(
        (result, whole.kind, whole.int_value),
        (HyphaParamResult::Ok, HYPHA_CONFIG_VALUE_INT, 1)
    );
    let (result, flag) = element(node, "flags", 0, "");
    assert_eq!(
        (result, flag.kind, flag.bool_value),
        (HyphaParamResult::Ok, HYPHA_CONFIG_VALUE_BOOL, 1)
    );
    let (result, text) = element(node, "names", 1, "");
    assert_eq!((result, text.kind), (HyphaParamResult::Ok, HYPHA_CONFIG_VALUE_TEXT));
    assert_eq!(stored_text(&text), b"gps");
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn table_element_fields_are_read_by_key_including_dotted_keys() {
    let node = create_node("table_fields", LISTS);
    let (result, topic) = element(node, "sensors", 0, "topic");
    assert_eq!((result, topic.kind), (HyphaParamResult::Ok, HYPHA_CONFIG_VALUE_TEXT));
    assert_eq!(stored_text(&topic), b"/imu");
    let (result, rate) = element(node, "sensors", 1, "rate");
    assert_eq!(
        (result, rate.kind, rate.int_value),
        (HyphaParamResult::Ok, HYPHA_CONFIG_VALUE_INT, 10)
    );
    let (result, max) = element(node, "sensors", 0, "limits.max");
    assert_eq!(
        (result, max.kind, max.int_value),
        (HyphaParamResult::Ok, HYPHA_CONFIG_VALUE_INT, 5),
        "a dotted field is a flat key, as for the scalar reads"
    );
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn an_absent_index_or_field_is_absent_and_writes_nothing() {
    let node = create_node("absent_fields", LISTS);
    let config = unsafe { hypha_node_config(node) };
    for (list, index, field, label) in [
        ("counts", 2, "", "index past the end"),
        ("never_set", 0, "", "missing list"),
        ("sensors", 1, "limits.max", "field not in this element"),
        ("sensors", 0, "no_such_field", "unknown field"),
        ("counts", 0, "field_on_scalar", "a scalar element has no fields"),
        ("sensors", 5, "topic", "index past the end of a table list"),
    ] {
        let (result, value) = config_list_value(config, list, index, field);
        assert_eq!(result, HyphaParamResult::Absent, "{label}");
        assert_eq!(value.kind, u32::MAX, "{label}: an absent read writes nothing");
    }
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn a_table_element_read_as_a_scalar_is_malformed() {
    let node = create_node("table_as_scalar", LISTS);
    let (result, value) = element(node, "sensors", 0, "");
    assert_eq!(result, HyphaParamResult::Malformed, "a table is not a value");
    assert_eq!(value.kind, u32::MAX, "a malformed read writes nothing");
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn a_text_element_borrows_its_bytes_and_reports_its_length() {
    let node = create_node("text_size", LISTS);
    let (result, text) = element(node, "names", 0, "");
    assert_eq!(
        (result, text.kind, text.text_len),
        (HyphaParamResult::Ok, HYPHA_CONFIG_VALUE_TEXT, 3)
    );
    assert_eq!(stored_text(&text), b"imu");
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn a_length_name_that_is_not_utf8_is_an_argument_error_not_an_absent_list() {
    let node = create_node("length_invalid_name", LISTS);
    let invalid = [0xff_u8];
    let mut len = 77_usize;

    assert_eq!(
        unsafe {
            hypha_config_list_len(
                hypha_node_config(node),
                invalid.as_ptr().cast(),
                invalid.len(),
                &raw mut len,
            )
        },
        HyphaParamResult::InvalidArgument,
        "a name that is not UTF-8 is refused, the same as the list read"
    );
    assert_eq!(len, 77, "a refused name writes no length");
    unsafe { hypha_node_destroy(node) };
}
