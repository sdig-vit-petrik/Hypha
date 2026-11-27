//! Services over iceoryx2 request-response (#132, stage 1a): the failure table of
//! `docs/design/native-services.md` §5, across processes where a process matters,
//! creation races, many clients, the bounds, and the measurements (`--no-capture`).

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    reason = "transport tests: a failed step is a test failure"
)]

use std::io::{BufRead as _, BufReader, Write as _};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use hypha_core::port::{
    CallSendError, PortError, ReplyError, ServiceError, ServicePolicy, ServiceSpec,
};
use hypha_iceoryx2::{CallEvent, CallReport, Iox2Client, Iox2Transport, Liveness};
use hypha_testkit::unique_name;
use iceoryx2::prelude::*;

const V1: &str = "09fb03525b03e7ea1fd3992bafd87e16";
const V2: &str = "29fb03525b03e7ea1fd3992bafd87e16";

fn spec(name: &str) -> ServiceSpec {
    ServiceSpec::new(name, "std_srvs/SetBool", V1)
}

fn service_name() -> String {
    format!("/{}", unique_name("svc"))
}

fn new_transport() -> Iox2Transport {
    Iox2Transport::with_name(&unique_name("node")).expect("a node")
}

/// A helper process; killed when dropped.
struct Helper {
    child: Child,
    stdout: BufReader<ChildStdout>,
}

impl Helper {
    fn start(service: &str, version: &str, mode: &[&str]) -> Self {
        Self::start_named(&unique_name("helper"), service, version, mode)
    }

    fn start_named(node: &str, service: &str, version: &str, mode: &[&str]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_hypha_iceoryx2_service_helper"))
            .args([node, service, version])
            .args(mode)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn the service helper");
        let stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
        Self { child, stdout }
    }

    fn line(&mut self) -> String {
        let mut line = String::new();
        self.stdout.read_line(&mut line).expect("a line of the helper");
        line.trim().to_owned()
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }

    fn kill(&mut self) {
        self.child.kill().expect("SIGKILL the helper");
        self.child.wait().expect("reap the helper");
    }

