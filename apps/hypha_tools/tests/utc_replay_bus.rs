//! The real CLI replays the customer's UTC window with its clock before data.
#![allow(clippy::expect_used)]

#[allow(dead_code, reason = "each test binary uses part of the shared helpers")]
mod common;

use common::Responder;
use hypha_node::sdk::BusNode;
use hypha_tools::utc_time::parse_utc_nanos;
use mcap::records::MessageHeader;
use mcap::{WriteOptions, Writer};
use std::collections::BTreeMap;
use std::fs::File;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The node name of the participant that answers the player's `/clock`.
const PARTICIPANT: &str = "utc_replay_observer";

#[test]
fn utc_window_replay_publishes_utc_clock_before_data() {
    if std::env::var_os("HYPHA_UTC_REPLAY_CHILD").is_none() {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).expect("UTC").as_nanos();
        let domain = format!("ur_{}_{nonce:x}", std::process::id());
        let status = Command::new(std::env::current_exe().expect("test executable"))
            .args(["--exact", "utc_window_replay_publishes_utc_clock_before_data", "--nocapture"])
            .env("HYPHA_UTC_REPLAY_CHILD", "1")
            .env("HYPHA_DOMAIN", domain)
            .status()
            .expect("isolated-domain test");
        assert!(status.success());
        return;
    }

    let start = parse_utc_nanos("2026-10-08T12:00:00Z").expect("UTC start");
    let first = start.checked_add(250_000_000).expect("first time");
    let second = start.checked_add(300_000_000).expect("second time");
    let end = start.checked_add(500_000_000).expect("end time");
    let path = std::env::temp_dir().join(format!("utc_replay_{}.mcap", std::process::id()));
    write_fixture(&path, start, [first, second, end]);

    let bus = BusNode::new(PARTICIPANT).expect("observer");
    // The replay opens its topics when it starts; the observer opens this one first,
    // because a subscriber that declares no policy never creates its topic.
    let incident_md5 = hypha_node::sdk::md5::md5_from_definition("std_msgs/UInt8", "uint8 data\n")
        .expect("data MD5");
    let _incident_opener = bus
        .publish_raw("/utc_incident", "std_msgs/UInt8", &incident_md5, "uint8 data\n")
        .expect("the incident topic's opener");
    let mut input = bus
        .subscribe_raw_with_schema(
            "/utc_incident",
            "std_msgs/UInt8",
            "",
            hypha_node::sdk::SubscriberOptions::default().keeping(8),
        )
        .expect("data subscription");
    let mut responder = Responder::new(&bus, PARTICIPANT);
    let mut child = Command::new(env!("CARGO_BIN_EXE_hypha"))
        .arg("play")
        .arg("--input")
        .arg(&path)
        .args([
            "--clock",
            "--nodes",
            PARTICIPANT,
            "--start-utc",
            "2026-10-08T12:00:00Z",
            "--end-utc",
            "2026-10-08T12:00:00.5Z",
            "--rate",
            "max",
        ])
        .spawn()
        .expect("hypha_play");
    let mut data = Vec::new();
    let mut clock_data = Vec::new();
    let deadline = Instant::now().checked_add(Duration::from_secs(10)).expect("deadline");
    let status = loop {
        for sample in responder.receive() {
            clock_data.push((sample.nanos, sample.sent));
            responder.answer(sample);
        }
        while let Some(sample) = input.try_receive().expect("data receive") {
            let value = *sample.bytes().expect("payload").first().expect("value");
            data.push((value, sample.identity().expect("identity").transport_sent_nanos));
        }
        if let Some(status) = child.try_wait().expect("child status") {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().expect("stop timed-out replay");
            child.wait().expect("reap replay");
            assert!(Instant::now() < deadline, "UTC replay did not terminate");
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    assert!(status.success());
    assert_eq!(data.iter().map(|(value, _)| *value).collect::<Vec<_>>(), vec![1, 2]);
    assert!(
        clock_data.iter().all(|(utc, _)| *utc >= start && *utc < end),
        "recorded /clock must not overwrite UTC"
    );
    assert!(clock_data.iter().any(|(utc, _)| *utc == start), "clock starts at requested UTC");
    // Each message is preceded by the newest clock at or before its time, which was sent
    // before the message went out.
    for ((_, sent), stamped) in data.iter().zip([first, second]) {
        let covering = clock_data
            .iter()
            .filter(|(utc, _)| *utc <= stamped)
            .max_by_key(|(utc, _)| *utc)
            .expect("a clock covers the message");
        assert!(covering.1 < *sent, "UTC clock must be sent before its data payload");
    }
    assert!(
        clock_data.iter().any(|(utc, _)| *utc > start && *utc < first),
        "clock advances through a data gap"
    );
    std::fs::remove_file(path).expect("remove fixture");
}

/// Writes data before, inside and at the end of the window, plus an old `/clock`.
fn write_fixture(path: &Path, start: u64, [first, second, end]: [u64; 3]) {
    let mut writer = Writer::with_options(
        File::create(path).expect("file"),
        WriteOptions::new().profile("ros1"),
    )
    .expect("writer");
    let schema = writer.add_schema("std_msgs/UInt8", "ros1msg", b"uint8 data\n").expect("schema");
    let metadata = BTreeMap::from([(
        "md5sum".to_string(),
        hypha_node::sdk::md5::md5_from_definition("std_msgs/UInt8", "uint8 data\n")
            .expect("data MD5"),
    )]);
    let channel = writer.add_channel(schema, "/utc_incident", "ros1", &metadata).expect("channel");
    for (time, value) in
        [(start.checked_sub(1).expect("before"), 9), (first, 1), (second, 2), (end, 8)]
    {
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
    let clock_schema =
        writer.add_schema("rosgraph_msgs/Clock", "ros1msg", b"time clock\n").expect("clock schema");
    let clock_md5 =
        hypha_node::sdk::md5::md5_from_definition("rosgraph_msgs/Clock", "time clock\n")
            .expect("clock MD5");
    let clock_meta = BTreeMap::from([("md5sum".to_string(), clock_md5)]);
    let clock_channel =
        writer.add_channel(clock_schema, "/clock", "ros1", &clock_meta).expect("clock channel");
    writer
        .write_to_known_channel(
            &MessageHeader {
                channel_id: clock_channel,
                sequence: 0,
                log_time: start.checked_add(200_000_000).expect("clock time"),
                publish_time: start,
            },
            &[1, 0, 0, 0, 0, 0, 0, 0],
        )
        .expect("old simulated clock");
    writer.finish().expect("finish");
    drop(writer);
}
