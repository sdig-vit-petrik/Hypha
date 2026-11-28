//! The harness of the service lockstep tests: a recording, C++ participants, a replay and
//! the facts they print.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use hypha_testkit::wait_until;
use hypha_tools::launch_supervisor::STOP_GRACE_PERIOD;
use hypha_tools::utc_time::parse_utc_nanos;
use mcap::records::MessageHeader;
use mcap::{WriteOptions, Writer};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

pub(crate) const PEER: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../build/crates/hypha-cxx/service_lockstep_peer");
pub(crate) const START: &str = "2026-11-01T12:00:00Z";
/// The tick topic: one message every 10 ms for one second, the window of every replay.
pub(crate) const TICK: &str = "/svc_lockstep_tick";
pub(crate) const TICKS: u64 = 100;
pub(crate) const RUN_LIMIT: Duration = Duration::from_secs(90);

/// Kills the process when the test ends, passed or not.
pub(crate) struct Killed(pub(crate) Child);

impl Drop for Killed {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A `hypha launch unit` supervisor, stopped with SIGTERM so it stops its node too.
pub(crate) struct Supervisor(pub(crate) Child);

impl Drop for Supervisor {
    fn drop(&mut self) {
        let pid = nix::unistd::Pid::from_raw(i32::try_from(self.0.id()).unwrap_or(i32::MAX));
        let _ = nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGTERM);
        let limit = STOP_GRACE_PERIOD.saturating_add(Duration::from_secs(5));
        if wait_until(limit, || !matches!(self.0.try_wait(), Ok(None))) {
            return;
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub(crate) fn scratch(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("service_lockstep_{}_{name}", std::process::id()))
}

pub(crate) fn require_peer() {
    assert!(
        Path::new(PEER).is_file(),
        "the C++ peer is not built at {PEER}; run: cmake --build build --target service_lockstep_peer"
    );
}

/// One `std_msgs/UInt32` on [`TICK`] every 10 ms from `START`.
pub(crate) fn write_recording(path: &Path) {
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
pub(crate) struct Peer {
    pub(crate) name: String,
    pub(crate) role: &'static str,
    pub(crate) triggered: bool,
    pub(crate) serves: Option<String>,
    pub(crate) calls: Option<String>,
    /// The topic of the `tick` trigger, [`TICK`] unless set.
    pub(crate) tick: Option<String>,
    /// The topic of the `out` output, which each step sends on.
    pub(crate) output: Option<String>,
    /// The rate of the second cycle of a pair or a twin.
    pub(crate) pair_rate: Option<&'static str>,
}

impl Peer {
    pub(crate) fn new(label: &str, role: &'static str) -> Self {
        let name = format!("svc_{label}_{}", std::process::id());
        Self {
            name,
            role,
            triggered: false,
            serves: None,
            calls: None,
            tick: None,
            output: None,
            pair_rate: None,
        }
    }

    /// Triggered by `topic` instead of [`TICK`].
    pub(crate) fn triggered_by(mut self, topic: &str) -> Self {
        self.triggered = true;
        self.tick = Some(topic.to_owned());
        self
    }

    pub(crate) fn sending(mut self, topic: &str) -> Self {
        self.output = Some(topic.to_owned());
        self
    }

    pub(crate) fn pair_rate(mut self, hz: &'static str) -> Self {
        self.pair_rate = Some(hz);
        self
    }

    pub(crate) fn triggered(mut self) -> Self {
        self.triggered = true;
        self
    }

    pub(crate) fn serving(mut self, service: &str) -> Self {
        self.serves = Some(service.to_owned());
        self
    }

    pub(crate) fn calling(mut self, service: &str) -> Self {
        self.calls = Some(service.to_owned());
        self
    }

    pub(crate) fn parameters(&self) -> PathBuf {
        let mut text = format!("[node]\nname = \"{}\"\n", self.name);
        if self.triggered {
            let tick = self.tick.as_deref().unwrap_or(TICK);
            let _ = write!(text, "[ports.inputs.tick]\ntopic = \"{tick}\"\nsilence_ms = 1000\n");
        } else if self.role != "plain" {
            text.push_str("[cycle]\nrate_hz = 100.0\n");
        }
        if let Some(service) = &self.serves {
            let _ = write!(text, "[ports.servers.serve]\nservice = \"{service}\"\n");
        }
        if let Some(service) = &self.calls {
            let _ = write!(text, "[ports.clients.call]\nservice = \"{service}\"\n");
        }
        if let Some(topic) = &self.output {
            let _ = write!(text, "[ports.outputs.out]\ntopic = \"{topic}\"\n");
        }
        let path = scratch(&format!("{}.toml", self.name));
        std::fs::write(&path, text).expect("the peer's parameter file");
        path
    }

    pub(crate) fn environment(&self) -> Vec<(&'static str, &'static str)> {
        vec![
            ("HYPHA_USE_SIM_TIME", "1"),
            ("SERVICE_PEER_ROLE", self.role),
            ("SERVICE_PEER_TRIGGERED", if self.triggered { "1" } else { "0" }),
            ("SERVICE_PEER_OUTPUT", if self.output.is_some() { "1" } else { "0" }),
            ("SERVICE_PEER_PAIR_RATE", self.pair_rate.unwrap_or("")),
        ]
    }

    pub(crate) fn spawn(&self, output: &Path) -> Killed {
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
pub(crate) fn service(label: &str) -> String {
    format!("/svc_lockstep_{}_{label}", std::process::id())
}

/// The fact lines of a peer's output: those with a step time, in order.
pub(crate) fn facts(text: &str) -> Vec<String> {
    text.lines().filter(|line| line.contains(" t=")).map(str::to_owned).collect()
}

/// How one run is made.
#[derive(Clone, Copy)]
pub(crate) struct Run {
    pub(crate) chunk_ms: u64,
    pub(crate) echo: bool,
}

/// Replay the recording once with `peers`, and return each peer's fact lines.
pub(crate) fn replay(peers: &[Peer], run: Run) -> Vec<Vec<String>> {
    replay_full(peers, &[], run, "max", &mut || {})
}

/// [`replay`] at `--rate rate`, with `outside` started first and left out of `--nodes`
/// (they take no part in the replay), and `during` run once the player started.
pub(crate) fn replay_full(
    peers: &[Peer],
    outside: &[Peer],
    run: Run,
    rate: &str,
    during: &mut dyn FnMut(),
) -> Vec<Vec<String>> {
    require_peer();
    let recording = scratch("tick.mcap");
    write_recording(&recording);
    let outputs: Vec<PathBuf> =
        peers.iter().map(|peer| scratch(&format!("{}.out", peer.name))).collect();
    let outside_outputs: Vec<PathBuf> =
        outside.iter().map(|peer| scratch(&format!("{}.out", peer.name))).collect();
    let others: Vec<Killed> =
        outside.iter().zip(&outside_outputs).map(|(peer, output)| peer.spawn(output)).collect();
    // A peer outside prints its start once its server exists.
    for output in &outside_outputs {
        let started = || std::fs::read_to_string(output).unwrap_or_default().contains("start");
        assert!(wait_until(Duration::from_secs(10), started), "a peer outside did not start");
    }
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
        .args(["--clock", "--start-utc", START, "--rate", rate, "--chunk", &chunk])
        .arg("--nodes")
        .args(&names)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("hypha play");
    during();
    let mut status = None;
    let ended = wait_until(RUN_LIMIT, || {
        status = player.try_wait().expect("the player status");
        status.is_some()
    });
    assert!(ended, "the replay did not end within {RUN_LIMIT:?}");
    let status = status.expect("an ended player has a status");
    let mut stderr = String::new();
    if let Some(mut pipe) = player.stderr.take() {
        let _ = std::io::Read::read_to_string(&mut pipe, &mut stderr);
    }
    assert!(status.success(), "hypha play failed: {status}: {stderr}");
    drop(echo);
    drop(children);
    drop(others);
    let lines = outputs
        .iter()
        .map(|output| facts(&std::fs::read_to_string(output).unwrap_or_default()))
        .collect();
    for path in outputs.iter().chain(&outside_outputs).chain([&recording]) {
        let _ = std::fs::remove_file(path);
    }
    for peer in peers.iter().chain(outside) {
        let _ = std::fs::remove_file(scratch(&format!("{}.toml", peer.name)));
    }
    lines
}

/// The step time of a fact line.
pub(crate) fn time_of(line: &str) -> i64 {
    let field = line.split_whitespace().find_map(|word| word.strip_prefix("t=")).expect("t=");
    field.parse().expect("a time")
}

/// The time and text of each line of `lines` whose fact is `fact`.
pub(crate) fn each(lines: &[String], fact: &str) -> Vec<(i64, String)> {
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
pub(crate) fn assert_strict_hops(client: &[String], hops: i64) {
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

pub(crate) fn assert_same(first: &[Vec<String>], other: &[Vec<String>], what: &str) {
    for (index, (a, b)) in first.iter().zip(other).enumerate() {
        assert_eq!(a, b, "participant {index} differs {what}");
    }
}

pub(crate) fn two_nodes(label: &str, triggered: bool) -> Vec<Peer> {
    let name = service(label);
    let mut server = Peer::new(&format!("{label}_server"), "server").serving(&name);
    let mut client = Peer::new(&format!("{label}_client"), "client").calling(&name);
    if triggered {
        server = server.triggered();
        client = client.triggered();
    }
    vec![client, server]
}
