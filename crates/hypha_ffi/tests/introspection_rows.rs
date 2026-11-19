//! The node introspection rows and the snapshot call.
#![allow(clippy::expect_used, clippy::too_many_lines)]

mod common;
use crate::common::c_text;
use crate::common::create_node;
use crate::common::introspection::introspection_text;
use crate::common::unique;

use std::ptr;

use hypha_ffi::*;

const MD5: &str = "d9d213e8c146065ec958333c617eb1a6";
const DATA_TYPE: &str = "std_msgs/Counter";

/// The line of `text` that starts with `topic`, split into its cells.
fn row_cells(text: &str, topic: &str) -> Option<Vec<String>> {
    text.lines()
        .find(|line| line.trim_start().starts_with(topic) && line.starts_with("  "))
        .map(|line| line.split_whitespace().map(str::to_owned).collect())
}

fn publish(node: *mut HyphaNode, topic: &str) -> *mut HyphaPublisher {
    let topic = c_text(topic);
    let data_type = c_text(DATA_TYPE);
    let md5 = c_text(MD5);
    let definition = c_text("");
    let qos = HyphaPublisherOptions { latch: 0, loaned_samples: 0, qos: crate::common::test_qos() };
    let mut publisher = ptr::null_mut();
    let status = unsafe {
        hypha_publish_with_qos(
            node,
            topic.as_ptr(),
            topic.as_bytes().len(),
            data_type.as_ptr(),
            data_type.as_bytes().len(),
            md5.as_ptr(),
            md5.as_bytes().len(),
            definition.as_ptr(),
            0,
            &raw const qos,
            &raw mut publisher,
        )
    };
    assert_eq!(status, HyphaStatus::Ok);
    publisher
}

fn subscriber_options() -> HyphaSubscriberOptions {
    HyphaSubscriberOptions {
        depth: 4,
        no_publisher_warning_after_nanos: 0,
        qos: crate::common::test_qos(),
    }
}

fn subscribe_typed(node: *mut HyphaNode, topic: &str) -> *mut HyphaSubscriber {
    let topic = c_text(topic);
    let data_type = c_text(DATA_TYPE);
    let md5 = c_text(MD5);
    let qos = subscriber_options();
    let mut subscriber = ptr::null_mut();
    let status = unsafe {
        hypha_subscribe_typed(
            node,
            topic.as_ptr(),
            topic.as_bytes().len(),
            data_type.as_ptr(),
            data_type.as_bytes().len(),
            md5.as_ptr(),
            md5.as_bytes().len(),
            std::ptr::null(),
            0,
            &raw const qos,
            &raw mut subscriber,
        )
    };
    assert_eq!(status, HyphaStatus::Ok);
    subscriber
}

fn subscribe_untyped(node: *mut HyphaNode, topic: &str) -> *mut HyphaSubscriber {
    let topic = c_text(topic);
    let md5 = c_text(MD5);
    let qos = subscriber_options();
    let mut subscriber = ptr::null_mut();
    let status = unsafe {
        hypha_subscribe_typed(
            node,
            topic.as_ptr(),
            topic.as_bytes().len(),
            std::ptr::null(),
            0,
            md5.as_ptr(),
            md5.as_bytes().len(),
            std::ptr::null(),
            0,
            &raw const qos,
            &raw mut subscriber,
        )
    };
    assert_eq!(status, HyphaStatus::Ok);
    subscriber
}