    /// Let a `call-and-exit` helper exit normally.
    fn finish(&mut self) {
        let mut stdin = self.child.stdin.take().expect("piped stdin");
        let _ = writeln!(stdin);
        drop(stdin);
        self.child.wait().expect("the helper exits");
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Wait in the client's response event, polling once per wake, until `count` reports
/// came or `limit` passed.
fn collect(client: &mut Iox2Client, count: usize, limit: Duration) -> Vec<CallReport> {
    let wake = client.open_listener().expect("the response listener");
    let start = Instant::now();
    let mut reports = Vec::new();
    loop {
        client.poll(&mut reports);
        if reports.len() >= count || start.elapsed() > limit {
            return reports;
        }
        wake.timed_wait(|_| {}, Duration::from_millis(200)).expect("wait for a response");
    }
}

fn load_average() -> String {
    let output = Command::new("sysctl").args(["-n", "vm.loadavg"]).output();
    match output {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).trim().to_owned(),
        _ => std::fs::read_to_string("/proc/loadavg").unwrap_or_default().trim().to_owned(),
    }
}

#[test]
fn a_call_with_no_server_is_refused_at_once_and_nothing_is_pending() {
    let name = service_name();
    let transport = new_transport();
    let mut client = transport.service_client(&spec(&name), None).expect("a client");
    let started = Instant::now();
    assert_eq!(client.send(&[1], 0), Err(CallSendError::NoServer));
    assert!(
        started.elapsed() < Duration::from_millis(100),
        "the send returns at once: {:?}",
        started.elapsed()
    );
    assert_eq!(client.in_flight(), 0);
    assert!(!client.server_available());
    assert_eq!(client.probe_server(), Liveness::NoServer);
    assert_eq!(client.counters().no_server, 1);
}

#[test]
fn a_framed_round_trip_between_two_processes() {
    let name = service_name();
    let transport = new_transport();
    let mut client = transport.service_client(&spec(&name), None).expect("a client");
    let graph = transport.graph_listener().expect("the graph listener");
    let mut server = Helper::start(&name, "v1", &["serve"]);
    assert_eq!(server.line(), "ready");
    // The server's creation wakes the graph listeners, so a waiting client can send.
    let mut woken = false;
    graph.try_wait(|_| woken = true).expect("read the graph event");
    assert!(woken || client.server_available(), "the server's creation is announced");
    assert!(client.server_available());
    assert_eq!(client.probe_server(), Liveness::Alive);

    let wake = client.open_listener().expect("the response listener");
    let rounds: u64 = 2000;
    let mut samples = Vec::new();
    for round in 1..=rounds {
        let data = u8::from(round % 2 == 0);
        let start = Instant::now();
        let id = client.send(&[data], 5).expect("send");
        let report = loop {
            let mut reports = Vec::new();
            client.poll(&mut reports);
            if let Some(report) = reports.pop() {
                break report;
            }
            wake.timed_wait(|_| {}, Duration::from_secs(2)).expect("wait");
            assert!(start.elapsed() < Duration::from_secs(5), "no response to call {round}");
        };
        samples.push(start.elapsed());
        assert_eq!(report.id, id);
        assert!(!report.late);
        let CallEvent::Answered(payload) = report.event else { panic!("{report:?}") };
        assert_eq!(payload.first().copied(), Some(data), "SetBool echo");
    }
    samples.sort_unstable();
    let at = |per_mille: usize| {
        let index = (samples.len() * per_mille / 1000).min(samples.len() - 1);
        samples[index]
    };
    println!(
        "service_transport: round trip, {rounds} SetBool calls between two processes: median {:?}, \
         p90 {:?}, p99 {:?}, max {:?}; load average {}",
        at(500),
        at(900),
        at(990),
        samples.last().copied().unwrap_or_default(),
        load_average()
    );
    assert_eq!(client.counters().sent, rounds);
    assert_eq!(client.counters().invalid_responses, 0);
}

#[test]
fn a_second_server_is_refused_on_the_name_in_the_same_and_in_another_version() {
    let name = service_name();
    let node = unique_name("holder");
    let mut first = Helper::start_named(&node, &name, "v1", &["serve"]);
    assert_eq!(first.line(), "ready");
    let transport = new_transport();
    for version in [V1, V2] {
        let refused = transport.serve(&ServiceSpec::new(&name, "std_srvs/SetBool", version));
        let Err(ServiceError::AlreadyServed(holder)) = refused else {
            panic!("version {version}: a second server must be refused, got {:?}", refused.err());
        };
        assert_eq!(holder.node, node, "the refusal names the holder");
        assert_eq!(holder.pid, Some(i32::try_from(first.pid()).unwrap()));
        assert_eq!(holder.served, Some(("std_srvs/SetBool".to_owned(), "09fb0352".to_owned())));
        let message = ServiceError::AlreadyServed(holder).to_string();
        assert!(
            message.contains("std_srvs/SetBool (09fb0352)") && message.contains(&node),
            "{message}"
        );
    }
}

#[test]
fn several_processes_creating_the_server_at_once_give_exactly_one_server() {
    let name = service_name();
    let go = std::env::temp_dir().join(unique_name("service_race_go"));
    let go_text = go.to_str().expect("utf-8 path").to_owned();
    let mut racers: Vec<Helper> = (0..6)
        .map(|index| {
            let version = if index % 2 == 0 { "v1" } else { "v2" };
            Helper::start(&name, version, &["race", &go_text])
        })
        .collect();
    std::thread::sleep(Duration::from_millis(300));
    std::fs::write(&go, b"go").expect("start the race");
    let results: Vec<String> = racers.iter_mut().map(Helper::line).collect();
    let _ = std::fs::remove_file(&go);
    let won = results.iter().filter(|line| *line == "won").count();
    let refused = results
        .iter()
        .filter(|line| line.starts_with("refused already served as std_srvs/SetBool ("))
        .count();
    println!("service_transport: creation race of 6 processes over two versions: {results:?}");
    assert_eq!(won, 1, "exactly one server: {results:?}");
    assert_eq!(refused, 5, "every other one is refused as already served: {results:?}");
}

#[test]
fn a_server_killed_during_a_call_is_found_dead_and_a_new_server_under_another_name_reclaims_it() {
    let name = service_name();
    let transport = new_transport();
    let mut client = transport.service_client(&spec(&name), None).expect("a client");
    let mut holder = Helper::start(&name, "v1", &["hold"]);
    assert_eq!(holder.line(), "ready");
    let id = client.send(&[1], 0).expect("send");
    assert_eq!(holder.line(), format!("holding {}", id.0));

    let killed = Instant::now();
    holder.kill();
    // The transport alone: the call stays connected and the server keeps its slot.
    let mut reports = Vec::new();
    client.poll(&mut reports);
    assert!(reports.is_empty(), "no outcome without the probe: {reports:?}");
    assert!(client.server_available(), "a crashed server keeps its slot");
    // At the call's timeout: still connected, so the client probes the server's node.
    assert!(client.expire(id), "the expired call is still connected");
    let probe = Instant::now();
    let liveness = client.probe_server();
    let probe_cost = probe.elapsed();
    assert_eq!(liveness, Liveness::Dead, "the probe finds the killed server's node dead");
    let again = Instant::now();
    assert_eq!(client.probe_server(), Liveness::Dead, "within a second the last result is given");
    let cached_cost = again.elapsed();
    println!(
        "service_transport: killed server found dead {:?} after the kill; one probe {probe_cost:?}, \
         a probe within its second {cached_cost:?}; load average {}",
        killed.elapsed(),
        load_average()
    );

    // A new server under another node name reclaims the dead holder and serves.
    let other = new_transport();
    let new_server = other.serve(&spec(&name));
    assert!(new_server.is_ok(), "the dead holder is reclaimed: {:?}", new_server.err());
    let reports = collect(&mut client, 1, Duration::from_secs(2));
    assert_eq!(reports.len(), 1, "the reclaim disconnects the old call: {reports:?}");
    assert_eq!(reports[0].event, CallEvent::Lost);
    assert!(reports[0].late, "it had timed out");
    assert_eq!(client.in_flight(), 0);
}

#[test]
fn a_server_that_exits_normally_loses_its_calls_at_the_next_poll() {
    let name = service_name();
    let server_side = new_transport();
    let mut server = server_side.serve(&spec(&name)).expect("a server");
    let client_side = new_transport();
    let mut client = client_side.service_client(&spec(&name), None).expect("a client");
    let id = client.send(&[1], 0).expect("send");
    let held = server.receive().expect("receive").expect("a request");
    assert_eq!(held.sequence, id.0);
    drop(server);
    let reports = collect(&mut client, 1, Duration::from_secs(2));
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].event, CallEvent::Lost);
    // The name is free at once for a new server.
    assert!(server_side.serve(&spec(&name)).is_ok());
}

