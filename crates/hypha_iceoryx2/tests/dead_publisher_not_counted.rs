//! A crashed publisher is not counted as connected.

#![allow(clippy::expect_used)]

use std::io::{BufRead as _, BufReader};
use std::process::{Command, Stdio};
use std::time::Duration;

use hypha_core::port::TopicSpec;
use hypha_iceoryx2::Iox2Transport;
use hypha_testkit::unique_name;

#[test]
#[allow(clippy::expect_used)]
fn a_crashed_publisher_stops_being_counted() {
    let topic = format!("/{}", unique_name("dead_pub_topic"));
    let node = unique_name("dead_pub_node");
    let mut child = Command::new(env!("CARGO_BIN_EXE_hypha_iceoryx2_crash_node_helper"))
        .arg(&node)
        .arg(&topic)
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn crash_node_helper");
    let stdout = child.stdout.take().expect("helper stdout piped");
    let mut ready = String::new();
    BufReader::new(stdout).read_line(&mut ready).expect("read ready line from helper");
    assert_eq!(ready.trim(), "ready", "the helper publishes before it reports ready");

    let transport = Iox2Transport::new().expect("transport");
    let spec = TopicSpec::new(&topic).with_type("std_msgs/String").keeping(2);
    let subscriber = transport.subscribe(&spec).expect("subscriber");
    assert_eq!(subscriber.publisher_count(), Ok(1), "the publisher of a live node is counted");

    child.kill().expect("kill crash_node_helper");
    child.wait().expect("reap killed helper");
    std::thread::sleep(Duration::from_millis(200));

    assert_eq!(subscriber.publisher_count(), Ok(0), "the publisher of a dead node is not counted");
}
