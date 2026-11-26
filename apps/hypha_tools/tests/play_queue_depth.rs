//! The transport queue of a replayed topic.
#![allow(clippy::expect_used, clippy::panic, clippy::arithmetic_side_effects)]

mod common;

use common::Responder;
use hypha_node::sdk::{BusNode, RawSubscriber, SubscriberOptions};
use hypha_tools::play_session::{channel_publisher_options, publish_channel};
use hypha_tools::player::ChannelResolution;
use hypha_tools::utc_time::parse_utc_nanos;
use mcap::records::MessageHeader;
use mcap::{WriteOptions, Writer};
use std::collections::BTreeMap;
use std::fs::File;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DATA_TYPE: &str = "std_msgs/UInt8";
const DEFINITION: &str = "uint8 data\n";

fn unique_topic(label: &str) -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).expect("UTC").as_nanos();
    format!("/play_queue_{label}_{}_{nanos:x}", std::process::id())
}

#[test]
fn a_node_with_subscriber_starts_after_the_player_publishes() {
    let bus = BusNode::new("play_queue_player_first").expect("node");
    let topic = unique_topic("player_first");
    let md5 = hypha_node::sdk::md5::md5_from_definition(DATA_TYPE, DEFINITION).expect("md5");
    let _player = bus
        .publish_raw_configured(
            &topic,
            DATA_TYPE,
            &md5,
            DEFINITION,
            channel_publisher_options(false, None),
        )
        .expect("the player publishes the topic");

    let options = SubscriberOptions::default().keeping(8);
    let _subscriber = bus.subscribe_raw_with_options(&topic, options).expect("subscriber connects");
}

#[test]
fn a_topic_opened_before_its_subscriber_is_still_published() {
    let bus = BusNode::new("play_queue_opened_first").expect("node");
    let topic = unique_topic("opened_first");
    let md5 = hypha_node::sdk::md5::md5_from_definition(DATA_TYPE, DEFINITION).expect("md5");
    // A subscriber that declares no policy never creates its topic, so this test opens it.
    let _opener = bus.publish_raw(&topic, DATA_TYPE, &md5, DEFINITION).expect("the topic's opener");
    let _subscriber = bus
        .subscribe_raw_with_schema(&topic, DATA_TYPE, &md5, SubscriberOptions::default().keeping(4))
        .expect("the subscriber joins the opened topic");
    let channel = ChannelResolution {
        topic: topic.clone(),
        data_type: DATA_TYPE.to_string(),
        md5,
        definition: DEFINITION.to_string(),
        max_payload: None,
    };
    let _player = publish_channel(&bus, &channel, false).expect("the topic is published");
}

const PARTICIPANT: &str = "play_queue_participant";
const START: &str = "2026-10-08T12:00:00Z";
/// Ten messages, each 1 ms apart, all inside the first 100 ms chunk.
const BURST: u8 = 10;

/// Writes [`BURST`] messages on `topic`, all inside the first chunk after `START`.
fn write_burst(path: &std::path::Path, topic: &str, start: u64) {
    let mut writer = Writer::with_options(
        File::create(path).expect("the recording file"),
        WriteOptions::new().profile("ros1"),
    )
    .expect("a writer");
    let schema = writer.add_schema(DATA_TYPE, "ros1msg", DEFINITION.as_bytes()).expect("schema");
    let md5 = hypha_node::sdk::md5::md5_from_definition(DATA_TYPE, DEFINITION).expect("md5");
    let metadata = BTreeMap::from([("md5sum".to_string(), md5)]);
    let channel = writer.add_channel(schema, topic, "ros1", &metadata).expect("channel");
    for value in 0..BURST {
        let time = start + u64::from(value + 1) * 1_000_000;
        writer
            .write_to_known_channel(
                &MessageHeader {
                    channel_id: channel,
                    sequence: u32::from(value),
                    log_time: time,
                    publish_time: time,
                },
                &[value],
            )
            .expect("message");
    }
    writer.finish().expect("finish");
}

fn drain_values(data: &mut RawSubscriber, values: &mut Vec<u8>) {
    while let Some(sample) = data.try_receive().expect("a data receive") {
        values.push(*sample.bytes().expect("a payload").first().expect("one byte"));
    }
}

#[test]
fn a_chunk_is_split_so_a_small_subscriber_queue_keeps_every_sample() {
    let topic = unique_topic("split");
    let start = parse_utc_nanos(START).expect("the UTC start");
    let path = std::env::temp_dir().join(format!("play_queue_split_{}.mcap", std::process::id()));
    write_burst(&path, &topic, start);

    let bus = BusNode::new("play_queue_split_participant").expect("node");
    // The test opens the topic first: a subscriber that declares no policy never creates it.
    let md5 = hypha_node::sdk::md5::md5_from_definition(DATA_TYPE, DEFINITION).expect("md5");
    let _opener = bus.publish_raw(&topic, DATA_TYPE, &md5, DEFINITION).expect("the topic's opener");
    let mut data = bus
        .subscribe_raw_with_schema(&topic, DATA_TYPE, "", SubscriberOptions::default().keeping(4))
        .expect("a subscriber of queue 4");
    let mut responder = Responder::new(&bus, PARTICIPANT);
    let mut child = Command::new(env!("CARGO_BIN_EXE_hypha"))
        .arg("play")
        .arg("--input")
        .arg(&path)
        .args(["--clock", "--start-utc", START, "--nodes", PARTICIPANT, "--rate", "max"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("hypha_play");

    // The subscriber drains its queue before it answers each clock sample.
    let began = Instant::now();
    let mut values = Vec::new();
    let mut clock_values: Vec<u64> = Vec::new();
    let status = loop {
        let exited = child.try_wait().expect("the child status");
        let clocks = responder.receive();
        drain_values(&mut data, &mut values);
        for clock in &clocks {
            if clock_values.last() != Some(&clock.nanos) {
                clock_values.push(clock.nanos);
            }
        }
        if let Some(newest) = clocks.last().copied() {
            responder.answer(newest);
        }
        if let Some(status) = exited {
            break status;
        }
        if began.elapsed() >= Duration::from_secs(30) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the replay did not finish");
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    drain_values(&mut data, &mut values);
    std::fs::remove_file(&path).expect("remove the recording");
    assert!(status.success(), "the replay succeeds");

    assert_eq!(values, (0..BURST).collect::<Vec<u8>>(), "every message arrives, in order");
    assert_eq!(data.missing_sequences().expect("the sequence count"), 0);
    // The fifth and the ninth message would overflow the queue of 4: each shortened chunk
    // closes one nanosecond before it, so it holds every message stamped at or before its
    // clock. The grid boundary at 100 ms ends the pass.
    let millis = |ms: u64| start + ms * 1_000_000;
    let expected = vec![start, millis(5) - 1, millis(9) - 1, millis(100)];
    assert_eq!(clock_values, expected, "the chunks close before the messages that overflow");
}
