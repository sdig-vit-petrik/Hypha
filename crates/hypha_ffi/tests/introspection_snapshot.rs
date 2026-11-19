//! A snapshot is taken now.
#![allow(clippy::expect_used)]

mod common;
use crate::common::c_text;
use crate::common::create_node;
use crate::common::introspection::introspection_text;
use crate::common::unique;

use std::ptr;

use hypha_ffi::*;

const MD5: &str = "d9d213e8c146065ec958333c617eb1a6";
const DATA_TYPE: &str = "test/Counter";

fn publish(node: *mut HyphaNode, topic: &str) -> *mut HyphaPublisher {
    let (t, dt, m, d) = (c_text(topic), c_text(DATA_TYPE), c_text(MD5), c_text(""));
    let qos = HyphaPublisherOptions { latch: 0, loaned_samples: 0, qos: crate::common::test_qos() };
    let mut publisher = ptr::null_mut();
    let status = unsafe {
        hypha_publish_with_qos(
            node,
            t.as_ptr(),
            t.as_bytes().len(),
            dt.as_ptr(),
            dt.as_bytes().len(),
            m.as_ptr(),
            m.as_bytes().len(),
            d.as_ptr(),
            0,
            &raw const qos,
            &raw mut publisher,
        )
    };
    assert_eq!(status, HyphaStatus::Ok);
    publisher
}

#[test]
fn an_endpoint_destroyed_between_two_snapshots_is_gone_from_the_second() {
    let node = create_node(&unique("introspection_churn"));
    let topic = format!("/{}", unique("churn"));
    let publisher = publish(node, &topic);
    assert!(introspection_text(node).contains(&topic));

    unsafe { hypha_publisher_destroy(publisher) };
    assert!(!introspection_text(node).contains(&topic));

    unsafe { hypha_node_destroy(node) };
}

#[test]
fn endpoints_created_concurrently_from_several_threads_all_appear() {
    let node = create_node(&unique("introspection_concurrent"));
    let (thread_count, endpoints_per_thread) = (8, 5);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(thread_count + 1));

    let mut handles = Vec::new();
    for t in 0..thread_count {
        let (node_addr, b) = (node as usize, std::sync::Arc::clone(&barrier));
        handles.push(std::thread::spawn(move || {
            let node_ptr = node_addr as *mut HyphaNode;
            let mut publishers = Vec::new();
            for i in 0..endpoints_per_thread {
                publishers.push(publish(node_ptr, &format!("/concurrent_t{t}_i{i}")));
            }
            b.wait();
            b.wait();
            for p in publishers {
                unsafe { hypha_publisher_destroy(p) };
            }
        }));
    }

    barrier.wait();
    let text = introspection_text(node);
    for t in 0..thread_count {
        for i in 0..endpoints_per_thread {
            let topic = format!("/concurrent_t{t}_i{i}");
            assert!(text.contains(&topic), "snapshot missing topic {topic}");
        }
    }
    barrier.wait();
    for handle in handles {
        handle.join().expect("thread failed");
    }
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn a_large_configuration_is_read_back_whole_and_the_node_keeps_running() {
    let node = create_node(&unique("introspection_big"));
    let value = c_text(&"x".repeat(1000));
    let row_count = 1_500;
    for index in 0..row_count {
        let key = c_text(&format!("row_{index}"));
        let status =
            unsafe { hypha_node_record_config(node, key.as_ptr(), value.as_ptr(), ptr::null(), 0) };
        assert_eq!(status, HyphaStatus::Ok);
    }

    let snapshot = crate::common::introspection::snapshot(node);
    assert_eq!(snapshot.configuration.len(), row_count);
    assert_ne!(unsafe { hypha_node_ok(node) }, 0, "the node keeps running");

    unsafe { hypha_node_destroy(node) };
}
