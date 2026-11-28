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

#[path = "service_lockstep/harness.rs"]
mod harness;
use harness::*;
use hypha_testkit::wait_until;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs::File;
use std::process::{Command, Stdio};

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
    let passes =
        |text: &str| -> Vec<Vec<String>> { text.split("start ").skip(1).map(facts).collect() };
    let looped = wait_until(RUN_LIMIT, || {
        assert!(player.0.try_wait().expect("the player").is_none(), "the player exited");
        outputs
            .iter()
            .all(|output| passes(&std::fs::read_to_string(output).unwrap_or_default()).len() >= 3)
    });
    assert!(looped, "no two passes within {RUN_LIMIT:?}");
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