#[test]
fn a_publisher_row_appears_on_creation_and_leaves_on_destruction() {
    let node = create_node(&unique("introspection_pub"));
    let topic = format!("/{}", unique("pub"));
    assert!(row_cells(&introspection_text(node), &topic).is_none());
    let publisher = publish(node, &topic);
    let cells = row_cells(&introspection_text(node), &topic).expect("publisher row");
    assert_eq!(cells, [topic.as_str(), DATA_TYPE, "-"]);
    unsafe { hypha_publisher_destroy(publisher) };
    assert!(row_cells(&introspection_text(node), &topic).is_none());
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn a_typed_subscriber_row_shows_its_message_type_and_an_untyped_one_shows_a_dash() {
    let node = create_node(&unique("introspection_sub"));
    let typed_topic = format!("/{}", unique("typed"));
    let untyped_topic = format!("/{}", unique("untyped"));
    let typed = subscribe_typed(node, &typed_topic);
    // A subscriber without a type opens only a topic someone has opened.
    let untyped_publisher = publish(node, &untyped_topic);
    let untyped = subscribe_untyped(node, &untyped_topic);
    let text = introspection_text(node);
    let typed_cells = row_cells(&text, &typed_topic).expect("typed subscriber row");
    assert_eq!(typed_cells, [typed_topic.as_str(), DATA_TYPE, "depth=4"]);
    let untyped_cells = row_cells(&text, &untyped_topic).expect("untyped subscriber row");
    assert_eq!(untyped_cells, [untyped_topic.as_str(), "-", "depth=4"]);
    unsafe { hypha_subscriber_destroy(typed) };
    assert!(row_cells(&introspection_text(node), &typed_topic).is_none());
    unsafe { hypha_subscriber_destroy(untyped) };
    unsafe { hypha_publisher_destroy(untyped_publisher) };
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn a_repeated_config_key_replaces_its_row() {
    let node = create_node(&unique("introspection_cfg"));
    let key = c_text("period");
    let first = c_text("50");
    let second = c_text("20");
    let default = c_text("50");
    let source = HYPHA_CONFIG_SOURCE_APPLICATION;
    assert_eq!(
        unsafe {
            hypha_node_record_config(node, key.as_ptr(), first.as_ptr(), default.as_ptr(), source)
        },
        HyphaStatus::Ok
    );
    assert_eq!(
        unsafe {
            hypha_node_record_config(
                node,
                key.as_ptr(),
                second.as_ptr(),
                ptr::null(),
                HYPHA_CONFIG_SOURCE_COMMAND_LINE,
            )
        },
        HyphaStatus::Ok
    );
    let text = introspection_text(node);
    let rows: Vec<&str> =
        text.lines().filter(|line| line.trim_start().starts_with("period")).collect();
    assert_eq!(rows.len(), 1, "{text}");
    let cells: Vec<&str> =
        rows.first().map_or_else(Vec::new, |row| row.split_whitespace().collect());
    assert_eq!(cells, ["period", "20", "-", "command", "line"]);
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn a_config_row_from_the_default_source_shows_default() {
    let node = create_node(&unique("introspection_src"));
    let key = c_text("count");
    let value = c_text("7");
    let default = c_text("7");
    assert_eq!(
        unsafe {
            hypha_node_record_config(
                node,
                key.as_ptr(),
                value.as_ptr(),
                default.as_ptr(),
                HYPHA_CONFIG_SOURCE_DEFAULT,
            )
        },
        HyphaStatus::Ok
    );
    assert_eq!(
        row_cells(&introspection_text(node), "count"),
        Some(vec!["count".into(), "7".into(), "7".into(), "default".into()])
    );
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn a_null_default_is_no_default_and_an_empty_default_is_a_distinct_empty_value() {
    let node = create_node(&unique("introspection_empty"));
    let key = c_text("label");
    let value = c_text("");
    let empty_default = c_text("");
    assert_eq!(
        unsafe {
            hypha_node_record_config(
                node,
                key.as_ptr(),
                value.as_ptr(),
                empty_default.as_ptr(),
                HYPHA_CONFIG_SOURCE_APPLICATION,
            )
        },
        HyphaStatus::Ok
    );
    let text = introspection_text(node);
    assert_eq!(
        row_cells(&text, "label"),
        Some(vec!["label".into(), "\"\"".into(), "\"\"".into(), "application".into()]),
        "{text}"
    );
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn the_snapshot_call_reports_the_required_length_when_the_buffer_is_too_small() {
    let node = create_node(&unique("introspection_len"));
    let mut required = 0_usize;
    assert_eq!(
        unsafe { hypha_node_introspect(node, ptr::null_mut(), 0, &raw mut required) },
        HyphaStatus::TooLarge
    );
    assert!(required > 0);
    let mut small = vec![0_u8; required - 1];
    let mut reported = 0_usize;
    assert_eq!(
        unsafe { hypha_node_introspect(node, small.as_mut_ptr(), small.len(), &raw mut reported) },
        HyphaStatus::TooLarge
    );
    assert_eq!(reported, required);
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn the_snapshot_names_the_node_and_its_process() {
    let name = unique("introspection_identity");
    let node = create_node(&name);
    let snapshot = crate::common::introspection::snapshot(node);
    assert_eq!(snapshot.node, name);
    assert_eq!(snapshot.pid, i32::try_from(std::process::id()).expect("pid"));
    assert_eq!(snapshot.hypha, env!("CARGO_PKG_VERSION"));
    assert_ne!(snapshot.created, "");
    unsafe { hypha_node_destroy(node) };
}