#[test]
fn refusals_and_failures_reach_the_client_with_their_reasons() {
    for (mode, expected) in [
        ("refuse", CallEvent::Refused("nope".to_owned())),
        ("fail", CallEvent::Failed("boom".to_owned())),
    ] {
        let name = service_name();
        let mut server = Helper::start(&name, "v1", &[mode]);
        assert_eq!(server.line(), "ready");
        let transport = new_transport();
        let mut client = transport.service_client(&spec(&name), None).expect("a client");
        client.send(&[1], 0).expect("send");
        let reports = collect(&mut client, 1, Duration::from_secs(5));
        assert_eq!(reports.len(), 1, "{mode}");
        assert_eq!(reports[0].event, expected, "{mode}");
    }
}

#[test]
fn a_request_and_a_response_above_their_bounds() {
    let name = service_name();
    let mut server = Helper::start(&name, "v1", &["big"]);
    assert_eq!(server.line(), "ready");
    let transport = new_transport();
    let mut client = transport.service_client(&spec(&name), None).expect("a client");
    let bound = ServicePolicy::default_service().max_request_bytes;
    assert_eq!(
        client.send(&vec![0; bound + 1], 0),
        Err(CallSendError::TooLarge { requested: bound + 1, limit: bound }),
        "a request above the bound is refused and nothing is sent"
    );
    assert_eq!(client.counters().sent, 0);
    client.send(&[1], 0).expect("a small request");
    assert_eq!(
        server.line(),
        format!("reply Err(TooLarge {{ requested: {}, limit: {bound} }})", bound + 1)
    );
    let reports = collect(&mut client, 1, Duration::from_secs(5));
    assert_eq!(
        reports.first().map(|r| r.event.clone()),
        Some(CallEvent::Refused(format!(
            "response {} B above bound {bound} B, policy defaultService",
            bound + 1
        )))
    );
}

