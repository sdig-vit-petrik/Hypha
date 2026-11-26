//! Cycle and service rows of the node introspection.
#![allow(clippy::expect_used, clippy::too_many_lines)]

mod common;
use crate::common::c_text;
use crate::common::create_node;
use crate::common::introspection::{binding, cycle, introspection_text, publisher, subscriber};
use crate::common::supervision::{bind, node_with_parameter_text};
use crate::common::unique;

use std::ffi::c_void;
use std::ptr;

use hypha_ffi::waitset::*;
use hypha_ffi::*;
use hypha_msgs::hypha_msgs::{IntrospectedEndpoint, IntrospectedInput};

const MD5: &str = "d9d213e8c146065ec958333c617eb1a6";
const DATA_TYPE: &str = "std_msgs/Counter";

/// The line of `text` that starts with `topic`, split into its cells.
fn row_cells(text: &str, topic: &str) -> Option<Vec<String>> {
    text.lines()
        .find(|line| line.trim_start().starts_with(topic) && line.starts_with("  "))
        .map(|line| line.split_whitespace().map(str::to_owned).collect())
}

fn subscriber_options() -> HyphaSubscriberOptions {
    HyphaSubscriberOptions {
        depth: 4,
        no_publisher_warning_after_nanos: 0,
        qos: crate::common::test_qos(),
    }
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

extern "C" fn empty_step(
    _image: *const HyphaCycleImage,
    _time: *const HyphaCycleTime,
    _emission: *mut HyphaCycleEmission,
    _context: *mut c_void,
) -> HyphaCallbackResult {
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

#[test]
fn a_cycle_row_follows_its_bindings_and_leaves_on_destruction() {
    let node = create_node(&unique("introspection_cycle"));
    let mut waitset = ptr::null_mut();
    assert_eq!(unsafe { hypha_node_default_waitset(node, &raw mut waitset) }, HyphaStatus::Ok);

    let cycle_name = unique("drive");
    let name = c_text(&cycle_name);
    let mut cycle = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                waitset,
                name.as_ptr(),
                name.as_bytes().len(),
                50_000_000,
                Some(empty_step),
                ptr::null_mut(),
                None,
                &raw mut cycle,
            )
        },
        HyphaStatus::Ok
    );
    let cycle_row = format!("  {cycle_name}");
    let before = introspection_text(node);
    let cells = row_cells(&before, &cycle_name).expect("cycle row");
    assert_eq!(cells, [cycle_name.as_str(), "20.0", "Hz", "0", "0"], "{before}");
    assert!(before.contains(&cycle_row));

    let topic = format!("/{}", unique("out"));
    let publisher_handle = publish(node, &topic);
    let mut index = 0_u32;
    assert_eq!(
        unsafe {
            hypha_cycle_bind_output(
                cycle,
                publisher_handle,
                HyphaCycleOutputFailure::FailRun as u32,
                1,
                &raw mut index,
            )
        },
        HyphaStatus::Ok
    );
    let bound = introspection_text(node);
    let owned = publisher(node, &topic).expect("bound publisher row");
    assert_eq!(owned.output_cycle, cycle_name);
    assert_eq!(owned.output_failure, IntrospectedEndpoint::OUTPUT_FAIL_RUN);
    let cycle_cells = row_cells(&bound, &cycle_name).expect("cycle row after bind");
    assert_eq!(cycle_cells, [cycle_name.as_str(), "20.0", "Hz", "0", "1"]);

    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    let after = introspection_text(node);
    assert!(row_cells(&after, &cycle_name).is_none());
    assert!(row_cells(&after, &topic).is_none());
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn a_cycle_row_lists_every_trigger() {
    let node = node_with_parameter_text(
        "introspection_triggers",
        "[ports.inputs.joy]\nsilence_ms = 500\n\n[ports.inputs.nav]\nsilence_ms = 1000\n",
    );
    let waitset = create_waitset(node);
    let cycle_name = unique("mux");
    let name = c_text(&cycle_name);
    let mut created = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                waitset,
                name.as_ptr(),
                name.as_bytes().len(),
                0,
                Some(empty_step),
                ptr::null_mut(),
                None,
                &raw mut created,
            )
        },
        HyphaStatus::Ok
    );
    let queued = HyphaCycleBindingOptions {
        mode: HyphaCycleInputMode::Queued as u32,
        max_samples_per_iteration: 1,
        optional: 0,
    };
    let topics = ["nav", "joy"].map(|port| {
        let topic = format!("/{}", unique(port));
        let mut source = ptr::null_mut();
        let subscriber_handle = subscribe_typed(node, &topic);
        assert_eq!(
            unsafe { hypha_input_source_create(waitset, subscriber_handle, &raw mut source) },
            HyphaStatus::Ok
        );
        let mut index = 0_u32;
        assert_eq!(
            unsafe { hypha_cycle_bind_input(created, source, &raw const queued, &raw mut index) },
            HyphaStatus::Ok
        );
        let bound = unsafe { bind(created, node, index, port, (true, false, true)) };
        assert_eq!(bound.status, HyphaStatus::Ok, "{}", bound.detail);
        topic
    });
    // Port order is binding order, whatever the order of the calls.
    for index in [1, 0] {
        assert_eq!(unsafe { hypha_cycle_set_trigger(created, index) }, HyphaStatus::Ok);
    }
    assert_eq!(unsafe { hypha_cycle_commit(created) }, HyphaStatus::Ok);
    let row = cycle(node, &cycle_name).expect("cycle row");
    assert_eq!(row.triggers, topics);
    assert!(!row.has_period);
    assert_eq!(unsafe { hypha_cycle_destroy(created) }, HyphaStatus::Ok);
    unsafe { hypha_node_destroy(node) };
}

