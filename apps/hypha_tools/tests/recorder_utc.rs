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
    let found = recorded_times(&output);
    assert!(found.is_empty(), "{found:?}");
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

/// The channels of `path` as `(topic, schema name, md5, messages)`, in the order they were made.
fn channels(path: &std::path::Path) -> Vec<(String, String, String, usize)> {
    let bytes = std::fs::read(path).expect("recording");
    let mut found: Vec<(u16, (String, String, String, usize))> = Vec::new();
    for message in mcap::MessageStream::new(&bytes).expect("MCAP reader") {
        let message = message.expect("valid message");
        let id = message.channel.id;
        if let Some((_, entry)) = found.iter_mut().find(|(known, _)| *known == id) {
            entry.3 = entry.3.saturating_add(1);
            continue;
        }
        let schema = message.channel.schema.as_ref().expect("a schema");
        let md5 = message.channel.metadata.get("md5sum").cloned().unwrap_or_default();
        found.push((id, (message.channel.topic.clone(), schema.name.clone(), md5, 1)));
    }
    found.into_iter().map(|(_, entry)| entry).collect()
}

#[test]
fn two_streams_of_one_topic_are_two_channels_each_with_its_own_schema() {
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
    // Strict, as a --sync recorder: the second stream is no failure.
    let mut session = RecorderSession::new(&bus, Recorder::new(&output), true);

    session.on_sample(&sample(&topic, 100), &[1]);
    let other = SampleIdentity {
        schema_hash: hypha_node::sdk::schema_hash_from_md5(uint8_md5).expect("a valid MD5"),
        ..sample(&topic, 200)
    };
    session.on_sample(&other, &[2]);
    session.on_sample(&sample(&topic, 300), &[0]);

    assert!(session.failure().is_none(), "{:?}", session.failure());
    let counts = session.recorder().counters();
    assert_eq!(counts.len(), 2, "one channel for each stream");
    let titles: Vec<String> = counts.iter().map(|channel| channel.title(&counts)).collect();
    assert_eq!(
        titles,
        [
            format!("{topic} [std_msgs/Bool (8b94c1b5)]"),
            format!("{topic} [std_msgs/UInt8 (7c816422)]"),
        ]
    );
    drop(session.finish());
    assert_eq!(recorded_times(&output), vec![(100, 100), (200, 200), (300, 300)]);
    assert_eq!(
        channels(&output),
        vec![
            (topic.clone(), "std_msgs/Bool".to_owned(), BOOL_MD5.to_owned(), 2),
            (topic, "std_msgs/UInt8".to_owned(), uint8_md5.to_owned(), 1),
        ]
    );
    std::fs::remove_file(output).expect("remove recording");
}

/// The file the recorder wrote before streams: the calls it made on the MCAP writer for a
/// topic of one type, in the order it made them.
fn the_file_of_the_recorder_before_streams(samples: &[(&str, u64, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    let options =
        mcap::WriteOptions::new().profile("ros1").compression(Some(mcap::Compression::Lz4));
    let mut writer =
        mcap::Writer::with_options(std::io::Cursor::new(&mut out), options).expect("writer");
    let mut channels: Vec<(String, u16, u32)> = Vec::new();
    for (topic, stamp, bytes) in samples {
        let position = channels.iter().position(|(known, _, _)| known == topic);
        let (channel_id, sequence) = if let Some(index) = position {
            let entry = channels.get_mut(index).expect("known");
            entry.2 = entry.2.saturating_add(1);
            (entry.1, entry.2.saturating_sub(1))
        } else {
            let schema =
                writer.add_schema("std_msgs/Bool", "ros1msg", b"bool data\n").expect("schema");
            let metadata =
                std::collections::BTreeMap::from([("md5sum".to_owned(), BOOL_MD5.to_owned())]);
            let channel = writer.add_channel(schema, topic, "ros1", &metadata).expect("channel");
            channels.push(((*topic).to_owned(), channel, 1));
            (channel, 0)
        };
        let header = mcap::records::MessageHeader {
            channel_id,
            sequence,
            log_time: *stamp,
            publish_time: *stamp,
        };
        writer.write_to_known_channel(&header, bytes).expect("message");
    }
    writer.finish().expect("finish");
    drop(writer);
    out
}

#[test]
fn a_recording_of_one_type_per_topic_is_the_file_the_recorder_made_before_streams() {
    let name = unique("rec_identical");
    let output = std::env::temp_dir().join(format!("{name}.mcap"));
    let bus = BusNode::new(&name).expect("recorder test node");
    let (a, b) = (format!("/{name}/a"), format!("/{name}/b"));
    let _publisher_a = bus.publish_raw(&a, "std_msgs/Bool", BOOL_MD5, "bool data\n").expect("a");
    let _publisher_b = bus.publish_raw(&b, "std_msgs/Bool", BOOL_MD5, "bool data\n").expect("b");
    let mut session = RecorderSession::new(&bus, Recorder::new(&output), false);
    let written: [(&str, u64, &[u8]); 4] =
        [(&a, 100, &[1]), (&b, 150, &[0]), (&a, 200, &[0]), (&b, 250, &[1])];
    for (topic, stamp, bytes) in written {
        session.on_sample(&sample(topic, stamp), bytes);
    }
    drop(session.finish());

    let now = std::fs::read(&output).expect("recording");
    assert_eq!(now, the_file_of_the_recorder_before_streams(&written));
    std::fs::remove_file(output).expect("remove recording");
}
