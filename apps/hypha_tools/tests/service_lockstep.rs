//! A replay with services between participants repeats exactly (FSR-44,
//! `docs/design/native-services.md` §4): real C++ participants (`service_lockstep_peer`)
//! with `Requests` and `Calls` ports under `hypha play --clock`.
//!
//! Each participant prints one line per fact of a step, stamped with the step's time. A run
//! is the lines of every participant; two runs that differ only in `--chunk`, in an attached
//! `hypha topic echo`, or in the pass of a `--loop`, print the same lines. The lines also
//! show the strict cut: a request stamped `t` is served at the server's first step after
//! `t`, and its answer is read at the client's first step after that.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use hypha_tools::launch_supervisor::STOP_GRACE_PERIOD;
use hypha_tools::utc_time::parse_utc_nanos;
use mcap::records::MessageHeader;
use mcap::{WriteOptions, Writer};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const PEER: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../build/crates/hypha-cxx/service_lockstep_peer");
const START: &str = "2026-11-01T12:00:00Z";
/// The tick topic: one message every 10 ms for one second, the window of every replay.
const TICK: &str = "/svc_lockstep_tick";
const TICKS: u64 = 100;
const RUN_LIMIT: Duration = Duration::from_secs(90);

/// Kills the process when the test ends, passed or not.
struct Killed(Child);

impl Drop for Killed {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A `hypha launch unit` supervisor, stopped with SIGTERM so it stops its node too.
struct Supervisor(Child);

impl Drop for Supervisor {
    fn drop(&mut self) {
        let pid = nix::unistd::Pid::from_raw(i32::try_from(self.0.id()).unwrap_or(i32::MAX));
        let _ = nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGTERM);
        let began = Instant::now();
        let limit = STOP_GRACE_PERIOD.saturating_add(Duration::from_secs(5));
        while began.elapsed() < limit {
            if !matches!(self.0.try_wait(), Ok(None)) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn scratch(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("service_lockstep_{}_{name}", std::process::id()))
}

fn require_peer() {
    assert!(
        Path::new(PEER).is_file(),
        "the C++ peer is not built at {PEER}; run: cmake --build build --target service_lockstep_peer"
    );
}

/// One `std_msgs/UInt32` on [`TICK`] every 10 ms from `START`.
fn write_recording(path: &Path) {
    let start = parse_utc_nanos(START).expect("the UTC start");
    let mut writer = Writer::with_options(
        File::create(path).expect("the recording file"),
        WriteOptions::new().profile("ros1"),
    )
    .expect("a writer");
    let definition = "uint32 data\n";
    let schema = writer.add_schema("std_msgs/UInt32", "ros1msg", definition.as_bytes()).unwrap();
    let md5 = hypha_node::sdk::md5::md5_from_definition("std_msgs/UInt32", definition).unwrap();
    let metadata = BTreeMap::from([("md5sum".to_string(), md5)]);
    let channel = writer.add_channel(schema, TICK, "ros1", &metadata).expect("channel");
    for index in 0..TICKS {
        let time = start + index * 10_000_000;
        let header = MessageHeader {
            channel_id: channel,
            sequence: u32::try_from(index).unwrap(),
            log_time: time,
            publish_time: time,
        };
        writer
            .write_to_known_channel(&header, &u32::try_from(index).unwrap().to_le_bytes())
            .unwrap();
    }
    writer.finish().expect("finish");
}

/// One participant: its node name, role, whether it steps on each tick, and the services
/// of its server and client ports.
#[derive(Clone)]
struct Peer {
    name: String,
    role: &'static str,
    triggered: bool,
    serves: Option<String>,
    calls: Option<String>,
}

impl Peer {
    fn new(label: &str, role: &'static str) -> Self {
        let name = format!("svc_{label}_{}", std::process::id());
        Self { name, role, triggered: false, serves: None, calls: None }
    }

    fn triggered(mut self) -> Self {
        self.triggered = true;
        self
    }