fn create_waitset(node: *mut HyphaNode) -> *mut HyphaWaitSet {
    let mut waitset = ptr::null_mut();
    assert_eq!(unsafe { hypha_node_default_waitset(node, &raw mut waitset) }, HyphaStatus::Ok);
    waitset
}

fn create_cycle(waitset: *mut HyphaWaitSet, name: &str) -> *mut HyphaCycle {
    let name = c_text(name);
    let mut cycle = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                waitset,
                name.as_ptr(),
                name.as_bytes().len(),
                50_000_000,
                Some(empty_step),
                ptr::null_mut(),
                None,
                &raw mut cycle,
            )
        },
        HyphaStatus::Ok
    );
    cycle
}

#[test]
fn a_bound_input_shows_its_cycle_and_binding_policy() {
    let node = node_with_parameter_text(
        "introspection_bind",
        "[ports.inputs.first]\ntopic = \"/first\"\nsilence_ms = 50\nstagnation_ms = 100\nignored = true\n\n[ports.inputs.second]\ntopic = \"/second\"\nsilence_ms = 150\n",
    );
    let waitset = create_waitset(node);
    let topic = format!("/{}", unique("input"));
    let subscriber_handle = subscribe_typed(node, &topic);
    let mut source = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_input_source_create(waitset, subscriber_handle, &raw mut source) },
        HyphaStatus::Ok
    );
    let cycle_name = unique("reader");
    let cycle = create_cycle(waitset, &cycle_name);

    let latest = HyphaCycleBindingOptions {
        mode: HyphaCycleInputMode::Latest as u32,
        max_samples_per_iteration: 0,
        optional: 0,
    };
    let mut index = 0_u32;
    assert_eq!(
        unsafe { hypha_cycle_bind_input(cycle, source, &raw const latest, &raw mut index) },
        HyphaStatus::Ok
    );
    let subscriber_id = subscriber(node, &topic).expect("bound subscriber row").id;
    let first = binding(node, &cycle_name, index).expect("first binding row");
    assert_eq!(first.subscriber_id, subscriber_id);
    assert_eq!(first.mode, IntrospectedInput::MODE_LATEST);
    assert!(!first.supervised);

    assert_eq!(
        unsafe { bind(cycle, node, index, "first", (true, true, true)).status },
        HyphaStatus::Ok
    );
    let supervised = binding(node, &cycle_name, index).expect("supervised binding row");
    assert!(supervised.supervised);
    assert_eq!(supervised.silence_nanos, 50_000_000);
    assert_eq!(supervised.stagnation_nanos, 100_000_000);
    assert!(supervised.ignored);
    let mut other_index = 0;
    assert_eq!(
        unsafe {
            let optional_latest = HyphaCycleBindingOptions { optional: 1, ..latest };
            hypha_cycle_bind_input(cycle, source, &raw const optional_latest, &raw mut other_index)
        },
        HyphaStatus::Ok
    );
    assert_eq!(
        unsafe { bind(cycle, node, other_index, "second", (true, false, true)).status },
        HyphaStatus::Ok
    );
    let shared = introspection_text(node);
    // The two bindings of the shared source name one subscriber and keep their own limits.
    let second = binding(node, &cycle_name, other_index).expect("second binding row");
    assert_eq!(second.subscriber_id, subscriber_id);
    assert_eq!(second.silence_nanos, 150_000_000);
    assert_eq!(binding(node, &cycle_name, index).map(|row| row.silence_nanos), Some(50_000_000));
    let binding_row = |index: &str, role: &str| -> Vec<String> {
        [cycle_name.as_str(), index, topic.as_str(), role].map(str::to_owned).to_vec()
    };
    let row_exists = |expected: &Vec<String>| -> bool {
        shared
            .lines()
            .map(|line| line.split_whitespace().map(str::to_owned).collect::<Vec<_>>())
            .any(|cells| &cells == expected)
    };
    assert!(row_exists(&binding_row("0", "required")), "{shared}");
    assert!(row_exists(&binding_row("1", "optional")), "{shared}");
    let peer_name = unique("peer");
    let peer_cycle = create_cycle(waitset, &peer_name);
    assert_eq!(
        unsafe {
            hypha_cycle_bind_input(peer_cycle, source, &raw const latest, &raw mut other_index)
        },
        HyphaStatus::Ok
    );

    let disabled_name = unique("optional");
    let disabled_cycle = create_cycle(waitset, &disabled_name);
    let mut disabled_source = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_input_source_create(waitset, ptr::null_mut(), &raw mut disabled_source) },
        HyphaStatus::Ok
    );
    let queued = HyphaCycleBindingOptions {
        mode: HyphaCycleInputMode::Queued as u32,
        max_samples_per_iteration: 16,
        optional: 0,
    };
    assert_eq!(
        unsafe {
            hypha_cycle_bind_input(
                disabled_cycle,
                disabled_source,
                &raw const queued,
                &raw mut index,
            )
        },
        HyphaStatus::Ok
    );
    let text = introspection_text(node);
    let disabled_cells = row_cells(&text, &disabled_name).expect("cycle row");
    assert_eq!(disabled_cells, [disabled_name.as_str(), "20.0", "Hz", "1", "0"], "{text}");
    // An unwired input has a binding row that names no subscriber.
    let disabled = binding(node, &disabled_name, index).expect("disabled binding row");
    assert_eq!(disabled.subscriber_id, 0, "{text}");
    assert_eq!(disabled.mode, IntrospectedInput::MODE_QUEUED);
    assert_eq!(disabled.max_samples_per_iteration, 16);

    assert_eq!(unsafe { hypha_cycle_destroy(disabled_cycle) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    let remaining = introspection_text(node);
    assert_eq!(subscriber(node, &topic).map(|row| row.id), Some(subscriber_id), "{remaining}");
    let peer = binding(node, &peer_name, 0).expect("the peer keeps its binding");
    assert_eq!(peer.subscriber_id, subscriber_id);
    assert!(!remaining.lines().any(|line| line.contains(&cycle_name)), "{remaining}");
    assert_eq!(unsafe { hypha_cycle_destroy(peer_cycle) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_input_source_destroy(disabled_source) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_input_source_destroy(source) }, HyphaStatus::Ok);
    let after = introspection_text(node);
    assert!(row_cells(&after, &disabled_name).is_none(), "{after}");
    assert!(row_cells(&after, &cycle_name).is_none(), "{after}");
    unsafe { hypha_node_destroy(node) };
}
