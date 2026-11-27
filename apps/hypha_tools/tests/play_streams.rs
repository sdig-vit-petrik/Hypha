//! `hypha play` replays a recording of two versions on one topic as two streams (#209): each
//! typed subscriber gets exactly its version in order, and under `--clock` the chunk and queue
//! rules count each stream on its own.
#![allow(clippy::expect_used, clippy::panic, clippy::arithmetic_side_effects)]

mod common;

use common::Responder;
use hypha_node::sdk::md5::md5_from_definition;
use hypha_node::sdk::{BusNode, RawSubscriber, SubscriberOptions};
use hypha_tools::recorder::{RecordSample, Recorder};
use hypha_tools::utc_time::parse_utc_nanos;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DATA_TYPE: &str = "overload_test/Reading";
const VERSION_1: &str = "uint8 data\n";
const VERSION_2: &str = "uint8 data\nuint8 extra\n";
const START: &str = "2026-10-08T12:00:00Z";
const PARTICIPANT: &str = "play_streams_participant";
/// Messages of each version, 1 ms apart, the second version half a millisecond later.
const BURST: u8 = 10;
const MS: u64 = 1_000_000;

fn md5_of(definition: &str) -> String {
    md5_from_definition(DATA_TYPE, definition).expect("md5")
}

fn unique_topic(label: &str) -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).expect("UTC").as_nanos();
    format!("/play_streams_{label}_{}_{nanos:x}", std::process::id())
}

/// Records [`BURST`] messages of each version on `topic` with the recorder, interleaved:
/// version 1 carries `value`, version 2 carries `100 + value` and one byte more.
fn record_two_versions(path: &std::path::Path, topic: &str, start: u64) {
    let mut recorder = Recorder::new(path);
    assert!(recorder.is_open(), "{}", recorder.open_error());
    let (md5_1, md5_2) = (md5_of(VERSION_1), md5_of(VERSION_2));
    for value in 0..BURST {
        let time = start + u64::from(value + 1) * MS;
        for (md5, definition, bytes, time) in [
            (&md5_1, VERSION_1, vec![value], time),
            (&md5_2, VERSION_2, vec![100 + value, 0], time + MS / 2),
        ] {
            assert!(recorder.record(&RecordSample {
                topic,
                data_type: DATA_TYPE,
                md5,
                definition,
                bytes: &bytes,
                log_time_nanos: time,
                publish_time_nanos: time,
            }));
        }
    }
    recorder.finish().expect("finish");
}

fn drain(subscriber: &mut RawSubscriber, values: &mut Vec<Vec<u8>>) {
    while let Some(sample) = subscriber.try_receive().expect("a receive") {
        values.push(sample.bytes().expect("a payload").to_vec());
    }
}

/// Two typed subscribers of `topic`, one for each version, of queue `depth`. The test opens
/// both streams first: a subscriber that declares no policy never creates its stream.
fn subscribers(bus: &BusNode, topic: &str, depth: usize) -> (RawSubscriber, RawSubscriber) {
    let mut open = Vec::new();
    let mut readers = Vec::new();
    for (md5, definition) in [(md5_of(VERSION_1), VERSION_1), (md5_of(VERSION_2), VERSION_2)] {
        open.push(bus.publish_raw(topic, DATA_TYPE, &md5, definition).expect("opener"));
        readers.push(
            bus.subscribe_raw_with_schema(
                topic,
                DATA_TYPE,
                &md5,
                SubscriberOptions::default().keeping(depth),
            )
            .expect("a typed subscriber"),
        );
    }
    std::mem::forget(open);
    let second = readers.pop().expect("version 2");
    (readers.pop().expect("version 1"), second)
}