    fn serving(mut self, service: &str) -> Self {
        self.serves = Some(service.to_owned());
        self
    }

    fn calling(mut self, service: &str) -> Self {
        self.calls = Some(service.to_owned());
        self
    }

    fn parameters(&self) -> PathBuf {
        let mut text = format!("[node]\nname = \"{}\"\n", self.name);
        if self.triggered {
            let _ = write!(text, "[ports.inputs.tick]\ntopic = \"{TICK}\"\nsilence_ms = 1000\n");
        } else {
            text.push_str("[cycle]\nrate_hz = 100.0\n");
        }
        if let Some(service) = &self.serves {
            let _ = write!(text, "[ports.servers.serve]\nservice = \"{service}\"\n");
        }
        if let Some(service) = &self.calls {
            let _ = write!(text, "[ports.clients.call]\nservice = \"{service}\"\n");
        }
        let path = scratch(&format!("{}.toml", self.name));
        std::fs::write(&path, text).expect("the peer's parameter file");
        path
    }

    fn environment(&self) -> [(&'static str, &'static str); 3] {
        [
            ("HYPHA_USE_SIM_TIME", "1"),
            ("SERVICE_PEER_ROLE", self.role),
            ("SERVICE_PEER_TRIGGERED", if self.triggered { "1" } else { "0" }),
        ]
    }

