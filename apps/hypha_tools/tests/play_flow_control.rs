//! The `/clock` flow control of `hypha play`, against a real player process.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod common;

use common::{ClockSample, Responder};
use hypha_node::sdk::{BusNode, RawSubscriber};
use hypha_tools::utc_time::parse_utc_nanos;
use mcap::records::MessageHeader;
use mcap::{WriteOptions, Writer};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// The node name of the participant, which the player is told to wait for.
const PARTICIPANT: &str = "flow_participant";
const START: &str = "2026-10-08T12:00:00Z";
const DATA_TOPIC: &str = "/flow_data";
/// The longest any of these replays may take.
const RUN_LIMIT: Duration = Duration::from_secs(30);

/// Messages as (offset from the start in milliseconds, value). With the default 100 ms
/// chunk they fall in the chunks [0, 100], (100, 200], (200, 300] and (400, 500].
const MESSAGES: [(u64, u8); 4] = [(10, 1), (150, 2), (250, 3), (420, 4)];

/// How the participant answers `/clock`.
#[derive(Clone, Copy)]
enum Policy {
    /// Every sample is answered as it arrives.
    Always,
    /// Samples are answered only after this long, so the player must wait.
    Withhold(Duration),
    /// Only the first `after` answers are sent, then the node goes quiet.
    Silent { after: usize },
    /// The first `after` answers are full; every later sample is answered with the clock
    /// of the last full one, as a node that holds a deadline for an upstream reports.
    Hold { after: usize },
}

impl Policy {
    fn answers(self, first_seen: Option<Instant>, answered: usize, now: Instant) -> bool {
        match self {
            Self::Always | Self::Hold { .. } => true,
            Self::Withhold(delay) => {
                first_seen.is_some_and(|seen| now.saturating_duration_since(seen) >= delay)
            }
            Self::Silent { after } => answered < after,
        }
    }

    /// The clock value an answer to `sample` reports, given the last one reported.
    fn clock_of(self, sample: ClockSample, answered: usize, last: Option<u64>) -> u64 {
        match (self, last) {
            (Self::Hold { after }, Some(last)) if answered >= after => last,
            _ => sample.nanos,
        }
    }
}

/// What the participant saw of one replay, in the order it arrived.
#[derive(Default)]
struct Trace {
    clocks: Vec<ClockSample>,
    /// Each data message as (value, host send time).
    data: Vec<(u8, u64)>,
}

fn start_nanos() -> u64 {
    parse_utc_nanos(START).expect("the UTC start")
}

fn at(start: u64, offset_millis: u64) -> u64 {
    let nanos = offset_millis.checked_mul(1_000_000).expect("a time in the window");
    start.checked_add(nanos).expect("a time in the window")
}

/// Writes `messages` on [`DATA_TOPIC`] at their offsets from `start`.
fn write_recording(path: &Path, start: u64, messages: &[(u64, u8)]) {
    let on_topic: Vec<(u64, u8, &str)> =
        messages.iter().map(|(offset, value)| (*offset, *value, DATA_TOPIC)).collect();
    write_on_topics(path, start, &on_topic);
}

/// Writes `messages`, as (offset from `start` in milliseconds, value, topic), in order.
fn write_on_topics(path: &Path, start: u64, messages: &[(u64, u8, &str)]) {
    let mut writer = Writer::with_options(
        File::create(path).expect("the recording file"),
        WriteOptions::new().profile("ros1"),
    )
    .expect("a writer");
    let schema = writer.add_schema("std_msgs/UInt8", "ros1msg", b"uint8 data\n").expect("schema");
    let md5 =
        hypha_node::sdk::md5::md5_from_definition("std_msgs/UInt8", "uint8 data\n").expect("MD5");
    let metadata = BTreeMap::from([("md5sum".to_string(), md5)]);
    let mut channels: BTreeMap<&str, u16> = BTreeMap::new();
    for (sequence, (offset, value, topic)) in messages.iter().enumerate() {
        let channel = *channels.entry(topic).or_insert_with(|| {
            writer.add_channel(schema, topic, "ros1", &metadata).expect("channel")
        });
        let time = at(start, *offset);
        writer
            .write_to_known_channel(
                &MessageHeader {
                    channel_id: channel,
                    sequence: u32::try_from(sequence).expect("a small sequence"),
                    log_time: time,
                    publish_time: time,
                },
                &[*value],
            )
            .expect("message");
    }
    writer.finish().expect("finish");
    drop(writer);
}

