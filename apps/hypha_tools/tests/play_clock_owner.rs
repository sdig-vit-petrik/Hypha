//! A `hypha play --clock` is refused while another process publishes `/clock`.
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
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

const START: &str = "2026-10-08T12:00:00Z";
const DATA_TOPIC: &str = "/owner_data";
const CLOCK_TOPIC: &str = "/clock";
const CLOCK_TYPE: &str = "rosgraph_msgs/Clock";
const CLOCK_DEFINITION: &str = "time clock\n";
/// The longest a refused or a short replay may take to exit.
const EXIT_LIMIT: Duration = Duration::from_secs(10);

/// Writes three messages on [`DATA_TOPIC`], spread over 300 ms of log time.
fn write_recording(path: &Path) {
    let start = hypha_tools::utc_time::parse_utc_nanos(START).expect("the UTC start");
    let mut writer = Writer::with_options(
        File::create(path).expect("the recording file"),
        WriteOptions::new().profile("ros1"),
    )
    .expect("a writer");
    let schema = writer.add_schema("std_msgs/UInt8", "ros1msg", b"uint8 data\n").expect("schema");
    let md5 =
        hypha_node::sdk::md5::md5_from_definition("std_msgs/UInt8", "uint8 data\n").expect("MD5");
    let metadata = BTreeMap::from([("md5sum".to_string(), md5)]);
    let channel = writer.add_channel(schema, DATA_TOPIC, "ros1", &metadata).expect("channel");
    for (sequence, offset_millis) in [0_u64, 100, 200].into_iter().enumerate() {
        let time = start
            .checked_add(offset_millis.checked_mul(1_000_000).expect("a short offset"))
            .expect("a time in the window");
        writer
            .write_to_known_channel(
                &MessageHeader {
                    channel_id: channel,
                    sequence: u32::try_from(sequence).expect("a small sequence"),
                    log_time: time,
                    publish_time: time,
                },
                &[u8::try_from(sequence).expect("a small value")],
            )
            .expect("message");
    }
    writer.finish().expect("finish");
    drop(writer);
}

fn recording_path(test: &str) -> PathBuf {
    std::env::temp_dir().join(format!("play_clock_owner_{test}_{}.mcap", std::process::id()))
}

/// Starts `hypha play --clock` at max rate with its stderr piped.
fn spawn_player(path: &Path, extra: &[&str]) -> Child {
    Command::new(env!("CARGO_BIN_EXE_hypha"))
        .arg("play")
        .arg("--input")
        .arg(path)
        .args(["--clock", "--start-utc", START, "--rate", "max"])
        .args(extra)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("hypha_play")
}

/// Waits for the child to exit, and kills it when it is still running after [`EXIT_LIMIT`].
fn wait_for_exit(child: &mut Child) -> ExitStatus {
    let began = Instant::now();
    loop {
        if let Some(status) = child.try_wait().expect("the child status") {
            return status;
        }
        if began.elapsed() >= EXIT_LIMIT {
            let _ = child.kill();
            let _ = child.wait();
            panic!("hypha_play did not exit within {EXIT_LIMIT:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn read_stderr(child: &mut Child) -> String {
    let mut text = String::new();
    child.stderr.take().expect("stderr").read_to_string(&mut text).expect("stderr text");
    text
}

#[test]
fn a_second_player_is_refused_while_another_publishes_clock() {
    let path = recording_path("refused");
    write_recording(&path);

    let bus = BusNode::new("clock_owner").expect("the other publisher's bus");
    let md5 = hypha_node::sdk::md5::md5_from_definition(CLOCK_TYPE, CLOCK_DEFINITION).expect("MD5");
    let _clock = bus
        .publish_raw(CLOCK_TOPIC, CLOCK_TYPE, &md5, CLOCK_DEFINITION)
        .expect("the other process's clock");

    let mut child = spawn_player(&path, &[]);
    let status = wait_for_exit(&mut child);
    let stderr = read_stderr(&mut child);
    std::fs::remove_file(&path).expect("remove the recording");

    assert!(!status.success(), "a second player must not run: {stderr}");
    assert!(stderr.contains("already publishes /clock"), "the refusal says why: {stderr}");
}

#[test]
fn a_player_runs_when_no_other_process_publishes_clock() {
    let path = recording_path("alone");
    write_recording(&path);

    let mut child = spawn_player(&path, &["--settle", "0.5"]);
    let status = wait_for_exit(&mut child);
    let stderr = read_stderr(&mut child);
    std::fs::remove_file(&path).expect("remove the recording");

    assert!(status.success(), "a lone player runs: {stderr}");
    assert!(!stderr.contains("already publishes"), "no refusal without a publisher: {stderr}");
}