    fn spawn(&self, output: &Path) -> Killed {
        let child = Command::new(PEER)
            .arg(self.parameters())
            .envs(self.environment())
            .stdout(File::create(output).expect("the peer's output"))
            .stderr(Stdio::null())
            .spawn()
            .expect("the peer");
        Killed(child)
    }
}

/// A service name unique to this process.
fn service(label: &str) -> String {
    format!("/svc_lockstep_{}_{label}", std::process::id())
}

/// The fact lines of a peer's output: those with a step time, in order.
fn facts(text: &str) -> Vec<String> {
    text.lines().filter(|line| line.contains(" t=")).map(str::to_owned).collect()
}

/// How one run is made.
#[derive(Clone, Copy)]
struct Run {
    chunk_ms: u64,
    echo: bool,
}

/// Replay the recording once with `peers`, and return each peer's fact lines.
fn replay(peers: &[Peer], run: Run) -> Vec<Vec<String>> {
    require_peer();
    let recording = scratch("tick.mcap");
    write_recording(&recording);
    let outputs: Vec<PathBuf> =
        peers.iter().map(|peer| scratch(&format!("{}.out", peer.name))).collect();
    let children: Vec<Killed> =
        peers.iter().zip(&outputs).map(|(peer, output)| peer.spawn(output)).collect();
    let echo = run.echo.then(|| {
        Killed(
            Command::new(env!("CARGO_BIN_EXE_hypha"))
                .args(["topic", "echo", TICK])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("hypha topic echo"),
        )
    });
    let chunk = run.chunk_ms.to_string();
    let names: Vec<&str> = peers.iter().map(|peer| peer.name.as_str()).collect();
    let mut player = Command::new(env!("CARGO_BIN_EXE_hypha"))
        .arg("play")
        .arg("--input")
        .arg(&recording)
        .args(["--clock", "--start-utc", START, "--rate", "max", "--chunk", &chunk])
        .arg("--nodes")
        .args(&names)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("hypha play");
    let began = Instant::now();
    let status = loop {
        if let Some(status) = player.try_wait().expect("the player status") {
            break status;
        }
        assert!(began.elapsed() < RUN_LIMIT, "the replay did not end within {RUN_LIMIT:?}");
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut stderr = String::new();
    if let Some(mut pipe) = player.stderr.take() {
        let _ = std::io::Read::read_to_string(&mut pipe, &mut stderr);
    }
    assert!(status.success(), "hypha play failed: {status}: {stderr}");
    drop(echo);
    drop(children);
    let lines = outputs
        .iter()
        .map(|output| facts(&std::fs::read_to_string(output).unwrap_or_default()))
        .collect();
    for path in outputs.iter().chain([&recording]) {
        let _ = std::fs::remove_file(path);
    }
    for peer in peers {
        let _ = std::fs::remove_file(scratch(&format!("{}.toml", peer.name)));
    }
    lines
}

/// The step time of a fact line.
fn time_of(line: &str) -> i64 {
    let field = line.split_whitespace().find_map(|word| word.strip_prefix("t=")).expect("t=");
    field.parse().expect("a time")
}

/// The time and text of each line of `lines` whose fact is `fact`.
fn each(lines: &[String], fact: &str) -> Vec<(i64, String)> {
    lines
        .iter()
        .filter_map(|line| {
            let mut words = line.split_whitespace().skip(2);
            (words.next() == Some(fact))
                .then(|| (time_of(line), words.next().unwrap_or("").to_owned()))
        })
        .collect()
}

/// Every answer `c<k>@<served>` read at `read`: served one step (10 ms) after the call,
/// read one step after that, as the strict cut gives with `hops` servers between.
fn assert_strict_hops(client: &[String], hops: i64) {
    let calls: BTreeMap<String, i64> =
        each(client, "call").into_iter().map(|(t, text)| (text, t)).collect();
    let results = each(client, "result");
    assert!(results.len() >= 10, "too few answers: {client:?}");
    for (read, text) in results {
        let request = text.split(['@', '>']).next().unwrap();
        let called = calls[request];
        assert_eq!(
            read,
            called + 20 * hops,
            "{request} called at {called} was read at {read}: {text}"
        );
    }
}

fn assert_same(first: &[Vec<String>], other: &[Vec<String>], what: &str) {
    for (index, (a, b)) in first.iter().zip(other).enumerate() {
        assert_eq!(a, b, "participant {index} differs {what}");
    }
}

fn two_nodes(label: &str, triggered: bool) -> Vec<Peer> {
    let name = service(label);
    let mut server = Peer::new(&format!("{label}_server"), "server").serving(&name);
    let mut client = Peer::new(&format!("{label}_client"), "client").calling(&name);
    if triggered {
        server = server.triggered();
        client = client.triggered();
    }
    vec![client, server]
}

#[test]
fn a_rate_client_and_server_repeat_exactly_whatever_the_chunk_and_an_observer() {
    let peers = two_nodes("rate", false);
    let first = replay(&peers, Run { chunk_ms: 50, echo: false });
    assert_strict_hops(&first[0], 1);
    let server = each(&first[1], "serve");
    assert!(!server.is_empty(), "the server served nothing");
    let again = replay(&peers, Run { chunk_ms: 50, echo: false });
    assert_same(&first, &again, "between two identical runs");
    let larger = replay(&peers, Run { chunk_ms: 200, echo: false });
    assert_same(&first, &larger, "between --chunk 50 and --chunk 200");
    let observed = replay(&peers, Run { chunk_ms: 50, echo: true });
    assert_same(&first, &observed, "with hypha topic echo attached");
}

#[test]
fn a_triggered_client_and_a_triggered_server_do_not_deadlock_and_repeat_exactly() {
    let peers = two_nodes("trig", true);
    let first = replay(&peers, Run { chunk_ms: 50, echo: false });
    assert_strict_hops(&first[0], 1);
    let larger = replay(&peers, Run { chunk_ms: 200, echo: false });
    assert_same(&first, &larger, "between --chunk 50 and --chunk 200");
}

#[test]
fn a_chain_of_deferred_answers_across_three_nodes_repeats_exactly() {
    let (front, back) = (service("chain_front"), service("chain_back"));
    let peers = vec![
        Peer::new("chain_client", "client").calling(&front),
        Peer::new("chain_relay", "relay").serving(&front).calling(&back),
        Peer::new("chain_server", "server").serving(&back),
    ];
    let first = replay(&peers, Run { chunk_ms: 50, echo: false });
    // client -> relay (defers, calls on) -> server, and back: two servers in the way.
    assert_strict_hops(&first[0], 2);
    let relayed = each(&first[0], "result");
    assert!(
        relayed.iter().all(|(_, text)| text.ends_with('<') && text.contains(">@")),
        "{relayed:?}"
    );
    let larger = replay(&peers, Run { chunk_ms: 200, echo: false });
    assert_same(&first, &larger, "between --chunk 50 and --chunk 200");
}

#[test]
fn a_client_and_its_server_in_one_waitset_repeat_exactly() {
    let name = service("pair");
    let peers = vec![Peer::new("pair", "pair").serving(&name).calling(&name)];
    let first = replay(&peers, Run { chunk_ms: 50, echo: false });
    assert_strict_hops(&first[0], 1);
    let larger = replay(&peers, Run { chunk_ms: 200, echo: false });
    assert_same(&first, &larger, "between --chunk 50 and --chunk 200");
}

/// A pass of `--loop` restarts every participant; the second pass prints the first's lines.
#[test]
fn a_loop_repeats_each_pass_after_the_participants_restart() {
    require_peer();
    let peers = two_nodes("loop", false);
    let recording = scratch("loop.mcap");
    write_recording(&recording);
    let mut supervisors = Vec::new();
    let mut outputs = Vec::new();
    for peer in &peers {
        let units = scratch(&format!("{}.units.toml", peer.name));
        let env = peer.environment().iter().fold(String::new(), |mut text, (key, value)| {
            let _ = writeln!(text, "{key} = \"{value}\"");
            text
        });
        std::fs::write(
            &units,
            format!(
                "[[units]]\nname = \"peer\"\nexec = \"{PEER}\"\nargs = [\"{}\"]\nrestart = \"no\"\n[units.env]\n{env}",
                peer.parameters().display()
            ),
        )
        .expect("the units file");
        let output = scratch(&format!("{}.loop.out", peer.name));
        let supervisor = Command::new(env!("CARGO_BIN_EXE_hypha"))
            .args(["launch", "unit"])
            .arg(&units)
            .arg("peer")
            .stdout(File::create(&output).expect("the peer output"))
            .stderr(Stdio::null())
            .spawn()
            .expect("the peer's supervisor");
        supervisors.push(Supervisor(supervisor));
        outputs.push(output);
    }
    let names: Vec<&str> = peers.iter().map(|peer| peer.name.as_str()).collect();
    let player = Command::new(env!("CARGO_BIN_EXE_hypha"))
        .arg("play")
        .arg("--input")
        .arg(&recording)
        .args(["--clock", "--start-utc", START, "--rate", "max", "--loop", "--nodes"])
        .args(&names)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("hypha play");
    let mut player = Killed(player);
    let began = Instant::now();
    let passes =
        |text: &str| -> Vec<Vec<String>> { text.split("start ").skip(1).map(facts).collect() };
    loop {
        let done = outputs
            .iter()
            .all(|output| passes(&std::fs::read_to_string(output).unwrap_or_default()).len() >= 3);
        if done {
            break;
        }
        assert!(player.0.try_wait().expect("the player").is_none(), "the player exited");
        assert!(began.elapsed() < RUN_LIMIT, "no two passes within {RUN_LIMIT:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(player);
    drop(supervisors);
    for (index, output) in outputs.iter().enumerate() {
        let passes = passes(&std::fs::read_to_string(output).unwrap_or_default());
        assert!(!passes[0].is_empty(), "participant {index} printed nothing in its first pass");
        assert_eq!(passes[0], passes[1], "participant {index}: the second pass differs");
        if index == 0 {
            assert_strict_hops(&passes[1], 1);
        }
    }
    for path in outputs.iter().chain([&recording]) {
        let _ = std::fs::remove_file(path);
    }
}
