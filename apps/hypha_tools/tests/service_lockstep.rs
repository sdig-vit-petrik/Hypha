//! A replay with services between participants repeats exactly (FSR-44,
//! `docs/design/native-services.md` §4): real C++ participants (`service_lockstep_peer`)
//! with `Requests` and `Calls` ports under `hypha play --clock`.
//!
//! Each participant prints one line per fact of a step, stamped with the step's time. A run
//! is the lines of every participant; two runs that differ only in `--chunk`, in an attached
//! `hypha topic echo`, or in the pass of a `--loop`, print the same lines. The lines also
//! show the strict cut: a request stamped `t` is served at the server's first step after
//! `t`, and its answer is read at the client's first step after that.
//!
//! The same participant joins two cycles of one node by a topic (#307,
//! `docs/execution-model.md` §11.1): the reader's steps read what the sender's earlier steps
//! sent, whatever the chunk and whichever cycle a turn runs first.
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
    /// The topic of the `tick` trigger, [`TICK`] unless set.
    tick: Option<String>,
    /// The topic of the `out` output, which each step sends on.
    output: Option<String>,
    /// The rate of the second cycle of a pair or a twin.
    pair_rate: Option<&'static str>,
    /// The topics of a `topic` participant.
    link: Option<Link>,
}

/// Two cycles of one node joined by a topic: the role `topic` of the peer.
#[derive(Clone)]
struct Link {
    /// The cycle that sends on the topic: `main` (the node's cycle) or `second`.
    sender: &'static str,
    /// How the reader steps: `rate`, on each `tick`, or on each sample of the `link`.
    reader: &'static str,
    /// The reader sends back to the sender: a feedback loop.
    feedback: bool,
    /// The sender also serves the service the reader calls.
    calls: bool,
}

