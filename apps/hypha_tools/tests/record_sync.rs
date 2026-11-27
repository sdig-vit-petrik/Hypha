//! `hypha record --sync` against a real player and a real recorder process.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod common;

use common::Responder;
use hypha_node::sdk::clock_barrier::PROGRESS_DEPTH;
use hypha_node::sdk::md5::md5_from_definition;
use hypha_node::sdk::{BusNode, PublisherOptions, RawPublisher, RawSubscriber, WaitSet};
use hypha_tools::replay_sync::{PROGRESS_TOPIC, Progress, decode_progress};
use hypha_tools::utc_time::parse_utc_nanos;
use mcap::records::MessageHeader;
use mcap::{WriteOptions, Writer};
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

const PARTICIPANT: &str = "rs_node";
/// The name the recorder gives its own node.
const RECORDER: &str = "hypha_record";
const START: &str = "2026-10-08T12:00:00Z";
const INPUT_TOPIC: &str = "/rs_in";
const OUTPUT_TOPIC: &str = "/rs_out";
const OUTPUT_TYPE: &str = "std_msgs/UInt8";
const OUTPUT_DEFINITION: &str = "uint8 data\n";
/// Messages the participant publishes on [`OUTPUT_TOPIC`] for each new clock value.
const PER_VALUE: u8 = 3;
/// Messages published at once for each new clock value, more than the recorder's queue
/// of 32 holds.
const BURST: u8 = 64;
/// The longest any of these replays may take.
const RUN_LIMIT: Duration = Duration::from_secs(60);

/// An [`OUTPUT_TYPE`] message is one byte, so its pool fits a container's `/dev/shm`.
fn output_options() -> PublisherOptions {
    PublisherOptions::default().bounded(1)
}

fn recording_path(test: &str) -> PathBuf {
    std::env::temp_dir().join(format!("record_sync_{test}_{}.mcap", std::process::id()))
}

fn start_nanos() -> u64 {
    parse_utc_nanos(START).expect("the UTC start")
}

/// Writes 200 messages on [`INPUT_TOPIC`], one every 10 ms for 2 s, for the player to replay.
fn write_input(path: &Path) {
    let start = start_nanos();
    let mut writer = Writer::with_options(
        File::create(path).expect("the input file"),
        WriteOptions::new().profile("ros1"),
    )
    .expect("a writer");
    let schema =
        writer.add_schema(OUTPUT_TYPE, "ros1msg", OUTPUT_DEFINITION.as_bytes()).expect("schema");
    let md5 = md5_from_definition(OUTPUT_TYPE, OUTPUT_DEFINITION).expect("MD5");
    let metadata = BTreeMap::from([("md5sum".to_string(), md5)]);
    let channel = writer.add_channel(schema, INPUT_TOPIC, "ros1", &metadata).expect("channel");
    for sequence in 0_u32..200 {
        let time = start
            .checked_add(u64::from(sequence).checked_mul(10_000_000).expect("a time in the window"))
            .expect("a time in the window");
        let value = u8::try_from(sequence % 256).expect("a byte");
        writer
            .write_to_known_channel(
                &MessageHeader {
                    channel_id: channel,
                    sequence,
                    log_time: time,
                    publish_time: time,
                },
                &[value],
            )
            .expect("message");
    }
    writer.finish().expect("finish");
    drop(writer);
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

/// Starts `hypha record --sync` on [`OUTPUT_TOPIC`], with `extra` arguments, and returns
/// once it has subscribed.
fn spawn_recorder(output: &Path, extra: &[&str]) -> (common::Killed, BufReader<ChildStdout>) {
    let mut recorder = Command::new(env!("CARGO_BIN_EXE_hypha"))
        .arg("record")
        .args(["--sync", "--topics", OUTPUT_TOPIC, "--compression", "none"])
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
        panic!("hypha_record exited before it followed {OUTPUT_TOPIC}");
    }
    (common::Killed(recorder), stdout)
}

