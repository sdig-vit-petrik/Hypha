//! A backward seek of `hypha play` restarts a real C++ participant, `replay_restart_peer`.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use hypha_tools::utc_time::parse_utc_nanos;
use mcap::records::MessageHeader;
use mcap::{WriteOptions, Writer};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead as _, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const PEER: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../build/crates/hypha-cxx/replay_restart_peer");
const PEER_NAME: &str = "replay_restart_peer";
const START: &str = "2026-10-08T12:00:00Z";
const RUN_LIMIT: Duration = Duration::from_secs(60);

/// Kills the process when the test ends, passed or not.
struct Killed(Child);

impl Drop for Killed {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn scratch(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("play_restart_{}_{name}", std::process::id()))
}

/// Three messages on `/restart_data` in the first 300 ms after `start`.
fn write_recording(path: &Path, start: u64) {
    let mut writer = Writer::with_options(
        File::create(path).expect("the recording file"),
        WriteOptions::new().profile("ros1"),
    )
    .expect("a writer");
    let schema = writer.add_schema("std_msgs/UInt8", "ros1msg", b"uint8 data\n").expect("schema");
    let md5 =
        hypha_node::sdk::md5::md5_from_definition("std_msgs/UInt8", "uint8 data\n").expect("MD5");
    let metadata = BTreeMap::from([("md5sum".to_string(), md5)]);
    let channel = writer.add_channel(schema, "/restart_data", "ros1", &metadata).expect("channel");
    for (sequence, offset_millis) in [10_u64, 150, 250].into_iter().enumerate() {
        let time = offset_millis
            .checked_mul(1_000_000)
            .and_then(|offset| start.checked_add(offset))
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
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

#[test]
fn a_loop_restarts_a_cpp_participant_and_it_rejoins_every_pass() {
    loop_restarts_the_peer(false);
}

#[test]
fn a_sync_recorder_stays_while_the_participants_restart() {
    loop_restarts_the_peer(true);
}

/// Starts `hypha record --sync` and returns once it follows its topics.
fn spawn_recorder(output: &Path) -> Killed {
    let mut recorder = Command::new(env!("CARGO_BIN_EXE_hypha"))
        .arg("record")
        .args(["--sync", "--topics", "/restart_data", "--compression", "none", "-o"])
        .arg(output)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("hypha_record");
    let stdout = recorder.stdout.take().expect("the recorder's stdout");
    let following = BufReader::new(stdout)
        .lines()
        .map_while(Result::ok)
        .any(|line| line.starts_with("Following"));
    assert!(following, "the recorder follows its topics");
    Killed(recorder)
}

/// Loops a short recording with the C++ peer, and a sync recorder when `with_recorder`,
/// until the peer has restarted and rejoined twice.
fn loop_restarts_the_peer(with_recorder: bool) {
    assert!(
        Path::new(PEER).is_file(),
        "the C++ peer is not built at {PEER}; run: cmake --build build --target replay_restart_peer"
    );
    let recording = scratch("loop.mcap");
    write_recording(&recording, parse_utc_nanos(START).expect("the UTC start"));
    let (peer_out, player_out, recorder_out) =
        (scratch("peer.out"), scratch("player.out"), scratch("recorder_out.mcap"));
    let recorder = with_recorder.then(|| spawn_recorder(&recorder_out));

    // The peer exits 75 on each backward seek; its supervisor starts it again at once
    // with the rejoin marker, as in deployment.
    let units = scratch("units.toml");
    std::fs::write(
        &units,
        format!(
            "[[units]]\nname = \"peer\"\nexec = \"{PEER}\"\nrestart = \"no\"\n[units.env]\nHYPHA_USE_SIM_TIME = \"1\"\n"
        ),
    )
    .expect("the units file");
    let peer = Command::new(env!("CARGO_BIN_EXE_hypha"))
        .arg("launch")
        .arg("unit")
        .arg(&units)
        .arg("peer")
        .stdout(File::create(&peer_out).expect("the peer output"))
        .stderr(Stdio::null())
        .spawn()
        .expect("the peer's supervisor");
    let mut peer = Killed(peer);
    let player = Command::new(env!("CARGO_BIN_EXE_hypha"))
        .arg("play")
        .arg("--input")
        .arg(&recording)
        .args(["--clock", "--start-utc", START, "--nodes", PEER_NAME, "--rate", "max", "--loop"])
        .stdout(File::create(&player_out).expect("the player output"))
        .stderr(Stdio::null())
        .spawn()
        .expect("hypha_play");
    let mut player = Killed(player);

    // Three passes: the first, and two restarts that each rejoin.
    let began = Instant::now();
    loop {
        let steps = read(&peer_out);
        let rejoined = steps.matches("first step, rejoined 1").count();
        let rejoins = read(&player_out).matches("every participant rejoined").count();
        if steps.contains("first step, rejoined 0") && rejoined >= 2 && rejoins >= 2 {
            break;
        }
        assert!(player.0.try_wait().expect("the player status").is_none(), "the player exited");
        assert!(
            peer.0.try_wait().expect("the supervisor status").is_none(),
            "the supervisor exited"
        );
        assert!(
            began.elapsed() < RUN_LIMIT,
            "no two restarts within {RUN_LIMIT:?}; peer: {steps}; player: {}",
            read(&player_out)
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let supervised = read(&peer_out);
    let restarted_now =
        supervised.matches("exited 75 (simulated clock went back); restarting now").count();
    assert!(restarted_now >= 2, "the peer exits 75 and restarts at once: {supervised}");
    if with_recorder {
        let restarting = read(&player_out);
        let line = restarting.lines().find(|line| line.contains("restarting")).unwrap_or_default();
        assert!(line.contains(PEER_NAME), "the peer restarts: {line}");
    }
    drop(player);
    drop(peer);
    drop(recorder);
    for path in [&recording, &peer_out, &player_out, &recorder_out, &units] {
        let _ = std::fs::remove_file(path);
    }
}