#[test]
fn a_client_that_leaves_has_its_held_requests_dropped_and_counted() {
    let name = service_name();
    let transport = new_transport();
    let mut server = transport.serve(&spec(&name)).expect("a server");
    let wake = server.open_listener().expect("the request listener");
    let mut leaving = Helper::start(&name, "v1", &["call-and-exit"]);
    assert_eq!(leaving.line(), "sent");
    wake.timed_wait(|_| {}, Duration::from_secs(2)).expect("wait");
    let held = server.receive().expect("receive").expect("the request");
    assert_eq!(server.drop_abandoned(), 0, "the client is still there");
    leaving.finish();
    // A reply to the gone client is refused as disconnected, and the slot is freed.
    assert_eq!(server.reply(held.handle, &[1], 0), Err(ReplyError::Disconnected));
    assert_eq!(server.held(), 0);
    assert_eq!(server.counters().abandoned, 1);

    let mut second = Helper::start(&name, "v1", &["call-and-exit"]);
    assert_eq!(second.line(), "sent");
    wake.timed_wait(|_| {}, Duration::from_secs(2)).expect("wait");
    let held = server.receive().expect("receive").expect("the request");
    second.finish();
    assert_eq!(server.drop_abandoned(), 1, "the turn drops the slot of the gone client");
    assert_eq!(server.counters().abandoned, 2);
    assert_eq!(server.reply(held.handle, &[1], 0), Err(ReplyError::UnknownRequest));
}

#[test]
fn a_response_that_does_not_verify_is_refused_and_counted() {
    let name = service_name();
    let transport = new_transport();
    let mut client = transport.service_client(&spec(&name), None).expect("a client");
    // A raw iceoryx2 server on the stream that seals for another request.
    let raw = raw_node();
    let stream = raw_stream(&raw, &name);
    let server = stream.server_builder().initial_max_slice_len(64).create().expect("a raw server");
    let id = client.send(&[1], 0).expect("send");
    let request = loop {
        if let Some(request) = server.receive().expect("receive") {
            break request;
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    let header = *request.user_header();
    let wrong = hypha_wire::frame::TopicCode::of(&format!("{name}#response/0/0"));
    let parsed = hypha_wire::frame::FrameHeader::from_raw_for_test(header);
    let sealed = hypha_wire::frame::seal(wrong, parsed.metadata, 0, &[1]);
    let mut response = request.loan_slice_uninit(1).expect("loan");
    *response.user_header_mut() = RawHeader::from_frame(sealed);
    response.write_from_slice(&[1]).send().expect("send");
    let reports = collect(&mut client, 1, Duration::from_secs(2));
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].id, id);
    assert_eq!(reports[0].event, CallEvent::Invalid("crc"));
    assert_eq!(client.counters().invalid_responses, 1);
}

