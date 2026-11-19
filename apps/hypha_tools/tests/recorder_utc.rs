//! A recording carries the framework send stamp of each frame as its MCAP times.
#![allow(clippy::expect_used)]

use hypha_node::sdk::{BusNode, SampleIdentity};
use hypha_testkit::unique_name as unique;
use hypha_tools::record_session::RecorderSession;
use hypha_tools::recorder::Recorder;

const BOOL_MD5: &str = "8b94c1b53db61fb6aed406028ad6332a";

/// The MCAP `(log_time, publish_time)` of every message of `path`, in file order.
fn recorded_times(path: &std::path::Path) -> Vec<(u64, u64)> {
    let bytes = std::fs::read(path).expect("recording");
    let mut times = Vec::new();
    for message in mcap::MessageStream::new(&bytes).expect("MCAP reader") {
        let message = message.expect("valid message");
        times.push((message.log_time, message.publish_time));
    }
    times
}

fn sample(topic: &str, framework_sent_nanos: u64) -> SampleIdentity<'_> {
    SampleIdentity {
        topic,
        transport_sent_nanos: 5,
        framework_sent_nanos,
        schema_hash: hypha_node::sdk::schema_hash_from_md5(BOOL_MD5).expect("a valid MD5"),
    }
}

#[test]
fn recording_times_are_the_framework_send_stamp() {
    let name = unique("rec_stamp");
    let topic = format!("/{name}/value");
    let output = std::env::temp_dir().join(format!("{name}.mcap"));
    let bus = BusNode::new(&name).expect("recorder test node");
    let _publisher =
        bus.publish_raw(&topic, "std_msgs/Bool", BOOL_MD5, "bool data\n").expect("publisher");
    let mut session = RecorderSession::new(&bus, Recorder::new(&output), false);

    // A frame stamped on another timeline than the host's: a vehicle recording replayed.
    let stamp = 1_600_000_000_123_456_789_u64;
    session.on_sample(&sample(&topic, stamp), &[1]);
    assert_eq!(session.recorder().counters().len(), 1, "schema must resolve");
    drop(session.finish());

    assert_eq!(recorded_times(&output), vec![(stamp, stamp)]);
    std::fs::remove_file(output).expect("remove recording");
}

#[test]
fn a_frame_without_a_framework_stamp_is_dropped_not_given_another_time() {
    let name = unique("rec_nostamp");
    let topic = format!("/{name}/value");
    let output = std::env::temp_dir().join(format!("{name}.mcap"));
    let bus = BusNode::new(&name).expect("recorder test node");
    let _publisher =
        bus.publish_raw(&topic, "std_msgs/Bool", BOOL_MD5, "bool data\n").expect("publisher");
    let mut session = RecorderSession::new(&bus, Recorder::new(&output), true);

    session.on_sample(&sample(&topic, 0), &[1]);

    let failure = session.failure().expect("a strict session fails on the drop").to_owned();
    assert!(failure.contains("the frame carries no framework send time"), "{failure}");
    drop(session.finish());
    assert!(recorded_times(&output).is_empty());
    std::fs::remove_file(output).expect("remove recording");
}

#[test]
fn a_looping_replay_is_recorded_with_its_repeated_times() {
    let name = unique("rec_loop");
    let topic = format!("/{name}/value");
    let output = std::env::temp_dir().join(format!("{name}.mcap"));
    let bus = BusNode::new(&name).expect("recorder test node");
    let _publisher =
        bus.publish_raw(&topic, "std_msgs/Bool", BOOL_MD5, "bool data\n").expect("publisher");
    // Strict, as a --sync recorder, which stays a participant through every pass.
    let mut session = RecorderSession::new(&bus, Recorder::new(&output), true);

    for stamp in [100_u64, 200, 100, 200] {
        session.on_sample(&sample(&topic, stamp), &[1]);
    }

    assert!(session.failure().is_none(), "a second pass is no failure");
    drop(session.finish());
    assert_eq!(recorded_times(&output), vec![(100, 100), (200, 200), (100, 100), (200, 200)]);
    std::fs::remove_file(output).expect("remove recording");
}

#[test]
fn a_second_type_on_a_recorded_topic_is_dropped_not_written_into_its_channel() {
    let name = unique("rec_two_types");
    let topic = format!("/{name}/value");
    let output = std::env::temp_dir().join(format!("{name}.mcap"));
    let bus = BusNode::new(&name).expect("recorder test node");
    let uint8_md5 = "7c8164229e7d2c17eb95e9231617fdee";
    let _bool_publisher =
        bus.publish_raw(&topic, "std_msgs/Bool", BOOL_MD5, "bool data\n").expect("Bool publisher");
    let _uint8_publisher = bus
        .publish_raw(&topic, "std_msgs/UInt8", uint8_md5, "uint8 data\n")
        .expect("UInt8 publisher");
    let mut session = RecorderSession::new(&bus, Recorder::new(&output), true);

    session.on_sample(&sample(&topic, 100), &[1]);
    let other = SampleIdentity {
        schema_hash: hypha_node::sdk::schema_hash_from_md5(uint8_md5).expect("a valid MD5"),
        ..sample(&topic, 200)
    };
    session.on_sample(&other, &[2]);

    let failure = session.failure().expect("a strict session fails on the drop").to_owned();
    assert!(
        failure.contains(
            "its type std_msgs/UInt8 differs from std_msgs/Bool, the type recorded for the topic"
        ),
        "{failure}"
    );
    drop(session.finish());
    assert_eq!(recorded_times(&output), vec![(100, 100)]);
    std::fs::remove_file(output).expect("remove recording");
}
