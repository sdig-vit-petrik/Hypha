//! A refused frame is logged once per subscriber per second. Each test needs its own
//! process (nextest): the logging configuration is process-wide.
#![allow(clippy::expect_used)]

mod common;
use crate::common::{c_text, create_node_with_file, unique};

use std::ptr;

use hypha_ffi::*;

const MARKER: &str = "refused a frame: CRC mismatch";

/// Publish one frame and change its payload after the CRC was sealed, as
/// another process with write access to the segment could.
fn publish_corrupted(publisher: *mut HyphaPublisher) {
    let mut loan = ptr::null_mut();
    let mut bytes = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_publisher_loan_frame(publisher, 4, &raw mut loan, &raw mut bytes) },
        HyphaStatus::Ok
    );
    let length = 4;
    unsafe { std::slice::from_raw_parts_mut(bytes, length).fill(1) };
    assert_eq!(unsafe { hypha_loan_send(loan, -1) }, HyphaStatus::Ok);
    unsafe { bytes.write(2) };
}

fn receive_refused(subscriber: *mut HyphaSubscriber) {
    let received = unsafe { crate::common::receive::receive(subscriber) };
    assert!(matches!(received, Err(HyphaStatus::CorruptPayload)), "the corrupted frame is refused");
}

#[test]
fn a_burst_of_refused_frames_is_logged_once_per_second() {
    let dir = std::env::temp_dir().join(unique("rejection_warning"));
    std::fs::create_dir_all(&dir).expect("fixture directory");
    let parameters = dir.join("params.toml");
    let table =
        "[log]\nconsole = false\n[log.sink.main]\npath = \"refused.log\"\nlevel = \"info\"\n";
    std::fs::write(&parameters, table).expect("parameter fixture");
    let node = create_node_with_file(&unique("rejection_warning"), Some(&parameters));

    let topic = c_text(&format!("/hypha_ffi/refused/{}", unique("topic")));
    let data_type = c_text("std_msgs/Bytes");
    let md5 = c_text("d9d213e8c146065ec958333c617eb1a6");
    let definition = c_text("");
    let publisher_qos =
        HyphaPublisherOptions { latch: 0, loaned_samples: 0, qos: crate::common::test_qos() };
    let mut publisher = ptr::null_mut();
    assert_eq!(
        unsafe {
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
                &raw const publisher_qos,
                &raw mut publisher,
            )
        },
        HyphaStatus::Ok
    );
    let subscriber_qos = HyphaSubscriberOptions {
        depth: 4,
        no_publisher_warning_after_nanos: 0,
        qos: hypha_ffi::HyphaQos::default(),
    };
    let mut subscriber = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_subscribe_typed(
                node,
                topic.as_ptr(),
                topic.as_bytes().len(),
                ptr::null(),
                0,
                ptr::null(),
                0,
                std::ptr::null(),
                0,
                &raw const subscriber_qos,
                &raw mut subscriber,
            )
        },
        HyphaStatus::Ok
    );

    let lines = || -> Vec<String> {
        let text = std::fs::read_to_string(dir.join("refused.log")).unwrap_or_default();
        text.lines().filter(|line| line.contains(MARKER)).map(str::to_owned).collect()
    };
    for _ in 0..100 {
        publish_corrupted(publisher);
        receive_refused(subscriber);
    }
    assert_eq!(lines().len(), 1, "{:?}", lines());

    std::thread::sleep(std::time::Duration::from_millis(1100));
    publish_corrupted(publisher);
    receive_refused(subscriber);
    let lines = lines();
    assert_eq!(lines.len(), 2, "{lines:?}");
    let topic = topic.to_str().expect("topic text");
    assert!(lines.iter().all(|line| line.contains(&format!("topic '{topic}'"))), "{lines:?}");
    assert!(lines.last().is_some_and(|line| line.contains("; 99 more refused")), "{lines:?}");

    unsafe {
        hypha_subscriber_destroy(subscriber);
        hypha_publisher_destroy(publisher);
        hypha_node_destroy(node);
    }
}