/// Returns once another process subscribes to `topic`. Panics after [`RUN_LIMIT`].
fn wait_until_subscribed(bus: &BusNode, topic: &str) {
    let began = Instant::now();
    loop {
        let topics = bus.list_topics().expect("the topic list");
        let mut subscribed = false;
        for info in &topics {
            let topic_has_subscriber = info.name == topic && info.subscribed;
            if topic_has_subscriber {
                subscribed = true;
            }
        }
        if subscribed {
            return;
        }
        assert!(began.elapsed() < RUN_LIMIT, "nothing subscribed to {topic} within {RUN_LIMIT:?}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn spawn_player(input: &Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_hypha"))
        .arg("play")
        .arg("--input")
        .arg(input)
        .args(["--clock", "--start-utc", START, "--nodes", PARTICIPANT, "--rate", "max"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("hypha_play")
}

fn drain_reports(reports: &mut RawSubscriber, into: &mut Vec<Progress>) {
    while let Some(sample) = reports.try_receive().expect("a report receive") {
        let bytes = sample.bytes().expect("a report payload");
        into.push(decode_progress(bytes).expect("a valid report"));
    }
}

/// What the participant did while the watched child ran.
struct Served {
    status: ExitStatus,
    /// The distinct clock values the participant published messages for.
    values: BTreeSet<u64>,
}

/// Process id of `child`, for a signal.
fn pid_of(child: &Child) -> Pid {
    Pid::from_raw(i32::try_from(child.id()).expect("a process id"))
}

/// What the participant publishes for each new clock value.
struct Burst {
    /// Messages published on [`OUTPUT_TOPIC`].
    count: u8,
    /// A process stopped while the burst is published, so it reads none of it before
    /// the burst is complete, however slowly this host publishes.
    frozen: Option<Pid>,
}

/// Answers `/clock` as `rs_node` until `watched` exits or [`RUN_LIMIT`] passes. Each new
fn serve(
    watched: &mut Child,
    waitset: &mut WaitSet<'_>,
    responder: &mut Responder<'_>,
    output: &mut RawPublisher,
    reports: &mut RawSubscriber,
    seen: &mut Vec<Progress>,
    burst: &Burst,
) -> Served {
    let began = Instant::now();
    let mut values = BTreeSet::new();
    loop {
        let exited = watched.try_wait().expect("the child's status");
        if let Some(error) = waitset.run_for(Duration::from_millis(1)).err() {
            panic!("the participant's executor failed: {error}");
        }
        drain_reports(reports, seen);
        for sample in responder.receive() {
            if values.insert(sample.nanos) {
                if let Some(pid) = burst.frozen {
                    kill(pid, Signal::SIGSTOP).expect("stop the reader");
                }
                for value in 0..burst.count {
                    assert!(output.publish(&[value]), "a message on {OUTPUT_TOPIC}");
                }
                if let Some(pid) = burst.frozen {
                    kill(pid, Signal::SIGCONT).expect("resume the reader");
                }
            }
            responder.answer(sample);
        }
        if let Some(status) = exited {
            drain_reports(reports, seen);
            return Served { status, values };
        }
        if began.elapsed() >= RUN_LIMIT {
            let _ = watched.kill();
            let _ = watched.wait();
            panic!("the replay did not finish within {RUN_LIMIT:?}");
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Reads and returns the messages of an MCAP file, as (topic, log time) pairs.
fn read_messages(path: &Path) -> Vec<(String, u64)> {
    let bytes = std::fs::read(path).expect("the recording");
    let mut messages = Vec::new();
    for message in mcap::MessageStream::new(&bytes).expect("an MCAP reader") {
        let message = message.expect("a valid message");
        messages.push((message.channel.topic.clone(), message.log_time));
    }
    messages
}

fn stderr_text(child: &mut Child) -> String {
    let mut text = String::new();
    child.stderr.take().expect("stderr").read_to_string(&mut text).expect("stderr text");
    text
}

/// Stops the recorder the way an operator does, so that it finishes its MCAP file.
fn stop_recorder(recorder: &mut Child) -> ExitStatus {
    kill(pid_of(recorder), Signal::SIGTERM).expect("signal the recorder");
    recorder.wait().expect("the recorder's status")
}

#[test]
fn a_recording_holds_each_chunk_until_it_is_written() {
    let bus = BusNode::new(PARTICIPANT).expect("the participant's bus");
    let md5 = md5_from_definition(OUTPUT_TYPE, OUTPUT_DEFINITION).expect("MD5");
    let mut output = bus
        .publish_raw_configured(
            OUTPUT_TOPIC,
            OUTPUT_TYPE,
            &md5,
            OUTPUT_DEFINITION,
            output_options(),
        )
        .expect("an output publisher");
    let placeholder = Responder::new(&bus, "placeholder");
    let mut responder = Responder::new(&bus, PARTICIPANT);
    let mut waitset = WaitSet::new(&bus).expect("the participant's executor");
    let mut reports = bus.subscribe_raw(PROGRESS_TOPIC, PROGRESS_DEPTH).expect("reports");
    // A reader takes the queued reports of several publishers in the order of its
    // connection slots, not of their sends (#92). Freeing the first slot gives the
    // recorder's publisher the one read first, so no check below may lean on that order.
    let _ = reports.try_receive().expect("a first receive");
    drop(placeholder);
    let _ = reports.try_receive().expect("a second receive");
    let input = recording_path("input_chunks");
    write_input(&input);
    let output_path = recording_path("output_chunks");

    // The recorder names the participant, so every chunk, the first included, is
    // acknowledged only after the participant's answer to it.
    let (mut recorder, mut recorder_out) = spawn_recorder(&output_path, &["--nodes", PARTICIPANT]);
    let mut player = spawn_player(&input);
    let mut seen = Vec::new();
    let served = serve(
        &mut player,
        &mut waitset,
        &mut responder,
        &mut output,
        &mut reports,
        &mut seen,
        &Burst { count: PER_VALUE, frozen: None },
    );
    let player_stderr = stderr_text(&mut player);
    assert!(served.status.success(), "the replay failed: {player_stderr}");
    assert!(served.values.len() > 1, "the replay barriered on several chunks");

    let recorder_status = stop_recorder(&mut recorder);
    let mut recorder_text = String::new();
    recorder_out.read_to_string(&mut recorder_text).expect("the recorder's output");
    let recorder_stderr = stderr_text(&mut recorder);
    assert!(
        recorder_status.success(),
        "hypha_record failed ({recorder_status}): {recorder_text}{recorder_stderr}\nvalues {} player: {player_stderr}",
        served.values.len()
    );

    let written =
        read_messages(&output_path).iter().filter(|(topic, _)| topic == OUTPUT_TOPIC).count();
    let expected = served.values.len().saturating_mul(usize::from(PER_VALUE));
    assert_eq!(
        written, expected,
        "every message of every chunk is recorded: {recorder_text} {player_stderr}"
    );

    // The recorder acknowledges every chunk, the first included, only after the
    // participant answered the same /clock sample: for each report of the recorder, the
    // participant reported that clock or a later one for a sample sent at or after it.
    // This is what the protocol promises; the order in which this reader received the
    // two reports is not (see the slot comment above).
    for value in &served.values {
        let acknowledged = seen
            .iter()
            .find(|report| report.node == RECORDER && report.clock_nanos == *value)
            .expect("the recorder reported the chunk");
        let answered = seen.iter().any(|report| {
            report.node == PARTICIPANT
                && report.clock_nanos >= acknowledged.clock_nanos
                && report.clock_sent_nanos >= acknowledged.clock_sent_nanos
        });
        assert!(answered, "the recorder acknowledged {value} before the participant answered it");
    }
    std::fs::remove_file(&output_path).expect("remove the recording");
    std::fs::remove_file(&input).expect("remove the input");
}

#[test]
fn a_recorder_that_loses_samples_stops_and_names_the_topic() {
    let bus = BusNode::new(PARTICIPANT).expect("the participant's bus");
    let md5 = md5_from_definition(OUTPUT_TYPE, OUTPUT_DEFINITION).expect("MD5");
    let mut output = bus
        .publish_raw_configured(
            OUTPUT_TOPIC,
            OUTPUT_TYPE,
            &md5,
            OUTPUT_DEFINITION,
            output_options(),
        )
        .expect("an output publisher");
    let mut responder = Responder::new(&bus, PARTICIPANT);
    let mut waitset = WaitSet::new(&bus).expect("the participant's executor");
    let mut reports = bus.subscribe_raw(PROGRESS_TOPIC, PROGRESS_DEPTH).expect("reports");
    let input = recording_path("input_overflow");
    write_input(&input);
    let output_path = recording_path("output_overflow");

    let (mut recorder, mut recorder_out) = spawn_recorder(&output_path, &[]);
    let mut player = spawn_player(&input);
    let mut seen = Vec::new();
    // The recorder drains its queue between chunks too: frozen during each burst, it
    // cannot keep up with a publisher slowed by a loaded or instrumented host.
    let recorder_pid = pid_of(&recorder);
    let served = serve(
        &mut recorder,
        &mut waitset,
        &mut responder,
        &mut output,
        &mut reports,
        &mut seen,
        &Burst { count: BURST, frozen: Some(recorder_pid) },
    );
    let _ = player.kill();
    let _ = player.wait();
    let mut recorder_text = String::new();
    recorder_out.read_to_string(&mut recorder_text).expect("the recorder's output");
    let recorder_stderr = stderr_text(&mut recorder);

    assert!(!served.status.success(), "a recorder that lost samples must fail");
    assert!(
        recorder_stderr.contains(OUTPUT_TOPIC) && recorder_stderr.contains("lost"),
        "stderr must name {OUTPUT_TOPIC}: {recorder_stderr}"
    );
    // The file is finished even on this path, so it reads to the end.
    let _ = read_messages(&output_path);
    std::fs::remove_file(&output_path).expect("remove the recording");
    std::fs::remove_file(&input).expect("remove the input");
}

#[test]
fn a_recorder_started_before_its_topic_records_every_message() {
    let bus = BusNode::new(PARTICIPANT).expect("the participant's bus");
    let md5 = md5_from_definition(OUTPUT_TYPE, OUTPUT_DEFINITION).expect("MD5");
    let mut responder = Responder::new(&bus, PARTICIPANT);
    let mut waitset = WaitSet::new(&bus).expect("the participant's executor");
    let mut reports = bus.subscribe_raw(PROGRESS_TOPIC, PROGRESS_DEPTH).expect("reports");
    let input = recording_path("input_late_topic");
    write_input(&input);
    let output_path = recording_path("output_late_topic");

    // The recorder and the player run before the topic has a publisher. The recorder
    // announces itself, so the player waits for it, and the recorder answers no chunk
    // until it follows the topic.
    let (mut recorder, mut recorder_out) = spawn_recorder(&output_path, &[]);
    let mut player = spawn_player(&input);
    std::thread::sleep(Duration::from_millis(600));
    let mut output = bus
        .publish_raw_configured(
            OUTPUT_TOPIC,
            OUTPUT_TYPE,
            &md5,
            OUTPUT_DEFINITION,
            output_options(),
        )
        .expect("an output publisher");
    // The node publishes on the start `/clock` before the recorder answers it, so it
    // waits until the recorder subscribes.
    wait_until_subscribed(&bus, OUTPUT_TOPIC);
    let mut seen = Vec::new();
    let served = serve(
        &mut player,
        &mut waitset,
        &mut responder,
        &mut output,
        &mut reports,
        &mut seen,
        &Burst { count: PER_VALUE, frozen: None },
    );
    let player_stderr = stderr_text(&mut player);
    assert!(served.status.success(), "the replay failed: {player_stderr}");

    let recorder_status = stop_recorder(&mut recorder);
    let mut recorder_text = String::new();
    recorder_out.read_to_string(&mut recorder_text).expect("the recorder's output");
    let recorder_stderr = stderr_text(&mut recorder);
    assert!(
        recorder_status.success(),
        "hypha_record failed ({recorder_status}): {recorder_text}{recorder_stderr}\nvalues {} player: {player_stderr}",
        served.values.len()
    );

    let written =
        read_messages(&output_path).iter().filter(|(topic, _)| topic == OUTPUT_TOPIC).count();
    let expected = served.values.len().saturating_mul(usize::from(PER_VALUE));
    assert_eq!(written, expected, "{recorder_text} {player_stderr}");
    assert!(
        seen.iter().any(|report| report.node == RECORDER && report.clock_sent_nanos == 0),
        "the recorder announced itself before it answered a sample"
    );
    std::fs::remove_file(&output_path).expect("remove the recording");
    std::fs::remove_file(&input).expect("remove the input");
}

/// A stream that a node creates in the middle of a chunk is followed only when the recorder
/// answers the chunk, so what it sent first is not in the recording: `--sync` fails, naming it,
/// instead of finishing a recording that lacks it (H2b of the fourth review of #268).
#[test]
fn a_stream_that_appears_during_a_chunk_fails_the_sync_recording() {
    let bus = BusNode::new(PARTICIPANT).expect("the participant's bus");
    let md5 = md5_from_definition(OUTPUT_TYPE, OUTPUT_DEFINITION).expect("MD5");
    let mut output = bus
        .publish_raw_configured(
            OUTPUT_TOPIC,
            OUTPUT_TYPE,
            &md5,
            OUTPUT_DEFINITION,
            output_options(),
        )
        .expect("an output publisher");
    let late_definition = "uint8 data\nuint8 extra\n";
    let late_md5 = md5_from_definition(OUTPUT_TYPE, late_definition).expect("MD5");
    let mut responder = Responder::new(&bus, PARTICIPANT);
    let mut waitset = WaitSet::new(&bus).expect("the participant's executor");
    let input = recording_path("input_late_stream");
    write_input(&input);
    let output_path = recording_path("output_late_stream");
    let (mut recorder, _recorder_out) = spawn_recorder(&output_path, &["--nodes", PARTICIPANT]);
    let mut player = spawn_player(&input);

    let began = Instant::now();
    let mut values = BTreeSet::new();
    let mut late = None;
    let status = loop {
        let exited = recorder.try_wait().expect("the recorder's status");
        if let Some(error) = waitset.run_for(Duration::from_millis(1)).err() {
            panic!("the participant's executor failed: {error}");
        }
        for sample in responder.receive() {
            if values.insert(sample.nanos) {
                if values.len() == 3 {
                    // The reader is stopped while the new stream is made and sends, so that it
                    // cannot follow the stream before its first frames are out.
                    let pid = pid_of(&recorder);
                    kill(pid, Signal::SIGSTOP).expect("stop the recorder");
                    let mut stream = bus
                        .publish_raw_configured(
                            OUTPUT_TOPIC,
                            OUTPUT_TYPE,
                            &late_md5,
                            late_definition,
                            output_options(),
                        )
                        .expect("a stream that joins the name");
                    for value in 0..PER_VALUE {
                        assert!(stream.publish(&[value, 0]), "a message on the new stream");
                    }
                    kill(pid, Signal::SIGCONT).expect("resume the recorder");
                    late = Some(stream);
                } else if let Some(stream) = late.as_mut() {
                    for value in 0..PER_VALUE {
                        assert!(stream.publish(&[value, 0]), "a message on the new stream");
                    }
                }
                for value in 0..PER_VALUE {
                    assert!(output.publish(&[value]), "a message on {OUTPUT_TOPIC}");
                }
            }
            responder.answer(sample);
        }
        if let Some(status) = exited {
            break status;
        }
        assert!(began.elapsed() < RUN_LIMIT, "the recorder did not end");
        std::thread::sleep(Duration::from_millis(1));
    };
    let _ = player.kill();
    let _ = player.wait();
    let recorder_stderr = stderr_text(&mut recorder);
    drop(late);

    assert!(!status.success(), "a recording that lacks samples is no success: {recorder_stderr}");
    assert!(
        recorder_stderr.contains("--sync")
            && recorder_stderr.contains("before it was followed")
            && recorder_stderr.contains(OUTPUT_TOPIC),
        "{recorder_stderr}"
    );
    let _ = std::fs::remove_file(&output_path);
    std::fs::remove_file(&input).expect("remove the input");
}

/// A stream that exists and refuses the recorder's subscription (its observer slots are taken
/// by other tools) fails a `--sync` recording, naming the stream and the limit; outside `--sync`
/// the recorder says so and records its other topics (H2a and L2).
#[test]
fn a_refused_subscription_fails_the_sync_recording_and_not_the_other_recordings() {
    let bus = BusNode::new(PARTICIPANT).expect("the participant's bus");
    let md5 = md5_from_definition(OUTPUT_TYPE, OUTPUT_DEFINITION).expect("MD5");
    let _output = bus
        .publish_raw_configured(
            OUTPUT_TOPIC,
            OUTPUT_TYPE,
            &md5,
            OUTPUT_DEFINITION,
            output_options(),
        )
        .expect("an output publisher");
    let other_topic = "/rs_other";
    let mut other = bus
        .publish_raw_configured(other_topic, OUTPUT_TYPE, &md5, OUTPUT_DEFINITION, output_options())
        .expect("another output");
    let hash = hypha_node::sdk::schema_hash_from_md5(&md5).expect("hash");
    let tools = BusNode::new("rs_other_tools").expect("other tools");
    let observers: Vec<_> = (0..hypha_node::sdk::UNDECLARED_SUBSCRIBER_SLOTS)
        .map(|_| {
            tools
                .subscribe_raw_stream(
                    OUTPUT_TOPIC,
                    hash,
                    hypha_node::sdk::SubscriberOptions::default(),
                )
                .expect("an observer")
        })
        .collect();

    // --sync: it fails at once, naming the stream and the limit.
    let sync_path = recording_path("output_refused_sync");
    let output = Command::new(env!("CARGO_BIN_EXE_hypha"))
        .args(["record", "--sync", "--topics", OUTPUT_TOPIC, "--compression", "none", "-o"])
        .arg(&sync_path)
        .output()
        .expect("hypha record --sync");
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{text}");
    assert!(
        text.contains(OUTPUT_TOPIC) && text.contains("observer slots") && text.contains("--sync"),
        "{text}"
    );
    assert!(!text.contains("cannot list"), "a refusal is not a listing failure: {text}");

    // Without --sync: the refusal is told, and the other topic is recorded.
    let path = recording_path("output_refused");
    let mut recorder = Command::new(env!("CARGO_BIN_EXE_hypha"))
        .args(["record", "--topics", OUTPUT_TOPIC, other_topic, "--compression", "none", "-o"])
        .arg(&path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("hypha record");
    let mut recorder_out = BufReader::new(recorder.stdout.take().expect("stdout"));
    assert!(wait_until_following(&mut recorder_out));
    let mut recorder = common::Killed(recorder);
    let began = Instant::now();
    while began.elapsed() < Duration::from_millis(1500) {
        assert!(other.publish(&[1]));
        std::thread::sleep(Duration::from_millis(20));
    }
    let status = stop_recorder(&mut recorder);
    let errors = stderr_text(&mut recorder);
    drop(observers);
    assert!(status.success(), "{errors}");
    assert!(errors.contains("observer slots") && errors.contains(OUTPUT_TOPIC), "{errors}");
    let messages = read_messages(&path);
    assert!(
        messages.iter().any(|(topic, _)| topic == other_topic),
        "the other topic: {messages:?}"
    );
    std::fs::remove_file(&path).expect("remove the recording");
}
