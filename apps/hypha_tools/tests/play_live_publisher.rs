//! A replayed topic another process already publishes is reported once; the replay still runs.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

#[allow(dead_code, reason = "each test binary uses part of the shared helpers")]
mod common;

use hypha_node::sdk::BusNode;
use mcap::records::MessageHeader;
use mcap::{WriteOptions, Writer};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const START: &str = "2026-10-08T12:00:00Z";
const TOPIC: &str = "/live_data";
const DATA_TYPE: &str = "std_msgs/UInt8";
const DEFINITION: &str = "uint8 data\n";
/// The longest the replay may take to exit.
const EXIT_LIMIT: Duration = Duration::from_secs(20);

fn recording_path(test: &str) -> PathBuf {
    std::env::temp_dir().join(format!("play_live_publisher_{test}_{}.mcap", std::process::id()))
}

/// Three messages on [`TOPIC`], in the first 200 ms after the start.
fn write_recording(path: &Path) {
    let start = hypha_tools::utc_time::parse_utc_nanos(START).expect("the UTC start");
    let mut writer = Writer::with_options(
        File::create(path).expect("the recording file"),
        WriteOptions::new().profile("ros1"),
    )
    .expect("a writer");
    let schema = writer.add_schema(DATA_TYPE, "ros1msg", DEFINITION.as_bytes()).expect("schema");
    let md5 = hypha_node::sdk::md5::md5_from_definition(DATA_TYPE, DEFINITION).expect("MD5");
    let metadata = BTreeMap::from([("md5sum".to_string(), md5)]);
    let channel = writer.add_channel(schema, TOPIC, "ros1", &metadata).expect("channel");
    for (sequence, offset_millis) in [0_u64, 100, 200].into_iter().enumerate() {
        let time = start
            .checked_add(offset_millis.checked_mul(1_000_000).expect("a short offset"))
            .expect("a time in the window");
        let header = MessageHeader {
            channel_id: channel,
            sequence: u32::try_from(sequence).expect("a small sequence"),
            log_time: time,
            publish_time: time,
        };
        writer.write_to_known_channel(&header, &[1]).expect("message");
    }
    writer.finish().expect("finish");
    drop(writer);
}

/// Runs `hypha play` at maximum rate to the end, and returns its status and both outputs.
fn run_player(path: &Path) -> (std::process::ExitStatus, String, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_hypha"))
        .arg("play")
        .arg("--input")
        .arg(path)
        .args(["--start-utc", START, "--rate", "max"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("hypha_play");
    let began = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("the child status") {
            break status;
        }
        if began.elapsed() >= EXIT_LIMIT {
            let _ = child.kill();
            let _ = child.wait();
            panic!("hypha_play did not exit within {EXIT_LIMIT:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut stdout = String::new();
    child.stdout.take().expect("stdout").read_to_string(&mut stdout).expect("stdout text");
    let mut stderr = String::new();
    child.stderr.take().expect("stderr").read_to_string(&mut stderr).expect("stderr text");
    (status, stdout, stderr)
}

#[test]
fn a_live_publisher_is_reported_once_and_does_not_refuse() {
    let path = recording_path("live");
    write_recording(&path);

    let bus = BusNode::new("live_publisher").expect("the other publisher's bus");
    let md5 = hypha_node::sdk::md5::md5_from_definition(DATA_TYPE, DEFINITION).expect("MD5");
    let _other = bus.publish_raw(TOPIC, DATA_TYPE, &md5, DEFINITION).expect("the live publisher");

    let (status, stdout, stderr) = run_player(&path);
    std::fs::remove_file(&path).expect("remove the recording");

    assert!(status.success(), "a live publisher does not refuse: {stderr}");
    let warning =
        format!("hypha play: {TOPIC} already has a live publisher; replayed data will mix with it");
    assert_eq!(stderr.matches(&warning).count(), 1, "one warning for the topic: {stderr}");
    assert!(stdout.contains("published 3 messages"), "the replay ran: {stdout}");
}