impl Peer {
    fn new(label: &str, role: &'static str) -> Self {
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
            link: None,
        }
    }

    /// Two cycles of one node, the `sender` sending on a topic that the other reads,
    /// stepping as `reader` says.
    fn linked(label: &str, sender: &'static str, reader: &'static str) -> Self {
        let mut peer = Self::new(label, "topic");
        peer.link = Some(Link { sender, reader, feedback: false, calls: false });
        peer
    }

    fn feedback(mut self) -> Self {
        self.link.as_mut().expect("a linked peer").feedback = true;
        self
    }

    /// The sender serves `service`, which the reader calls.
    fn linked_calls(mut self, service: &str) -> Self {
        self.link.as_mut().expect("a linked peer").calls = true;
        self.serves = Some(service.to_owned());
        self.calls = Some(service.to_owned());
        self
    }

    /// The parameters of a `topic` participant.
    fn link_parameters(&self, link: &Link) -> String {
        let mut text = format!("[node]\nname = \"{}\"\n", self.name);
        // The node's own cycle has a rate unless it is the reader and steps on triggers.
        if link.sender == "main" || link.reader == "rate" {
            text.push_str("[cycle]\nrate_hz = 100.0\n");
        }
        let topic =
            |which: &str| format!("/svc_lockstep_{}_{}_{which}", std::process::id(), self.name);
        let link_topic = topic("link");
        let _ = write!(
            text,
            "[ports.outputs.link]\ntopic = \"{link_topic}\"\n[ports.inputs.link_in]\ntopic = \"{link_topic}\"\nsilence_ms = 1000\n"
        );
        if link.feedback {
            let back = topic("back");
            let _ = write!(
                text,
                "[ports.outputs.back]\ntopic = \"{back}\"\n[ports.inputs.back_in]\ntopic = \"{back}\"\nsilence_ms = 1000\n"
            );
        }
        if link.reader == "tick" {
            let _ = write!(text, "[ports.inputs.tick]\ntopic = \"{TICK}\"\nsilence_ms = 1000\n");
        }
        if let (Some(serves), Some(calls)) = (&self.serves, &self.calls) {
            let _ = write!(
                text,
                "[ports.servers.serve]\nservice = \"{serves}\"\n[ports.clients.call]\nservice = \"{calls}\"\n"
            );
        }
        text
    }

    /// Triggered by `topic` instead of [`TICK`].
    fn triggered_by(mut self, topic: &str) -> Self {
        self.triggered = true;
        self.tick = Some(topic.to_owned());
        self
    }

    fn sending(mut self, topic: &str) -> Self {
        self.output = Some(topic.to_owned());
        self
    }

    fn pair_rate(mut self, hz: &'static str) -> Self {
        self.pair_rate = Some(hz);
        self
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
        if let Some(link) = &self.link {
            let path = scratch(&format!("{}.toml", self.name));
            std::fs::write(&path, self.link_parameters(link)).expect("the peer's parameter file");
            return path;
        }
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

    fn environment(&self) -> Vec<(&'static str, &'static str)> {
        vec![
            ("HYPHA_USE_SIM_TIME", "1"),
            ("SERVICE_PEER_ROLE", self.role),
            ("SERVICE_PEER_TRIGGERED", if self.triggered { "1" } else { "0" }),
            ("SERVICE_PEER_OUTPUT", if self.output.is_some() { "1" } else { "0" }),
            ("SERVICE_PEER_PAIR_RATE", self.pair_rate.unwrap_or("")),
            ("SERVICE_PEER_SENDER", self.link.as_ref().map_or("", |link| link.sender)),
            ("SERVICE_PEER_READER", self.link.as_ref().map_or("", |link| link.reader)),
            ("SERVICE_PEER_LOOP", flag(self.link.as_ref().is_some_and(|link| link.feedback))),
            ("SERVICE_PEER_CALLS", flag(self.link.as_ref().is_some_and(|link| link.calls))),
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

fn flag(on: bool) -> &'static str {
    if on { "1" } else { "0" }
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
    replay_full(peers, &[], run, "max", &mut || {})
}

/// [`replay`] at `--rate rate`, with `outside` started first and left out of `--nodes`
/// (they take no part in the replay), and `during` run once the player started.
fn replay_full(
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
    for output in &outside_outputs {
        let began = Instant::now();
        while !std::fs::read_to_string(output).unwrap_or_default().contains("start") {
            assert!(began.elapsed() < Duration::from_secs(10), "a peer outside did not start");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    // Its setup has begun; give its endpoints a moment.
    std::thread::sleep(Duration::from_millis(300));
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

/// Run `peers` at `--chunk 50` and `--chunk 200` and check both print the same lines.
fn same_across_chunks(peers: &[Peer]) -> Vec<Vec<String>> {
    let first = replay(peers, Run { chunk_ms: 50, echo: false });
    let larger = replay(peers, Run { chunk_ms: 200, echo: false });
    assert_same(&first, &larger, "between --chunk 50 and --chunk 200");
    first
}

#[test]
fn two_nodes_that_each_serve_and_call_the_other_repeat_exactly() {
    let (a, b) = (service("mutual_a"), service("mutual_b"));
    let peers = vec![
        Peer::new("mutual_a", "mutual").serving(&a).calling(&b),
        Peer::new("mutual_b", "mutual").serving(&b).calling(&a),
    ];
    let first = same_across_chunks(&peers);
    assert_strict_hops(&first[0], 1);
    assert_strict_hops(&first[1], 1);
}

#[test]
fn a_pair_in_one_waitset_at_two_rates_repeats_exactly() {
    let name = service("pair_rates");
    // The client at 100 Hz, its server at 50 Hz: an answer is read at the client's first
    // step after the server's first step after the call.
    let peers =
        vec![Peer::new("pair_rates", "pair").serving(&name).calling(&name).pair_rate("50.0")];
    let first = same_across_chunks(&peers);
    let calls: BTreeMap<String, i64> =
        each(&first[0], "call").into_iter().map(|(t, text)| (text, t)).collect();
    let results = each(&first[0], "result");
    assert!(results.len() >= 10, "too few answers: {:?}", first[0]);
    for (read, text) in results {
        let (request, served) = text.split_once('@').unwrap();
        let served: i64 = served.parse().unwrap();
        let called = calls[request];
        let first_server_step = (called / 20 + 1) * 20;
        assert_eq!(served, first_server_step, "{text} called at {called}");
        assert_eq!(read, served + 10, "{text} read at {read}");
    }
}

#[test]
fn a_triggered_client_and_a_rate_server_in_one_waitset_repeat_exactly() {
    let name = service("pair_triggered");
    let peers = vec![
        Peer::new("pair_triggered", "pair")
            .serving(&name)
            .calling(&name)
            .triggered()
            .pair_rate("100.0"),
    ];
    let first = same_across_chunks(&peers);
    assert_strict_hops(&first[0], 1);
}

/// A server triggered only by its client's output: the call of the client's step at `t`
/// and the output that triggers the server at `t` carry one stamp, and the request is cut
/// before `t`, so the server serves it at its next trigger, the client's next step.
#[test]
fn a_server_triggered_by_its_clients_topic_serves_at_the_next_trigger() {
    let name = service("by_client");
    let topic = format!("/svc_lockstep_{}_client_out", std::process::id());
    let peers = vec![
        Peer::new("by_client_client", "client").calling(&name).sending(&topic),
        Peer::new("by_client_server", "server").serving(&name).triggered_by(&topic),
    ];
    let first = same_across_chunks(&peers);
    assert_strict_hops(&first[0], 1);
}

#[test]
fn two_cycles_of_one_node_on_one_port_name_are_served_in_one_order() {
    let name = service("twin");
    let peers = vec![
        Peer::new("twin_client", "twin").calling(&name).pair_rate("100.0"),
        Peer::new("twin_server", "server").serving(&name),
    ];
    let first = same_across_chunks(&peers);
    let served = each(&first[1], "serve");
    let tied = served.windows(2).any(|pair| pair[0].0 == pair[1].0);
    assert!(tied, "the two clients' requests share a step: {served:?}");
}

/// A server that takes no part in the replay answers a cycle's client at the step after
/// the answer came, not by its stamp: answered, never a timeout of an answer in hand.
#[test]
fn a_cycle_client_reads_the_answer_of_a_plain_server() {
    let name = service("plain");
    let outside = [Peer::new("plain_server", "plain").serving(&name)];
    let peers = [Peer::new("plain_client", "client").calling(&name)];
    let run = Run { chunk_ms: 50, echo: false };
    let lines = replay_full(&peers, &outside, run, "max", &mut || {});
    let results = each(&lines[0], "result");
    assert!(results.len() >= 10, "too few answers: {:?}", lines[0]);
    assert!(results.iter().all(|(_, text)| text.ends_with("@plain")), "{results:?}");
}

/// `hypha service call` stamps its request with the host's time, after the recording's: a
/// cycle server serves it at its first step after it came.
#[test]
fn the_cli_calls_a_cycle_server_during_a_replay() {
    let name = service("cli");
    let peers = [Peer::new("cli_server", "server").serving(&name)];
    let mut answer = None;
    let mut call = || {
        let output = Command::new(env!("CARGO_BIN_EXE_hypha"))
            // A generous bound: how soon a loaded host gives the paced server its step is
            // the host's speed; a call that is never served still fails at the bound.
            .args(["service", "call", &name, "text: shell", "--timeout", "30"])
            .output()
            .expect("hypha service call");
        answer = Some(output);
    };
    let run = Run { chunk_ms: 50, echo: false };
    let lines = replay_full(&peers, &[], run, "0.5", &mut call);
    let output = answer.expect("the call ran");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("shell@"),
        "{}: {stdout} {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(each(&lines[0], "serve").iter().any(|(_, text)| text.starts_with("shell@")));
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

/// The fact lines of each cycle of one participant, by the name each line starts with: two
/// cycles of one node print into one file, in an order that only the turns decide.
fn by_cycle(lines: &[String]) -> BTreeMap<String, Vec<String>> {
    let mut cycles: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for line in lines {
        let who = line.split_whitespace().next().unwrap_or("").to_owned();
        cycles.entry(who).or_default().push(line.clone());
    }
    cycles
}

/// Replay the `topic` participant built by `peer` for each sender in `senders`, at
/// `--chunk 50` and `--chunk 200`, and check that each cycle prints the same lines in every
/// run; return them.
fn linked_alike(
    senders: &[&'static str],
    peer: impl Fn(&'static str) -> Peer,
) -> BTreeMap<String, Vec<String>> {
    let mut first: Option<(String, BTreeMap<String, Vec<String>>)> = None;
    for &sender in senders {
        for chunk_ms in [50, 200] {
            let lines = replay(&[peer(sender)], Run { chunk_ms, echo: false });
            let cycles = by_cycle(&lines[0]);
            let what = format!("sender {sender}, --chunk {chunk_ms}");
            match &first {
                None => first = Some((what, cycles)),
                Some((base, expected)) => {
                    for (who, lines) in expected {
                        assert_eq!(
                            Some(lines),
                            cycles.get(who),
                            "the {who} cycle differs between {base} and {what}"
                        );
                    }
                }
            }
        }
    }
    first.expect("a run").1
}

/// The value the reader read at each of its steps.
fn reader_reads(cycles: &BTreeMap<String, Vec<String>>) -> Vec<(i64, String)> {
    let reads = each(&cycles["reader"], "read");
    assert!(reads.len() >= 90, "the reader ran too few steps: {reads:?}");
    reads
}

/// A rate cycle reads at `t` the sample its node's other cycle sent at `t - 10 ms`, whatever
/// the chunk and whichever cycle a turn runs first (#307).
#[test]
fn a_topic_between_two_cycles_of_one_node_is_read_alike_whatever_the_chunk_and_the_order() {
    let cycles = linked_alike(&["main", "second"], |sender| {
        Peer::linked(&format!("link_{sender}"), sender, "rate")
    });
    let reads = reader_reads(&cycles);
    for (t, read) in reads.iter().skip(1) {
        assert_eq!(read, &(t - 10).to_string(), "the reader's step at {t}: {reads:?}");
    }
}

/// Two cycles of one node that each read the other's topic: both advance, each reading the
/// other's step one period back.
#[test]
fn a_feedback_loop_between_two_cycles_of_one_node_advances_and_repeats() {
    let cycles = linked_alike(&["main", "second"], |sender| {
        Peer::linked(&format!("loop_{sender}"), sender, "rate").feedback()
    });
    let reads = reader_reads(&cycles);
    let backs = each(&cycles["sender"], "back");
    assert!(backs.len() >= 90, "the sender ran too few steps: {backs:?}");
    for (t, read) in reads.iter().skip(1).chain(backs.iter().skip(1)) {
        assert_eq!(read, &(t - 10).to_string(), "the step at {t}: {reads:?} {backs:?}");
    }
}

/// A triggered reader: on its node's other cycle's samples it reads the one that stepped
/// it; on the player's ticks, a step at `t` reads the other cycle's sample of `t - 10 ms`
/// (an input cut strictly before `t`) or of `t` (one read through `t`, #299), alike in
/// every run.
#[test]
fn a_triggered_cycle_reads_the_topic_of_a_cycle_of_its_node_alike() {
    let cycles = linked_alike(&["main", "second"], |sender| {
        Peer::linked(&format!("trig_link_{sender}"), sender, "link")
    });
    let reads = reader_reads(&cycles);
    for (t, read) in &reads {
        assert_eq!(read, &t.to_string(), "the reader's step at {t}: {reads:?}");
    }
    let cycles = linked_alike(&["main", "second"], |sender| {
        Peer::linked(&format!("trig_tick_{sender}"), sender, "tick")
    });
    let reads = reader_reads(&cycles);
    // The first tick comes before the other cycle's first step, the second with it.
    for (t, read) in reads.iter().skip(2) {
        let read: i64 = read.parse().unwrap_or_else(|_| panic!("one sample at {t}: {reads:?}"));
        assert!(read == t - 10 || read == *t, "the reader's step at {t} read {read}: {reads:?}");
    }
}

/// A topic and a service between the same two cycles of one node: the topic is read one
/// period back, the answer at the step after the serve.
#[test]
fn a_topic_and_a_service_between_two_cycles_of_one_node_repeat_exactly() {
    let cycles = linked_alike(&["main", "second"], |sender| {
        Peer::linked(&format!("mix_{sender}"), sender, "rate")
            .linked_calls(&service(&format!("mix_{sender}")))
    });
    let reads = reader_reads(&cycles);
    for (t, read) in reads.iter().skip(1) {
        assert_eq!(read, &(t - 10).to_string(), "the reader's step at {t}: {reads:?}");
    }
    assert_strict_hops(&cycles["reader"], 1);
}