#[test]
fn a_recording_of_two_versions_on_one_name_is_replayed_on_two_streams() {
    let topic = unique_topic("open");
    let path = std::env::temp_dir().join(format!("play_streams_open_{}.mcap", std::process::id()));
    record_two_versions(&path, &topic, parse_utc_nanos(START).expect("the UTC start"));
    let bus = BusNode::new("play_streams_open").expect("node");
    let (mut first, mut second) = subscribers(&bus, &topic, 16);

    let mut child = Command::new(env!("CARGO_BIN_EXE_hypha"))
        .arg("play")
        .arg("--input")
        .arg(&path)
        .args(["--rate", "max"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("hypha play");
    // Drained while the player runs: a message goes with the publisher that sent it.
    let (mut one, mut two) = (Vec::new(), Vec::new());
    while child.try_wait().expect("the child status").is_none() {
        drain(&mut first, &mut one);
        drain(&mut second, &mut two);
        std::thread::sleep(Duration::from_millis(1));
    }
    drain(&mut first, &mut one);
    drain(&mut second, &mut two);
    let output = child.wait_with_output().expect("hypha play output");
    std::fs::remove_file(&path).expect("remove the recording");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout} {}", String::from_utf8_lossy(&output.stderr));

    assert_eq!(one, (0..BURST).map(|v| vec![v]).collect::<Vec<_>>(), "version 1, in order");
    assert_eq!(two, (0..BURST).map(|v| vec![100 + v, 0]).collect::<Vec<_>>(), "version 2");
    assert_eq!(
        first.missing_sequences().expect("count") + second.missing_sequences().expect("count"),
        0
    );
    println!("{stdout}");
    assert!(
        stdout.contains(&format!(
            "'{topic}' [{DATA_TYPE} ({})] published 10 messages",
            &md5_of(VERSION_1)[..8]
        )),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!(
            "'{topic}' [{DATA_TYPE} ({})] published 10 messages",
            &md5_of(VERSION_2)[..8]
        )),
        "{stdout}"
    );
}

#[test]
fn a_lockstep_replay_counts_the_queue_of_each_stream_on_its_own() {
    let topic = unique_topic("lockstep");
    let path = std::env::temp_dir().join(format!("play_streams_lock_{}.mcap", std::process::id()));
    let start = parse_utc_nanos(START).expect("the UTC start");
    record_two_versions(&path, &topic, start);
    let bus = BusNode::new("play_streams_lockstep").expect("node");
    let (mut first, mut second) = subscribers(&bus, &topic, 4);
    let mut responder = Responder::new(&bus, PARTICIPANT);
    let mut child = Command::new(env!("CARGO_BIN_EXE_hypha"))
        .arg("play")
        .arg("--input")
        .arg(&path)
        .args(["--clock", "--start-utc", START, "--nodes", PARTICIPANT, "--rate", "max"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("hypha play");

    let began = Instant::now();
    let (mut one, mut two) = (Vec::new(), Vec::new());
    let mut clock_values: Vec<u64> = Vec::new();
    let status = loop {
        let exited = child.try_wait().expect("the child status");
        let clocks = responder.receive();
        drain(&mut first, &mut one);
        drain(&mut second, &mut two);
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
    drain(&mut first, &mut one);
    drain(&mut second, &mut two);
    std::fs::remove_file(&path).expect("remove the recording");
    assert!(status.success(), "the replay succeeds");

    assert_eq!(one, (0..BURST).map(|v| vec![v]).collect::<Vec<_>>(), "version 1, in order");
    assert_eq!(two, (0..BURST).map(|v| vec![100 + v, 0]).collect::<Vec<_>>(), "version 2");
    assert_eq!(first.missing_sequences().expect("count"), 0);
    assert_eq!(second.missing_sequences().expect("count"), 0);
    // Each stream fills its queue of 4 on its own: the fifth message of version 1 (at 5 ms)
    // and its ninth (at 9 ms) close a chunk, the other version's messages in between
    // counting for their own stream. Counted by name, the chunk would close every 4 messages.
    let expected = vec![start, start + 5 * MS - 1, start + 9 * MS - 1, start + 100 * MS];
    assert_eq!(clock_values, expected, "the chunks close before the messages that overflow");
}
