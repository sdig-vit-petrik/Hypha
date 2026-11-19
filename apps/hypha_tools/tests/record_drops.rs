//! Recorder drops (a sample it cannot write) and losses (a sample the transport overwrote).
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

// The helpers of `common` that the other replay tests use are unused here.
#[allow(dead_code)]
mod common;

use hypha_node::sdk::BusNode;
use hypha_node::sdk::md5::md5_from_definition;
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

const TOPIC: &str = "/rs_unserved";
const DATA_TYPE: &str = "std_msgs/UInt8";
const DEFINITION: &str = "uint8 data\n";
/// The longest a recorder may take to refuse the topic.
const RUN_LIMIT: Duration = Duration::from_secs(60);
/// Messages published back to back to overrun an observing recorder's queue.
const FLOOD_MESSAGES: usize = 20_000;
/// Payload of each flood message: one byte, the size of the `uint8` type.
const FLOOD_PAYLOAD: [u8; 1] = [7];
/// How long an observer runs after the flood: past one loss-report interval, so the
/// periodic read has run. Timing assumption: the flood overruns the queue within
/// milliseconds, and no recorder drains 20 000 messages before its first sleep.
const OBSERVE_AFTER_FLOOD: Duration = Duration::from_secs(11);

fn output_path(test: &str) -> PathBuf {
    std::env::temp_dir().join(format!("record_drops_{test}_{}.mcap", std::process::id()))
}

/// Reads the recorder's output until it says it follows the topic. False when it ends first.
fn wait_until_following(stdout: &mut BufReader<ChildStdout>) -> bool {
    loop {
        let mut line = String::new();
        match stdout.read_line(&mut line) {
            Ok(0) | Err(_) => return false,
            Ok(_) if line.starts_with("Following") => return true,
            Ok(_) => {}
        }
    }
}

fn spawn_recorder(output: &Path, extra: &[&str]) -> (common::Killed, BufReader<ChildStdout>) {
    let mut recorder = Command::new(env!("CARGO_BIN_EXE_hypha"))
        .arg("record")
        .args(["--topics", TOPIC, "--compression", "none"])
        .args(extra)
        .arg("-o")
        .arg(output)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("hypha_record");
    let mut stdout = BufReader::new(recorder.stdout.take().expect("the recorder's stdout"));
    if !wait_until_following(&mut stdout) {
        let _ = recorder.kill();
        let _ = recorder.wait();
        panic!("hypha_record exited before it followed {TOPIC}");
    }
    (common::Killed(recorder), stdout)
}

fn publisher(bus: &BusNode) -> hypha_node::sdk::RawPublisher {
    let md5 = md5_from_definition(DATA_TYPE, DEFINITION).expect("MD5");
    bus.publish_raw(TOPIC, DATA_TYPE, &md5, "").expect("a publisher")
}

fn stderr_text(child: &mut Child) -> String {
    let mut text = String::new();
    child.stderr.take().expect("stderr").read_to_string(&mut text).expect("stderr text");
    text
}

/// Counts the messages of `topic` in a finished MCAP file. The file must read to its end.
fn count_on(path: &Path, topic: &str) -> usize {
    let bytes = std::fs::read(path).expect("the recording");
    let mut count = 0_usize;
    for message in mcap::MessageStream::new(&bytes).expect("an MCAP reader") {
        let message = message.expect("a valid message");
        if message.channel.topic == topic {
            count = count.saturating_add(1);
        }
    }
    count
}

fn stop(recorder: &mut Child) -> ExitStatus {
    let pid = Pid::from_raw(i32::try_from(recorder.id()).expect("a process id"));
    kill(pid, Signal::SIGTERM).expect("signal the recorder");
    recorder.wait().expect("the recorder's status")
}

#[test]
fn a_sync_recorder_refuses_the_first_sample_it_cannot_record() {
    let bus = BusNode::new("drops_publisher").expect("a bus node");
    let mut output = publisher(&bus);
    let path = output_path("refuse");
    let (mut recorder, _stdout) = spawn_recorder(&path, &["--sync"]);

    let began = Instant::now();
    let status = loop {
        assert!(output.publish(&[1]), "a message on {TOPIC}");
        if let Some(status) = recorder.try_wait().expect("the recorder's status") {
            break status;
        }
        assert!(began.elapsed() < RUN_LIMIT, "the recorder did not refuse {TOPIC} in time");
        std::thread::sleep(Duration::from_millis(20));
    };
    let stderr = stderr_text(&mut recorder);

    assert!(!status.success(), "a sync recorder that cannot record a sample must fail");
    assert!(
        stderr.contains(&format!("cannot record '{TOPIC}'")),
        "stderr must name {TOPIC}: {stderr}"
    );
    assert_eq!(count_on(&path, TOPIC), 0, "the refused sample is not written");
    std::fs::remove_file(&path).expect("remove the recording");
}

#[test]
fn a_recorder_without_sync_warns_once_per_topic_and_goes_on() {
    let bus = BusNode::new("drops_observer").expect("a bus node");
    let mut output = publisher(&bus);
    let path = output_path("observe");
    let (mut recorder, _stdout) = spawn_recorder(&path, &[]);

    for _ in 0..10 {
        assert!(output.publish(&[1]), "a message on {TOPIC}");
        std::thread::sleep(Duration::from_millis(30));
    }
    let status = stop(&mut recorder);
    let stderr = stderr_text(&mut recorder);

    assert!(status.success(), "an observer keeps recording past a dropped sample: {stderr}");
    let warnings = stderr.matches(&format!("cannot record '{TOPIC}'")).count();
    assert_eq!(warnings, 1, "one warning per topic: {stderr}");
    std::fs::remove_file(&path).expect("remove the recording");
}

/// A publisher that serves the schema, so the recorder can write what it reads.
fn publisher_with_schema(bus: &BusNode) -> hypha_node::sdk::RawPublisher {
    let md5 = md5_from_definition(DATA_TYPE, DEFINITION).expect("MD5");
    bus.publish_raw(TOPIC, DATA_TYPE, &md5, DEFINITION).expect("a publisher")
}

#[test]
fn an_observer_that_loses_samples_reports_them_once_and_exits_one() {
    let bus = BusNode::new("drops_flood").expect("a bus node");
    let mut output = publisher_with_schema(&bus);
    let path = output_path("loss");
    let (mut recorder, mut stdout) = spawn_recorder(&path, &[]);

    let mut accepted = 0_usize;
    for _ in 0..FLOOD_MESSAGES {
        if output.publish(&FLOOD_PAYLOAD) {
            accepted = accepted.saturating_add(1);
        }
    }
    assert!(accepted > FLOOD_MESSAGES / 2, "the flood reached {TOPIC}: {accepted} accepted");
    std::thread::sleep(OBSERVE_AFTER_FLOOD);
    let status = stop(&mut recorder);
    let stderr = stderr_text(&mut recorder);
    let mut summary = String::new();
    stdout.read_to_string(&mut summary).expect("the recorder's summary");
    std::fs::remove_file(&path).expect("remove the recording");

    assert_eq!(status.code(), Some(1), "a recording that lost samples fails: {stderr}");
    let reports = stderr.matches(&format!("hypha record: {TOPIC}: ")).count();
    assert_eq!(reports, 1, "one loss report for the topic: {stderr}");
    assert!(stderr.contains("samples lost"), "the report says what was lost: {stderr}");
    let summary_line = format!("  {TOPIC} lost ");
    assert!(summary.contains(&summary_line), "the summary lists the topic: {summary}");
}