#[test]
fn a_request_that_does_not_verify_is_dropped_and_counted() {
    let name = service_name();
    let transport = new_transport();
    let mut server = transport.serve(&spec(&name)).expect("a server");
    let raw = raw_node();
    let stream = raw_stream(&raw, &name);
    let client = stream.client_builder().initial_max_slice_len(64).create().expect("a raw client");
    let request = client.loan_slice_uninit(1).expect("loan").write_from_slice(&[1]);
    let _pending = request.send().expect("send an unsealed request");
    let start = Instant::now();
    while server.counters().invalid_requests == 0 && start.elapsed() < Duration::from_secs(2) {
        assert_eq!(server.receive().expect("receive"), None);
    }
    assert_eq!(server.counters().invalid_requests, 1);
    assert_eq!(server.held(), 0);
}

#[test]
fn a_client_at_its_limit_is_busy_at_once_and_an_expired_call_held_by_the_server_counts() {
    let name = service_name();
    let server_side = new_transport();
    let mut server = server_side.serve(&spec(&name)).expect("a server");
    let client_side = new_transport();
    let mut client = client_side.service_client(&spec(&name), Some(1)).expect("a client");
    let first = client.send(&[1], 0).expect("send");
    let started = Instant::now();
    assert_eq!(client.send(&[1], 0), Err(CallSendError::Busy { outstanding: 1, expired: 0 }));
    assert!(started.elapsed() < Duration::from_millis(100), "Busy returns at once");
    let held = server.receive().expect("receive").expect("the request");
    assert!(client.expire(first), "still connected");
    let busy = client.send(&[1], 0);
    assert_eq!(busy, Err(CallSendError::Busy { outstanding: 1, expired: 1 }));
    assert!(busy.unwrap_err().to_string().contains("the server still holds 1 expired calls"));
    server.reply(held.handle, &[1], 0).expect("the late answer");
    let reports = collect(&mut client, 1, Duration::from_secs(2));
    assert!(reports[0].late, "a response after the timeout is late");
    assert_eq!(client.counters().late_responses, 1);
    assert!(client.send(&[1], 0).is_ok(), "the slot is free again");
}

#[test]
fn many_clients_each_get_their_own_answers_and_the_limit_keeps_observer_slots() {
    let name = service_name();
    let server_side = new_transport();
    let mut server = server_side.serve(&spec(&name)).expect("a server");
    let policy = ServicePolicy::default_service();
    let transports: Vec<Iox2Transport> = (0..policy.max_clients).map(|_| new_transport()).collect();
    let mut clients: Vec<Iox2Client> = transports
        .iter()
        .map(|t| t.service_client(&spec(&name), None).expect("a declared client"))
        .collect();
    let extra = new_transport();
    assert!(
        matches!(
            extra.service_client(&spec(&name), None),
            Err(ServiceError::Port(PortError::ResourceLimit))
        ),
        "a ninth declared client is refused"
    );
    let tool = Iox2Transport::with_name("_hyphacli_service_test").expect("a CLI node");
    let mut observer =
        tool.service_client(&spec(&name), None).expect("a tool takes an observer slot");

    for (index, client) in clients.iter_mut().enumerate() {
        for _ in 0..policy.max_outstanding {
            client.send(&[u8::try_from(index).unwrap()], 0).expect("send");
        }
    }
    observer.send(&[200], 0).expect("the tool's call");
    let total = (policy.max_clients * policy.max_outstanding) + 1;
    let mut answered = 0;
    let wake = server.open_listener().expect("listener");
    while answered < total {
        while let Some(request) = server.receive().expect("receive") {
            let payload = server.payload(request.handle).expect("payload").to_vec();
            server.reply(request.handle, &payload, 0).expect("reply");
            answered += 1;
        }
        if answered < total {
            wake.timed_wait(|_| {}, Duration::from_millis(200)).expect("wait");
        }
    }
    for (index, client) in clients.iter_mut().enumerate() {
        let reports = collect(client, policy.max_outstanding, Duration::from_secs(2));
        assert_eq!(reports.len(), policy.max_outstanding, "client {index}");
        for report in reports {
            assert_eq!(
                report.event,
                CallEvent::Answered(vec![u8::try_from(index).unwrap()]),
                "client {index}"
            );
        }
    }
    assert_eq!(
        collect(&mut observer, 1, Duration::from_secs(2))[0].event,
        CallEvent::Answered(vec![200])
    );
    assert_eq!(server.counters().answered, u64::try_from(total).unwrap());
}