fn recording_path(test: &str) -> PathBuf {
    std::env::temp_dir().join(format!("play_flow_{test}_{}.mcap", std::process::id()))
}

fn spawn_player(path: &Path, extra: &[&str]) -> Child {
    spawn_player_with_stdout(path, extra, Stdio::null())
}

/// Starts the player with `--clock`, its stdout going to `stdout`.
fn spawn_player_with_stdout(path: &Path, extra: &[&str], stdout: Stdio) -> Child {
    Command::new(env!("CARGO_BIN_EXE_hypha"))
        .arg("play")
        .arg("--input")
        .arg(path)
        .args(["--clock", "--start-utc", START])
        .args(extra)
        .stdout(stdout)
        .stderr(Stdio::piped())
        .spawn()
        .expect("hypha_play")
}

fn drain_data(data: &mut [RawSubscriber], trace: &mut Trace) {
    for subscriber in data {
        while let Some(sample) = subscriber.try_receive().expect("a data receive") {
            let value = *sample.bytes().expect("a payload").first().expect("one byte");
            let sent = sample.identity().expect("an identity").transport_sent_nanos;
            trace.data.push((value, sent));
        }
    }
}

/// The result of [`serve`]: the child's exit status, when it exited first, and the trace.
struct Run {
    status: Option<ExitStatus>,
    trace: Trace,
}

/// Plays the participant until the child exits, `done` says so, or [`RUN_LIMIT`] passes.
fn serve(
    child: &mut Child,
    responder: &mut Responder<'_>,
    data: &mut RawSubscriber,
    policy: Policy,
    done: impl Fn(&Trace) -> bool,
) -> Run {
    serve_topics(child, responder, std::slice::from_mut(data), policy, done)
}

