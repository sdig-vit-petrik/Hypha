//! Housekeeping runs on the caller's `WaitSet`, not on a thread of its own.
#![allow(clippy::expect_used)]

use hypha_core::port::TopicSpec;
use hypha_iceoryx2::Iox2Transport;
use hypha_testkit::unique_name;

/// Count the threads of this process through the operating system.
fn thread_count() -> usize {
    let pid = std::process::id();
    let output =
        std::process::Command::new("ps").args(["-M", &pid.to_string()]).output().expect("ps -M");
    String::from_utf8_lossy(&output.stdout).lines().count().saturating_sub(1)
}

#[test]
fn a_latched_publisher_creates_no_thread() {
    let before = thread_count();
    let transport = Iox2Transport::with_name(&unique_name("latched_node")).expect("transport");

    let topic = format!("/{}", unique_name("latched_topic"));
    let spec = TopicSpec::new(&topic).with_type("hypha_test/Data").latched().keeping(1);
    let _publisher = transport.publish(&spec).expect("publish latched");

    assert_eq!(thread_count(), before, "latch replay must not start a worker");
}

#[test]
fn many_schema_publishers_create_no_thread() {
    let before = thread_count();
    let transport = Iox2Transport::with_name(&unique_name("schema_node")).expect("transport");

    let mut publishers = Vec::new();
    for index in 0..8 {
        let topic = format!("/{}_{index}", unique_name("schema_topic"));
        let spec = TopicSpec::new(&topic)
            .with_type("std_msgs/Float64")
            .with_md5("fdb28210bfa9d7c91146260178d9a584")
            .with_definition("float64 data");
        publishers.push(transport.publish(&spec).expect("publish with definition"));
    }

    assert_eq!(thread_count(), before, "a schema server must not start a worker");
}

#[test]
fn latch_replay_progresses_only_when_the_owner_drives_it() {
    let transport = Iox2Transport::with_name(&unique_name("replay_node")).expect("transport");

    let topic = format!("/{}", unique_name("replay_topic"));
    let spec = TopicSpec::new(&topic).with_type("hypha_test/Data").latched().keeping(1);
    let _publisher = transport.publish(&spec).expect("publish latched");

    transport.replay_latched();
    transport.replay_latched();
}