#[test]
fn a_name_over_219_bytes_is_refused_with_the_limit() {
    let transport = new_transport();
    let long = format!("/{}", "a".repeat(220));
    let refused = transport.serve(&spec(&long)).err().expect("refused");
    assert_eq!(refused, ServiceError::NameTooLong { bytes: 220, limit: 219 });
    assert!(refused.to_string().contains("219"));
    let at_limit = format!("/{}", "b".repeat(219));
    assert!(transport.serve(&spec(&at_limit)).is_ok(), "219 bytes fit");
}

#[test]
fn the_tools_list_each_stream_with_its_server_and_clients() {
    let name = service_name();
    let server_node = unique_name("lister_server");
    let server_side = Iox2Transport::with_name(&server_node).expect("node");
    let _server = server_side.serve(&spec(&name)).expect("a server");
    let client_side = new_transport();
    let _client = client_side.service_client(&spec(&name), None).expect("a client");
    let tool = new_transport();
    let streams = tool.list_services().expect("the list");
    let stream = streams.iter().find(|s| s.name == name).expect("the stream is listed");
    assert_eq!(stream.type_name, "std_srvs/SetBool");
    assert_eq!(stream.md5, V1);
    assert_eq!(stream.policy, Some(ServicePolicy::default_service()));
    assert_eq!(stream.servers, 1);
    assert_eq!(stream.clients, 1);
    let node = stream.server_node.as_ref().expect("the server's node");
    assert_eq!(node.name, server_node);
    assert!(node.is_alive);
}

#[test]
fn a_stream_opened_with_another_policy_is_refused() {
    let name = service_name();
    let transport = new_transport();
    let _server = transport.serve(&spec(&name)).expect("a server");
    let other = ServicePolicy { name: "camera".to_owned(), ..ServicePolicy::default_service() };
    let refused = transport.service_client(&spec(&name).with_policy(other), None).err();
    assert_eq!(refused, Some(ServiceError::QosMismatch));
}

#[test]
fn measurements_of_memory_and_descriptors_per_service() {
    let policy = ServicePolicy::default_service();
    let chunks = policy.admitted_clients() * 2 * policy.max_outstanding * 3;
    let formula = chunks * policy.max_response_bytes;
    let transport = new_transport();
    let fds_before = descriptors();
    let shm_before = shm_bytes();
    let name = service_name();
    let server = transport.serve(&spec(&name)).expect("a server");
    let fds_server = descriptors();
    let shm_server = shm_bytes();
    let client_side = transport_with_name("measure_client");
    let client = client_side.service_client(&spec(&name), None).expect("a client");
    let fds_client = descriptors();
    let shm_client = shm_bytes();
    let report = |label: &str, before: Option<u64>, after: Option<u64>| match (before, after) {
        (Some(b), Some(a)) => format!("{label} {} KiB", a.saturating_sub(b) / 1024),
        _ => format!("{label} not measurable here (no /dev/shm)"),
    };
    println!(
        "service_transport: memory under the default policy: formula {} chunks x {} B = {} KiB per \
         server; measured {}, {}",
        chunks,
        policy.max_response_bytes,
        formula / 1024,
        report("server (with the stream and its events)", shm_before, shm_server),
        report("client", shm_server, shm_client),
    );
    println!(
        "service_transport: descriptors: server {} (stream, lock, response notifier), client {}",
        fds_server.saturating_sub(fds_before),
        fds_client.saturating_sub(fds_server)
    );
    if let (Some(before), Some(after)) = (shm_before, shm_server) {
        let measured = after.saturating_sub(before);
        assert!(
            measured >= u64::try_from(formula).unwrap(),
            "the server holds at least its formula"
        );
    }
    drop((server, client));
}