/// [`serve`] for a participant that reads several topics.
fn serve_topics(
    child: &mut Child,
    responder: &mut Responder<'_>,
    data: &mut [RawSubscriber],
    policy: Policy,
    done: impl Fn(&Trace) -> bool,
) -> Run {
    let began = Instant::now();
    let mut trace = Trace::default();
    let mut first_seen = None;
    let mut answered = 0_usize;
    let mut last_reported = None;
    let status = loop {
        let exited = child.try_wait().expect("the child status");
        drain_data(data, &mut trace);
        let received = responder.receive();
        let now = Instant::now();
        if !received.is_empty() {
            first_seen.get_or_insert(now);
            let newest = received.last().copied();
            trace.clocks.extend(received);
            // A newer sample covers the older ones: an answer is "sent at or after".
            if let Some(newest) = newest.filter(|_| policy.answers(first_seen, answered, now)) {
                assert!(
                    answered > 0 || trace.data.is_empty(),
                    "a message went out before any participant answered the clock"
                );
                let nanos = policy.clock_of(newest, answered, last_reported);
                responder.answer(ClockSample { nanos, ..newest });
                last_reported = Some(nanos);
                answered = answered.saturating_add(1);
            }
        }
        if let Some(status) = exited {
            break Some(status);
        }
        if done(&trace) {
            break None;
        }
        if began.elapsed() >= RUN_LIMIT {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the replay did not finish within {RUN_LIMIT:?}");
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    drain_data(data, &mut trace);
    trace.clocks.extend(responder.receive());
    Run { status, trace }
}

/// The distinct clock values in the order they were first sent.
fn clock_values(trace: &Trace) -> Vec<u64> {
    let mut values: Vec<u64> = Vec::new();
    for clock in &trace.clocks {
        if values.last() != Some(&clock.nanos) {
            values.push(clock.nanos);
        }
    }
    values
}

/// The first send time of each clock value.
fn first_sent(trace: &Trace, nanos: u64) -> Option<u64> {
    trace.clocks.iter().filter(|clock| clock.nanos == nanos).map(|clock| clock.sent).min()
}

/// Every message went out, in order, each in its chunk ([`assert_in_its_chunk`]).
fn assert_message_order(trace: &Trace, messages: &[(u64, u8)]) {
    assert_eq!(trace.data.len(), messages.len(), "every message went out");
    for ((value, _), (_, expected)) in trace.data.iter().zip(messages) {
        assert_eq!(value, expected, "messages go out in order");
    }
    assert_in_its_chunk(trace, messages);
}

/// Each message, found by its value, went out after the clock that opened its chunk and
/// before the clock that closes it: a chunk closed at `T` holds every message stamped at
/// or before `T`. The start barrier's clock opens the first chunk, so the messages stamped
/// at the start follow it.
fn assert_in_its_chunk(trace: &Trace, messages: &[(u64, u8)]) {
    let values = clock_values(trace);
    let (start, closes) = values.split_first().expect("the start barrier's clock");
    for (value, sent) in &trace.data {
        let (time, _) = messages.iter().find(|(_, v)| v == value).expect("a known message");
        let opened = closes.iter().filter(|nanos| *nanos < time).max().unwrap_or(start);
        let opened_sent = first_sent(trace, *opened).expect("the opening clock was sent");
        assert!(opened_sent < *sent, "clock {opened} was sent after message {value}");
        if let Some(closing) = closes.iter().find(|nanos| *nanos >= time) {
            let closing_sent = first_sent(trace, *closing).expect("the closing clock was sent");
            assert!(*sent < closing_sent, "message {value} went out after clock {closing}");
        }
    }
}

fn messages_at(start: u64) -> Vec<(u64, u8)> {
    MESSAGES.iter().map(|(offset, value)| (at(start, *offset), *value)).collect()
}

#[test]
fn the_player_waits_for_its_named_node_and_releases_each_chunk_after_its_answer() {
    let start = start_nanos();
    let path = recording_path("waits");
    write_recording(&path, start, &MESSAGES);

    let bus = BusNode::new(PARTICIPANT).expect("the participant's bus");
    // A subscriber that declares no policy never creates the topic, so this test opens it first.
    let data_md5 =
        hypha_node::sdk::md5::md5_from_definition("std_msgs/UInt8", "uint8 data\n").expect("MD5");
    let _data_opener = bus
        .publish_raw(DATA_TOPIC, "std_msgs/UInt8", &data_md5, "uint8 data\n")
        .expect("the data topic's opener");
    let mut data = bus
        .subscribe_raw_with_schema(
            DATA_TOPIC,
            "std_msgs/UInt8",
            "",
            hypha_node::sdk::SubscriberOptions::default().keeping(8),
        )
        .expect("a data subscriber");
    let mut responder = Responder::new(&bus, PARTICIPANT);
    let mut child =
        spawn_player_with_stdout(&path, &["--nodes", PARTICIPANT, "--rate", "max"], Stdio::piped());

    // The node answers only after 300 ms, so no message may go out before that answer.
    let run = serve(
        &mut child,
        &mut responder,
        &mut data,
        Policy::Withhold(Duration::from_millis(300)),
        |_| false,
    );
    let status = run.status.expect("the child exits");
    let mut stderr = String::new();
    child.stderr.take().expect("stderr").read_to_string(&mut stderr).expect("stderr text");
    let mut stdout = String::new();
    child.stdout.take().expect("stdout").read_to_string(&mut stdout).expect("stdout text");
    std::fs::remove_file(&path).expect("remove the recording");
    assert!(status.success(), "the replay failed: {stderr}");
    // The participant answered every barrier, so it is the one that set the rate.
    assert!(stdout.contains("effective rate"), "no rate report: {stdout}");
    assert!(stdout.contains("set by 'flow_participant'"), "wrong limiter: {stdout}");

    let boundaries: Vec<u64> =
        [0, 100, 200, 300, 400, 500].iter().map(|ms| at(start, *ms)).collect();
    assert_eq!(clock_values(&run.trace), boundaries, "one clock per chunk boundary, then the end");
    assert_message_order(&run.trace, &messages_at(start));
}

#[test]
fn a_participant_that_goes_quiet_stops_the_replay_and_is_named() {
    let start = start_nanos();
    let path = recording_path("quiet");
    write_recording(&path, start, &MESSAGES);

    let bus = BusNode::new(PARTICIPANT).expect("the participant's bus");
    // A subscriber that declares no policy never creates the topic, so this test opens it first.
    let data_md5 =
        hypha_node::sdk::md5::md5_from_definition("std_msgs/UInt8", "uint8 data\n").expect("MD5");
    let _data_opener = bus
        .publish_raw(DATA_TOPIC, "std_msgs/UInt8", &data_md5, "uint8 data\n")
        .expect("the data topic's opener");
    let mut data = bus
        .subscribe_raw_with_schema(
            DATA_TOPIC,
            "std_msgs/UInt8",
            "",
            hypha_node::sdk::SubscriberOptions::default().keeping(8),
        )
        .expect("a data subscriber");
    let mut responder = Responder::new(&bus, PARTICIPANT);
    let mut child =
        spawn_player(&path, &["--nodes", PARTICIPANT, "--rate", "max", "--timeout", "1"]);

    let run = serve(&mut child, &mut responder, &mut data, Policy::Silent { after: 2 }, |_| false);
    let status = run.status.expect("the child exits");
    let mut stderr = String::new();
    child.stderr.take().expect("stderr").read_to_string(&mut stderr).expect("stderr text");
    std::fs::remove_file(&path).expect("remove the recording");

    assert!(!status.success(), "a silent participant must fail the replay");
    assert!(stderr.contains(PARTICIPANT), "the error names the participant: {stderr}");
    assert!(stderr.contains("stopped reporting"), "the error says why: {stderr}");
    assert!(stderr.contains("HYPHA_REPLAY_LOCKSTEP=0"), "the error names the open loop: {stderr}");
    let notice = format!("waiting for '{PARTICIPANT}'");
    assert!(stderr.contains(&notice), "the wait was named before the end: {stderr}");
}

#[test]
fn a_participant_that_reports_but_never_advances_stops_the_replay_and_is_named() {
    let start = start_nanos();
    let path = recording_path("held");
    write_recording(&path, start, &MESSAGES);

    let bus = BusNode::new(PARTICIPANT).expect("the participant's bus");
    let (_opener, mut data) = open_data(&bus);
    let mut responder = Responder::new(&bus, PARTICIPANT);
    let mut child =
        spawn_player(&path, &["--nodes", PARTICIPANT, "--rate", "max", "--timeout", "1"]);

    // Every resent /clock is answered, but with the clock of the second answer: the node
    // is never quiet, and never reaches the chunk.
    let run = serve(&mut child, &mut responder, &mut data, Policy::Hold { after: 2 }, |_| false);
    let status = run.status.expect("the child exits");
    let mut stderr = String::new();
    child.stderr.take().expect("stderr").read_to_string(&mut stderr).expect("stderr text");
    std::fs::remove_file(&path).expect("remove the recording");

    assert!(!status.success(), "a participant that never advances must fail the replay");
    assert!(stderr.contains(PARTICIPANT), "the error names the participant: {stderr}");
    assert!(stderr.contains("does not advance"), "the error says why: {stderr}");
    assert!(stderr.contains("HYPHA_REPLAY_LOCKSTEP=0"), "the error names the open loop: {stderr}");
}

#[test]
fn a_loop_restarts_the_participant_after_the_tail_of_its_pass_is_answered() {
    let start = start_nanos();
    let path = recording_path("loop");
    write_recording(&path, start, &MESSAGES);

    let bus = BusNode::new(PARTICIPANT).expect("the participant's bus");
    // A subscriber that declares no policy never creates the topic, so this test opens it first.
    let data_md5 =
        hypha_node::sdk::md5::md5_from_definition("std_msgs/UInt8", "uint8 data\n").expect("MD5");
    let _data_opener = bus
        .publish_raw(DATA_TOPIC, "std_msgs/UInt8", &data_md5, "uint8 data\n")
        .expect("the data topic's opener");
    let mut data = bus
        .subscribe_raw_with_schema(
            DATA_TOPIC,
            "std_msgs/UInt8",
            "",
            hypha_node::sdk::SubscriberOptions::default().keeping(8),
        )
        .expect("a data subscriber");
    let mut responder = Responder::new(&bus, PARTICIPANT);
    // A restarted node takes a while to leave: the player must wait for it.
    responder.leave_delay = Duration::from_millis(500);
    let mut child = spawn_player(&path, &["--nodes", PARTICIPANT, "--rate", "max", "--loop"]);

    let wanted = 2 * MESSAGES.len();
    let run = serve(&mut child, &mut responder, &mut data, Policy::Always, |trace| {
        trace.data.len() >= wanted
    });
    assert!(run.status.is_none(), "the loop keeps playing until stopped");
    let _ = child.kill();
    let _ = child.wait();
    std::fs::remove_file(&path).expect("remove the recording");

    let trace = run.trace;
    // The rewind is the first sample of the start clock after the first tail sample of pass one.
    let tail = at(start, 500);
    let first_tail =
        trace.clocks.iter().position(|clock| clock.nanos == tail).expect("a tail clock");
    let rewind = trace
        .clocks
        .get(first_tail..)
        .expect("the tail is in the trace")
        .iter()
        .find(|clock| clock.nanos == start)
        .expect("the pass starts again")
        .sent;
    let tail_sent = first_sent(&trace, tail).expect("the pass ends with its last clock");
    assert!(tail_sent < rewind, "the rewind came before the tail of the pass was sent");
    // The participant restarts: the player sends nothing more until it has left the bus.
    let left = *responder.left_at.first().expect("the participant left for the restart");
    let next = trace.clocks.iter().find(|clock| clock.sent > rewind).expect("the pass goes on");
    assert!(next.sent > left, "a /clock went out before the participant left the bus");
    assert_eq!(trace.data.len(), wanted, "two full passes went out");
    for (value, sent) in trace.data.iter().take(MESSAGES.len()) {
        assert!(*sent < rewind, "pass one message {value} went out after the rewind");
    }
    for (value, sent) in trace.data.iter().skip(MESSAGES.len()) {
        assert!(*sent > rewind, "pass two message {value} went out before the rewind");
    }
}

#[test]
fn without_clock_nothing_waits_for_a_participant_and_no_clock_is_published() {
    let start = start_nanos();
    let path = recording_path("no_clock");
    write_recording(&path, start, &MESSAGES);

    let bus = BusNode::new(PARTICIPANT).expect("the observer's bus");
    // A subscriber that declares no policy never creates the topic, so this test opens it first.
    let data_md5 =
        hypha_node::sdk::md5::md5_from_definition("std_msgs/UInt8", "uint8 data\n").expect("MD5");
    let _data_opener = bus
        .publish_raw(DATA_TOPIC, "std_msgs/UInt8", &data_md5, "uint8 data\n")
        .expect("the data topic's opener");
    let mut data = bus
        .subscribe_raw_with_schema(
            DATA_TOPIC,
            "std_msgs/UInt8",
            "",
            hypha_node::sdk::SubscriberOptions::default().keeping(8),
        )
        .expect("a data subscriber");
    let mut responder = Responder::new(&bus, PARTICIPANT);
    let mut child = Command::new(env!("CARGO_BIN_EXE_hypha"))
        .arg("play")
        .arg("--input")
        .arg(&path)
        .args(["--start-utc", START, "--rate", "max"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("hypha_play");

    let run = serve(&mut child, &mut responder, &mut data, Policy::Always, |_| false);
    let status = run.status.expect("the child exits");
    std::fs::remove_file(&path).expect("remove the recording");
    assert!(status.success(), "the replay without --clock succeeds");
    assert!(run.trace.clocks.is_empty(), "no /clock without --clock");
    assert_message_order_values(&run.trace, &messages_at(start));
}

/// The values of the messages, in order, without the clock ordering check.
fn assert_message_order_values(trace: &Trace, messages: &[(u64, u8)]) {
    let values: Vec<u8> = trace.data.iter().map(|(value, _)| *value).collect();
    let expected: Vec<u8> = messages.iter().map(|(_, value)| *value).collect();
    assert_eq!(values, expected, "every message went out in order");
}

#[test]
fn a_message_on_a_chunk_boundary_goes_out_before_the_clock_that_closes_it() {
    let start = start_nanos();
    // At the start, twice at one boundary, and last on the grid.
    let messages = [(0, 1), (100, 2), (100, 3), (200, 4), (300, 5)];
    let path = recording_path("boundary");
    write_recording(&path, start, &messages);

    let bus = BusNode::new(PARTICIPANT).expect("the participant's bus");
    // A subscriber that declares no policy never creates the topic, so this test opens it first.
    let data_md5 =
        hypha_node::sdk::md5::md5_from_definition("std_msgs/UInt8", "uint8 data\n").expect("MD5");
    let _data_opener = bus
        .publish_raw(DATA_TOPIC, "std_msgs/UInt8", &data_md5, "uint8 data\n")
        .expect("the data topic's opener");
    let mut data = bus
        .subscribe_raw_with_schema(
            DATA_TOPIC,
            "std_msgs/UInt8",
            "",
            hypha_node::sdk::SubscriberOptions::default().keeping(8),
        )
        .expect("a data subscriber");
    let mut responder = Responder::new(&bus, PARTICIPANT);
    let mut child = spawn_player(&path, &["--nodes", PARTICIPANT, "--rate", "max"]);
    let run = serve(&mut child, &mut responder, &mut data, Policy::Always, |_| false);
    let status = run.status.expect("the child exits");
    std::fs::remove_file(&path).expect("remove the recording");
    assert!(status.success(), "the replay succeeds");

    // The messages at 100 ms go out before /clock 100 ms, the message at the start after
    // the start barrier's clock, and the pass ends at the first boundary after its last
    // message.
    let boundaries: Vec<u64> = [0, 100, 200, 300, 400].iter().map(|ms| at(start, *ms)).collect();
    assert_eq!(clock_values(&run.trace), boundaries);
    let timed: Vec<(u64, u8)> =
        messages.iter().map(|(offset, value)| (at(start, *offset), *value)).collect();
    assert_message_order(&run.trace, &timed);
}

#[test]
fn a_window_that_ends_off_the_chunk_grid_ends_the_clock_at_its_end() {
    let start = start_nanos();
    let path = recording_path("off_grid_end");
    write_recording(&path, start, &MESSAGES);

    let bus = BusNode::new(PARTICIPANT).expect("the participant's bus");
    // A subscriber that declares no policy never creates the topic, so this test opens it first.
    let data_md5 =
        hypha_node::sdk::md5::md5_from_definition("std_msgs/UInt8", "uint8 data\n").expect("MD5");
    let _data_opener = bus
        .publish_raw(DATA_TOPIC, "std_msgs/UInt8", &data_md5, "uint8 data\n")
        .expect("the data topic's opener");
    let mut data = bus
        .subscribe_raw_with_schema(
            DATA_TOPIC,
            "std_msgs/UInt8",
            "",
            hypha_node::sdk::SubscriberOptions::default().keeping(8),
        )
        .expect("a data subscriber");
    let mut responder = Responder::new(&bus, PARTICIPANT);
    // The window ends at 450 ms, which is not a chunk boundary. The last clock is 450 ms, not 500.
    let mut child = spawn_player(
        &path,
        &["--nodes", PARTICIPANT, "--rate", "max", "--end-utc", "2026-10-08T12:00:00.450Z"],
    );
    let run = serve(&mut child, &mut responder, &mut data, Policy::Always, |_| false);
    let status = run.status.expect("the child exits");
    std::fs::remove_file(&path).expect("remove the recording");
    assert!(status.success(), "the replay succeeds");

    let boundaries: Vec<u64> =
        [0, 100, 200, 300, 400, 450].iter().map(|ms| at(start, *ms)).collect();
    assert_eq!(clock_values(&run.trace), boundaries);
    assert_message_order(&run.trace, &messages_at(start));
}

#[test]
fn a_tie_on_two_topics_at_a_boundary_goes_out_before_its_clock() {
    const OTHER_TOPIC: &str = "/flow_data_other";
    let start = start_nanos();
    let messages =
        [(0, 1, DATA_TOPIC), (100, 2, DATA_TOPIC), (100, 3, OTHER_TOPIC), (150, 4, OTHER_TOPIC)];
    let path = recording_path("tie");
    write_on_topics(&path, start, &messages);

    let bus = BusNode::new(PARTICIPANT).expect("the participant's bus");
    let (_opener, data) = open_data(&bus);
    let (_other_opener, other) = open_topic(&bus, OTHER_TOPIC);
    let mut data = [data, other];
    let mut responder = Responder::new(&bus, PARTICIPANT);
    let mut child = spawn_player(&path, &["--nodes", PARTICIPANT, "--rate", "max"]);
    let run = serve_topics(&mut child, &mut responder, &mut data, Policy::Always, |_| false);
    let status = run.status.expect("the child exits");
    std::fs::remove_file(&path).expect("remove the recording");
    assert!(status.success(), "the replay succeeds");

    // Both messages at 100 ms, one on each topic, are out before /clock 100 ms.
    let boundaries: Vec<u64> = [0, 100, 200].iter().map(|ms| at(start, *ms)).collect();
    assert_eq!(clock_values(&run.trace), boundaries);
    assert_eq!(run.trace.data.len(), messages.len(), "every message went out");
    let timed: Vec<(u64, u8)> =
        messages.iter().map(|(offset, value, _)| (at(start, *offset), *value)).collect();
    assert_in_its_chunk(&run.trace, &timed);
}

#[test]
fn a_window_that_ends_after_a_last_message_on_the_grid_closes_that_boundary_first() {
    let start = start_nanos();
    let messages = [(0, 1), (150, 2), (300, 3)];
    let path = recording_path("grid_end_window");
    write_recording(&path, start, &messages);

    let bus = BusNode::new(PARTICIPANT).expect("the participant's bus");
    let (_opener, mut data) = open_data(&bus);
    let mut responder = Responder::new(&bus, PARTICIPANT);
    let mut child = spawn_player(
        &path,
        &["--nodes", PARTICIPANT, "--rate", "max", "--end-utc", "2026-10-08T12:00:00.350Z"],
    );
    let run = serve(&mut child, &mut responder, &mut data, Policy::Always, |_| false);
    let status = run.status.expect("the child exits");
    std::fs::remove_file(&path).expect("remove the recording");
    assert!(status.success(), "the replay succeeds");

    // The message at 300 ms is closed by /clock 300 ms; the window's end, 350 ms, ends the pass.
    let boundaries: Vec<u64> = [0, 100, 200, 300, 350].iter().map(|ms| at(start, *ms)).collect();
    assert_eq!(clock_values(&run.trace), boundaries);
    let timed: Vec<(u64, u8)> =
        messages.iter().map(|(offset, value)| (at(start, *offset), *value)).collect();
    assert_message_order(&run.trace, &timed);
}

#[test]
fn a_loop_ends_each_pass_after_a_last_message_on_the_grid() {
    let start = start_nanos();
    let messages = [(0, 1), (150, 2), (300, 3)];
    let path = recording_path("grid_end_loop");
    write_recording(&path, start, &messages);

    let bus = BusNode::new(PARTICIPANT).expect("the participant's bus");
    let (_opener, mut data) = open_data(&bus);
    let mut responder = Responder::new(&bus, PARTICIPANT);
    let mut child = spawn_player(&path, &["--nodes", PARTICIPANT, "--rate", "max", "--loop"]);
    let wanted = 2 * messages.len();
    let run = serve(&mut child, &mut responder, &mut data, Policy::Always, |trace| {
        trace.data.len() >= wanted
    });
    assert!(run.status.is_none(), "the loop keeps playing until stopped");
    let _ = child.kill();
    let _ = child.wait();
    std::fs::remove_file(&path).expect("remove the recording");

    // The first pass closes 300 ms with its last message, ends at 400 ms, and only then
    // goes back to the start.
    let values = clock_values(&run.trace);
    let pass: Vec<u64> = [0, 100, 200, 300, 400, 0].iter().map(|ms| at(start, *ms)).collect();
    assert_eq!(values.get(..pass.len()), Some(&pass[..]), "{values:?}");
    let (_, last_sent) = run.trace.data.get(2).copied().expect("the last message of pass one");
    let closing = first_sent(&run.trace, at(start, 300)).expect("the clock at the last message");
    assert!(last_sent < closing, "the last message went out after the clock that closes it");
}

/// Starts an open-loop player (`HYPHA_REPLAY_LOCKSTEP=0`) with `--clock`.
fn spawn_open_loop_player(path: &Path, extra: &[&str]) -> Child {
    Command::new(env!("CARGO_BIN_EXE_hypha"))
        .env("HYPHA_REPLAY_LOCKSTEP", "0")
        .arg("play")
        .arg("--input")
        .arg(path)
        .args(["--clock", "--start-utc", START])
        .args(extra)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("hypha_play")
}

/// The participant's data subscriber, on a topic this test opens first: a subscriber
/// that declares no policy never creates it.
fn open_data(bus: &BusNode) -> (hypha_node::sdk::RawPublisher, RawSubscriber) {
    open_topic(bus, DATA_TOPIC)
}

/// [`open_data`] for `topic`.
fn open_topic(bus: &BusNode, topic: &str) -> (hypha_node::sdk::RawPublisher, RawSubscriber) {
    let data_md5 =
        hypha_node::sdk::md5::md5_from_definition("std_msgs/UInt8", "uint8 data\n").expect("MD5");
    let opener = bus
        .publish_raw(topic, "std_msgs/UInt8", &data_md5, "uint8 data\n")
        .expect("the data topic's opener");
    let data = bus
        .subscribe_raw_with_schema(
            topic,
            "std_msgs/UInt8",
            "",
            hypha_node::sdk::SubscriberOptions::default().keeping(8),
        )
        .expect("a data subscriber");
    (opener, data)
}

#[test]
fn an_open_loop_replay_does_not_wait_for_a_participant_that_never_answers() {
    let start = start_nanos();
    let path = recording_path("open_loop");
    write_recording(&path, start, &MESSAGES);

    let bus = BusNode::new(PARTICIPANT).expect("the participant's bus");
    let (_opener, mut data) = open_data(&bus);
    let mut responder = Responder::new(&bus, PARTICIPANT);
    let mut child = spawn_open_loop_player(&path, &["--rate", "20"]);

    let run = serve(&mut child, &mut responder, &mut data, Policy::Silent { after: 0 }, |_| false);
    let status = run.status.expect("the child exits");
    std::fs::remove_file(&path).expect("remove the recording");
    assert!(status.success(), "an open-loop replay waits for no answer");
    let boundaries: Vec<u64> =
        [0, 100, 200, 300, 400, 500].iter().map(|ms| at(start, *ms)).collect();
    assert_eq!(clock_values(&run.trace), boundaries, "one /clock per chunk, sent once each");
    assert_eq!(run.trace.clocks.len(), boundaries.len(), "no /clock is sent again");
    assert_message_order(&run.trace, &messages_at(start));
}

#[test]
fn an_open_loop_loop_sends_the_clock_back_without_waiting_for_a_restart() {
    let start = start_nanos();
    let path = recording_path("open_loop_loop");
    write_recording(&path, start, &MESSAGES);

    let bus = BusNode::new(PARTICIPANT).expect("the participant's bus");
    let (_opener, mut data) = open_data(&bus);
    let mut responder = Responder::new(&bus, PARTICIPANT);
    let mut child = spawn_open_loop_player(&path, &["--rate", "20", "--loop"]);

    let wanted = 2 * MESSAGES.len();
    let run = serve(&mut child, &mut responder, &mut data, Policy::Silent { after: 0 }, |trace| {
        trace.data.len() >= wanted
    });
    assert!(run.status.is_none(), "the loop keeps playing until stopped");
    let _ = child.kill();
    let _ = child.wait();
    std::fs::remove_file(&path).expect("remove the recording");

    let values = clock_values(&run.trace);
    let tail = values.iter().position(|nanos| *nanos == at(start, 500)).expect("a tail clock");
    assert_eq!(values.get(tail + 1), Some(&start), "the next pass starts with the clock back");
    let values: Vec<u8> = run.trace.data.iter().map(|(value, _)| *value).collect();
    assert_eq!(values.get(..wanted), Some(&[1, 2, 3, 4, 1, 2, 3, 4][..]), "two passes went out");
}

#[test]
fn an_open_loop_replay_refuses_rate_max() {
    let path = recording_path("open_loop_max");
    write_recording(&path, start_nanos(), &MESSAGES);
    let mut child = spawn_open_loop_player(&path, &["--rate", "max"]);
    let status = child.wait().expect("the child exits");
    let mut stderr = String::new();
    child.stderr.take().expect("stderr").read_to_string(&mut stderr).expect("stderr text");
    std::fs::remove_file(&path).expect("remove the recording");
    assert!(!status.success(), "nothing would pace an open-loop replay at max");
    assert!(stderr.contains("--rate max"), "the refusal names the flag: {stderr}");
}
