//! `hypha record` follows streams, not names (#209): a second version that joins a followed
//! topic after the recording began is a channel of its own, with its own schema.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

#[allow(dead_code)]
mod common;

use hypha_node::sdk::BusNode;
use hypha_node::sdk::md5::md5_from_definition;
use hypha_testkit::unique_name;
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};

const DATA_TYPE: &str = "overload_test/Reading";
const VERSION_1: &str = "int32 value\n";
const VERSION_2: &str = "int32 value\nstring label\nfloat64 weight\n";

/// One decoded message of a recording: its channel's topic, schema name and md5, and bytes.
type Written = (String, String, String, Vec<u8>);

fn written(path: &std::path::Path) -> Vec<Written> {
    let bytes = std::fs::read(path).expect("the recording");
    mcap::MessageStream::new(&bytes)
        .expect("an MCAP reader")
        .map(|message| {
            let message = message.expect("a valid message");
            let schema = message.channel.schema.as_ref().expect("a schema");
            let md5 = message.channel.metadata.get("md5sum").cloned().unwrap_or_default();
            assert_eq!(
                schema.data,
                if md5 == md5_of(VERSION_1) { VERSION_1.as_bytes() } else { VERSION_2.as_bytes() },
                "each channel carries the definition of its own stream"
            );
            (message.channel.topic.clone(), schema.name.clone(), md5, message.data.to_vec())
        })
        .collect()
}

fn md5_of(definition: &str) -> String {
    md5_from_definition(DATA_TYPE, definition).expect("MD5")
}

#[test]
fn a_version_that_joins_a_followed_topic_after_the_start_is_recorded_on_its_own_channel() {
    let name = unique_name("rec_streams");
    let topic = format!("/{name}/reading");
    let path = std::env::temp_dir().join(format!("{name}.mcap"));
    let bus = BusNode::new(&name).expect("a bus node");
    let mut v1 = bus.publish_raw(&topic, DATA_TYPE, &md5_of(VERSION_1), VERSION_1).expect("v1");

    let mut recorder = Command::new(env!("CARGO_BIN_EXE_hypha"))
        .args(["record", "--topics", &topic, "--compression", "none", "-o"])
        .arg(&path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("hypha record");
    let mut stdout = BufReader::new(recorder.stdout.take().expect("stdout"));
    let mut notices = BufReader::new(recorder.stderr.take().expect("stderr"));
    let mut recorder = common::Killed(recorder);
    let mut line = String::new();
    while !line.starts_with("Following 1 stream") {
        line.clear();
        assert!(stdout.read_line(&mut line).expect("a line") > 0, "the recorder ended early");
    }

    // The migration: v2 joins the name the recorder already follows.
    let mut v2 = bus.publish_raw(&topic, DATA_TYPE, &md5_of(VERSION_2), VERSION_2).expect("v2");
    while !line.contains("following 1 more stream") {
        line.clear();
        assert!(notices.read_line(&mut line).expect("a line") > 0, "the recorder ended early");
    }
    for value in 0_u8..3 {
        assert!(v1.publish(&[value, 0, 0, 0]));
        assert!(v2.publish(&[value, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    std::thread::sleep(std::time::Duration::from_millis(200));
    kill(Pid::from_raw(i32::try_from(recorder.id()).expect("pid")), Signal::SIGTERM)
        .expect("signal the recorder");
    let status = recorder.wait().expect("status");
    let mut summary = String::new();
    stdout.read_to_string(&mut summary).expect("summary");
    assert!(status.success(), "a recording with two versions is complete: {summary}");

    let messages = written(&path);
    std::fs::remove_file(&path).expect("remove the recording");
    let of = |md5: String| -> Vec<Vec<u8>> {
        messages.iter().filter(|m| m.2 == md5).map(|m| m.3.clone()).collect()
    };
    assert_eq!(of(md5_of(VERSION_1)), vec![vec![0, 0, 0, 0], vec![1, 0, 0, 0], vec![2, 0, 0, 0]]);
    assert_eq!(of(md5_of(VERSION_2)).len(), 3, "every message of v2");
    assert!(messages.iter().all(|m| m.0 == topic && m.1 == DATA_TYPE));
    println!("{summary}");
    let short_1 = &md5_of(VERSION_1)[..8];
    let short_2 = &md5_of(VERSION_2)[..8];
    assert!(summary.contains(&format!("{topic} [{DATA_TYPE} ({short_1})]")), "{summary}");
    assert!(summary.contains(&format!("{topic} [{DATA_TYPE} ({short_2})]")), "{summary}");
}
