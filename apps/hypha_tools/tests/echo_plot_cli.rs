//! The real `hypha topic echo -p` prints a header and one row for each message.
#![allow(clippy::expect_used)]

use mcap::records::MessageHeader;
use mcap::{WriteOptions, Writer};
use std::collections::BTreeMap;
use std::fs::File;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

const DEFINITION: &str = "string data\n";
const COLORED: &str = "\u{1b}[32mOK\u{1b}[0m state=idle";

#[test]
fn plot_mode_prints_a_header_and_a_row_with_the_colored_string() {
    if std::env::var_os("HYPHA_ECHO_PLOT_CHILD").is_none() {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).expect("clock").as_nanos();
        let status = Command::new(std::env::current_exe().expect("test executable"))
            .args(["--exact", "plot_mode_prints_a_header_and_a_row_with_the_colored_string"])
            .env("HYPHA_ECHO_PLOT_CHILD", "1")
            .env("HYPHA_DOMAIN", format!("t_{}_ep_{nonce:x}", std::process::id()))
            .status()
            .expect("isolated-domain test");
        assert!(status.success());
        return;
    }

    let path = std::env::temp_dir().join(format!("echo_plot_{}.mcap", std::process::id()));
    write_fixture(&path);
    let echo = Command::new(env!("CARGO_BIN_EXE_hypha"))
        .arg("topic")
        .args(["echo", "-p", "/debug_state", "--count", "1"])
        .stdout(Stdio::piped())
        .spawn()
        .expect("hypha_topic");
    std::thread::sleep(std::time::Duration::from_millis(500));
    let mut play = Command::new(env!("CARGO_BIN_EXE_hypha"))
        .arg("play")
        .arg("--input")
        .arg(&path)
        .arg("--loop")
        .stdout(Stdio::null())
        .spawn()
        .expect("hypha_play");
    let output = echo.wait_with_output().expect("echo output");
    play.kill().expect("stop playback");
    play.wait().expect("reap playback");
    std::fs::remove_file(path).expect("remove fixture");

    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).expect("utf-8");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.first().copied(), Some("%time,data"), "got: {text:?}");
    let row = lines.get(1).expect("one row");
    assert!(row.ends_with(&format!(",{COLORED}")), "got: {row:?}");
    // %time is the framework send stamp, which the player gives as the recorded time.
    let stamped_as_recorded = row.starts_with("1000000,") || row.starts_with("2000000,");
    assert!(stamped_as_recorded, "got: {row:?}");
}

fn write_fixture(path: &Path) {
    let mut writer = Writer::with_options(
        File::create(path).expect("file"),
        WriteOptions::new().profile("ros1"),
    )
    .expect("writer");
    let schema =
        writer.add_schema("std_msgs/String", "ros1msg", DEFINITION.as_bytes()).expect("schema");
    let md5 =
        hypha_node::sdk::md5::md5_from_definition("std_msgs/String", DEFINITION).expect("MD5");
    let metadata = BTreeMap::from([("md5sum".to_string(), md5)]);
    let channel = writer.add_channel(schema, "/debug_state", "ros1", &metadata).expect("channel");
    let mut payload = u32::try_from(COLORED.len()).expect("length").to_le_bytes().to_vec();
    payload.extend_from_slice(COLORED.as_bytes());
    for (sequence, time) in [(0_u32, 1_000_000_u64), (1, 2_000_000)] {
        let header =
            MessageHeader { channel_id: channel, sequence, log_time: time, publish_time: time };
        writer.write_to_known_channel(&header, &payload).expect("message");
    }
    writer.finish().expect("finish");
}
