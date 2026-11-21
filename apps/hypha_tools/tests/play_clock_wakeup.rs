//! `hypha play --clock --rate max` replays in lockstep with a participant that answers at
//! once. That a report wakes the player's wait, rather than a poll, is tested where the
//! wait lives: `hypha_node/waitset_attach` (a waking subscription ends the run early).
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, clippy::arithmetic_side_effects)]

mod common;

use common::Responder;
use hypha_node::sdk::BusNode;
use hypha_tools::utc_time::parse_utc_nanos;
use mcap::records::MessageHeader;
use mcap::{WriteOptions, Writer};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const PARTICIPANT: &str = "wakeup_participant";
const START: &str = "2026-10-08T12:00:00Z";
const DATA_TOPIC: &str = "/wakeup_data";
/// Chunk boundaries of the recording: one message per 10 ms chunk.
const STEPS: u64 = 1_000;
const RUN_LIMIT: Duration = Duration::from_secs(60);

/// One `std_msgs/UInt8` every 10 ms for `STEPS` steps from `start`.
fn write_recording(path: &Path, start: u64) {
    let mut writer = Writer::with_options(
        File::create(path).expect("file"),
        WriteOptions::new().profile("ros1"),
    )
    .expect("a writer");
    let schema = writer.add_schema("std_msgs/UInt8", "ros1msg", b"uint8 data\n").expect("schema");
    let md5 =
        hypha_node::sdk::md5::md5_from_definition("std_msgs/UInt8", "uint8 data\n").expect("MD5");
    let metadata = BTreeMap::from([("md5sum".to_string(), md5)]);
    let channel = writer.add_channel(schema, DATA_TOPIC, "ros1", &metadata).expect("channel");
    for step in 0..STEPS {
        let time = start + step * 10_000_000;
        let header = MessageHeader {
            channel_id: channel,
            sequence: u32::try_from(step).expect("a small sequence"),
            log_time: time,
            publish_time: time,
        };
        writer.write_to_known_channel(&header, &[1]).expect("message");
    }
    writer.finish().expect("finish");
}

fn recording_path() -> PathBuf {
    std::env::temp_dir().join(format!("play_wakeup_{}.mcap", std::process::id()))
}

#[test]
fn a_replay_against_a_participant_that_answers_at_once_completes_in_lockstep() {
    let start = parse_utc_nanos(START).expect("the UTC start");
    let path = recording_path();
    write_recording(&path, start);
    let bus = BusNode::new(PARTICIPANT).expect("the participant's bus");
    let mut responder = Responder::new(&bus, PARTICIPANT);
    let mut child = Command::new(env!("CARGO_BIN_EXE_hypha"))
        .arg("play")
        .arg("--input")
        .arg(&path)
        .args(["--clock", "--start-utc", START, "--chunk", "10"])
        .args(["--nodes", PARTICIPANT, "--rate", "max"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("hypha_play");
    let began = Instant::now();
    let status = loop {
        let received = responder.receive();
        if let Some(newest) = received.last().copied() {
            responder.answer(newest);
        }
        if let Some(status) = child.try_wait().expect("the child status") {
            break status;
        }
        assert!(began.elapsed() < RUN_LIMIT, "the replay did not finish");
        std::thread::sleep(Duration::from_micros(50));
    };
    let mut stderr = String::new();
    child.stderr.take().expect("stderr").read_to_string(&mut stderr).expect("stderr");
    std::fs::remove_file(&path).expect("remove the recording");
    // With --nodes the player ends successfully only when every chunk was acknowledged.
    assert!(status.success(), "the replay failed: {stderr}");
}
