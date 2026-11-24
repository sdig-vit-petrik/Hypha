//! A lockstep wait of `hypha play` never ends in silence (#148): a wait names the nodes it
//! waits for, and one that cannot end stops the player with their names, against real
//! processes. Every assertion is on the player's lines and exit code, never on its speed.
#![expect(clippy::expect_used, reason = "a refused setup step is a failed test")]

use hypha_node::sdk::BusNode;
use hypha_tools::utc_time::parse_utc_nanos;
use mcap::records::MessageHeader;
use mcap::{WriteOptions, Writer};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead as _, BufReader, Read as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

const PEER: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../build/crates/hypha-cxx/replay_loop_peer");
const START: &str = "2026-10-08T12:00:00Z";

/// Kills the process when the test ends, passed or not.
struct Killed(Child);

impl Drop for Killed {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn scratch(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("play_waits_{}_{name}", std::process::id()))
}

/// One message on `/waits_data` 10 ms after the start.
fn write_recording(path: &Path) {
    let start = parse_utc_nanos(START).expect("the UTC start");
    let mut writer = Writer::with_options(
        File::create(path).expect("the recording file"),
        WriteOptions::new().profile("ros1"),
    )
    .expect("a writer");
    let schema = writer.add_schema("std_msgs/UInt8", "ros1msg", b"uint8 data\n").expect("schema");
    let md5 =
        hypha_node::sdk::md5::md5_from_definition("std_msgs/UInt8", "uint8 data\n").expect("MD5");
    let metadata = BTreeMap::from([("md5sum".to_string(), md5)]);
    let channel = writer.add_channel(schema, "/waits_data", "ros1", &metadata).expect("channel");
    let time = start.checked_add(10_000_000).expect("a time in the window");
    let header =
        MessageHeader { channel_id: channel, sequence: 0, log_time: time, publish_time: time };
    writer.write_to_known_channel(&header, &[1]).expect("message");
    writer.finish().expect("finish");
}

fn spawn_player(recording: &Path, extra: &[&str]) -> Killed {
    let player = Command::new(env!("CARGO_BIN_EXE_hypha"))
        .arg("play")
        .arg("--input")
        .arg(recording)
        .args(["--clock", "--start-utc", START, "--rate", "max"])
        .args(extra)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("hypha_play");
    Killed(player)
}

#[test]
fn a_named_node_without_a_cycle_is_named_as_not_on_the_progress_topic() {
    let recording = scratch("plain.mcap");
    write_recording(&recording);
    // A node that runs, with no cycle: it is not on /clock_progress.
    let name = format!("plain_named_{}", std::process::id());
    let _plain = BusNode::new(&name).expect("the plain node");
    let mut player = spawn_player(&recording, &["--nodes", &name]);
    let stderr = player.0.stderr.take().expect("the player's stderr");
    let expected = format!("'{name}' runs but is not on /clock_progress");
    // The notice comes once the wait passes one second; the 30 s refusal is not awaited.
    let notice = BufReader::new(stderr)
        .lines()
        .map_while(Result::ok)
        .find(|line| line.contains("waiting for --nodes to report"))
        .expect("the player names the wait before it ends");
    drop(player);
    let _ = std::fs::remove_file(&recording);
    assert!(notice.contains(&expected), "{notice}");
}

/// The parameter file of one loop peer: `name` triggered by `input`, sending on `output`.
fn peer_parameters(name: &str, input: &str, output: &str) -> PathBuf {
    let path = scratch(&format!("{name}.toml"));
    std::fs::write(
        &path,
        format!(
            "[node]\nname = \"{name}\"\n\n[ports.inputs.in]\ntopic = \"{input}\"\nsilence_ms = 100\n\n[ports.outputs.out]\ntopic = \"{output}\"\n"
        ),
    )
    .expect("the peer's parameter file");
    path
}

fn spawn_peer(parameters: &Path) -> Killed {
    let peer = Command::new(PEER)
        .arg(parameters)
        .env("HYPHA_USE_SIM_TIME", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the loop peer");
    Killed(peer)
}

#[test]
fn a_loop_of_two_triggered_cycles_stops_the_player_and_both_are_named() {
    assert!(
        Path::new(PEER).is_file(),
        "the C++ peer is not built at {PEER}; run: cmake --build build --target replay_loop_peer"
    );
    let recording = scratch("loop.mcap");
    write_recording(&recording);
    let id = std::process::id();
    let (a, b) = (format!("loop_a_{id}"), format!("loop_b_{id}"));
    let (a_to_b, b_to_a) = (format!("/waits_{id}/a_to_b"), format!("/waits_{id}/b_to_a"));
    let a_parameters = peer_parameters(&a, &b_to_a, &a_to_b);
    let b_parameters = peer_parameters(&b, &a_to_b, &b_to_a);
    let _a = spawn_peer(&a_parameters);
    let _b = spawn_peer(&b_parameters);

    let mut player = spawn_player(&recording, &["--nodes", &a, &b, "--timeout", "1"]);
    let status = player.0.wait().expect("the player exits");
    let mut stderr = String::new();
    player.0.stderr.take().expect("stderr").read_to_string(&mut stderr).expect("stderr text");
    for path in [&recording, &a_parameters, &b_parameters] {
        let _ = std::fs::remove_file(path);
    }

    assert!(!status.success(), "a loop the lockstep cannot run fails the replay: {stderr}");
    let last = stderr.lines().last().unwrap_or_default();
    assert!(last.contains(&format!("'{a}' reports")), "{stderr}");
    assert!(last.contains(&format!("'{b}' reports")), "{stderr}");
    assert!(last.contains("no rate cycle as its delay element"), "{stderr}");
    assert!(last.contains("HYPHA_REPLAY_LOCKSTEP=0"), "{stderr}");
}