fn transport_with_name(prefix: &str) -> Iox2Transport {
    Iox2Transport::with_name(&unique_name(prefix)).expect("a node")
}

/// Open descriptors of this process.
fn descriptors() -> usize {
    std::fs::read_dir("/dev/fd").map_or(0, Iterator::count)
}

/// Bytes of the files in `/dev/shm`, where iceoryx2 keeps its segments on Linux.
fn shm_bytes() -> Option<u64> {
    let entries = std::fs::read_dir("/dev/shm").ok()?;
    Some(entries.flatten().filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum())
}

type RawStream = iceoryx2::service::port_factory::request_response::PortFactory<
    iceoryx2::service::ipc_threadsafe::Service,
    [u8],
    RawHeader,
    [u8],
    RawHeader,
>;

/// The frame header in its shared-memory layout, as `hypha_iceoryx2`'s own.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, ZeroCopySend)]
#[type_name("hypha::FrameHeader")]
struct RawHeader {
    magic: u32,
    flags: u32,
    schema_hash: u64,
    publisher_id: u64,
    sequence: u64,
    transport_sent_nanos: u64,
    framework_sent_nanos: u64,
    crc: u32,
    reserved: u32,
}

trait FromRawForTest {
    fn from_raw_for_test(raw: RawHeader) -> Self;
}

impl FromRawForTest for hypha_wire::frame::FrameHeader {
    fn from_raw_for_test(raw: RawHeader) -> Self {
        Self {
            magic: raw.magic,
            flags: raw.flags,
            metadata: hypha_wire::frame::FrameMetadata {
                schema_hash: raw.schema_hash,
                publisher_id: raw.publisher_id,
                sequence: raw.sequence,
                transport_sent_nanos: raw.transport_sent_nanos,
                framework_sent_nanos: raw.framework_sent_nanos,
            },
            crc: raw.crc,
        }
    }
}

impl RawHeader {
    fn from_frame(frame: hypha_wire::frame::FrameHeader) -> Self {
        let m = frame.metadata;
        Self {
            magic: frame.magic,
            flags: frame.flags,
            schema_hash: m.schema_hash,
            publisher_id: m.publisher_id,
            sequence: m.sequence,
            transport_sent_nanos: m.transport_sent_nanos,
            framework_sent_nanos: m.framework_sent_nanos,
            crc: frame.crc,
            reserved: 0,
        }
    }
}

fn raw_node() -> iceoryx2::node::Node<iceoryx2::service::ipc_threadsafe::Service> {
    let config = hypha_iceoryx2::domain::config().expect("the domain");
    NodeBuilder::new()
        .config(config)
        .name(&NodeName::new(&unique_name("raw")).expect("a name"))
        .signal_handling_mode(SignalHandlingMode::Disabled)
        .create::<iceoryx2::service::ipc_threadsafe::Service>()
        .expect("a raw node")
}

/// The stream of `name` (version 1) as an existing service, opened without any setting.
fn raw_stream(
    node: &iceoryx2::node::Node<iceoryx2::service::ipc_threadsafe::Service>,
    name: &str,
) -> RawStream {
    let hash = hypha_wire::schema_hash_from_md5(V1).expect("an MD5");
    let stream = format!("hypha/srv/{}@{hash:016x}", name.trim_start_matches('/'));
    node.service_builder(&ServiceName::new(&stream).expect("a stream name"))
        .request_response::<[u8], [u8]>()
        .request_user_header::<RawHeader>()
        .response_user_header::<RawHeader>()
        .open()
        .expect("open the stream")
}
