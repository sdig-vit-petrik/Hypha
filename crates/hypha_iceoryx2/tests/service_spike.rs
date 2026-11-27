//! Spike for #132 (native services): iceoryx2 0.10.1 request-response carrying hypha
//! frames between two processes.
//!
//! It checks the assumptions the design in `docs/design/native-services.md` rests on:
//! the frame header fits as the user header of both directions and binds a response to
//! its request; the server sleeps in an event until a request comes; a second server
//! on the name is refused; a request that no server took is seen at send; and what a
//! client observes when the server process is killed while it holds the request.
//! It also measures a round trip of a small request (`--no-capture` prints it).

#![allow(clippy::expect_used, clippy::panic, reason = "a spike test")]

#[path = "support/service_frame.rs"]
mod service_frame;

use std::io::{BufRead as _, BufReader};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use iceoryx2::node::{NodeState, NodeView as _};
use iceoryx2::prelude::{CallbackProgression, PortFactory as _};
use iceoryx2::service::port_factory::server::ServerCreateError;
use service_frame::{
    Ipc, MAX_PAYLOAD, WireHeader, node, open_event, open_service, request_code, response_code,
    seal, verify,
};

type Service = iceoryx2::service::port_factory::request_response::PortFactory<
    Ipc,
    [u8],
    WireHeader,
    [u8],
    WireHeader,
>;

const CLIENT_ID: u64 = 0xc11e;

/// A server process; killed when dropped.
struct ServerProcess {
    child: Child,
    stdout: BufReader<ChildStdout>,
}

impl ServerProcess {
    fn start(node_name: &str, service: &str, mode: &str) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_hypha_iceoryx2_service_server_helper"))
            .args([node_name, service, mode])
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn the server helper");
        let stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
        let mut process = Self { child, stdout };
        assert_eq!(process.line(), "ready");
        process
    }

    fn line(&mut self) -> String {
        let mut line = String::new();
        self.stdout.read_line(&mut line).expect("read a line of the server helper");
        line.trim().to_owned()
    }

    fn kill(&mut self) {
        self.child.kill().expect("SIGKILL the server");
        self.child.wait().expect("reap the server");
    }
}

impl Drop for ServerProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn unique(prefix: &str) -> String {
    hypha_testkit::unique_name(prefix)
}

/// The serialized `std_srvs/SetBool` request: `bool data`.
fn set_bool_request(data: bool) -> [u8; 1] {
    [u8::from(data)]
}

fn load_average() -> String {
    let output = Command::new("sysctl").args(["-n", "vm.loadavg"]).output();
    match output {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).trim().to_owned(),
        _ => std::fs::read_to_string("/proc/loadavg").unwrap_or_default().trim().to_owned(),
    }
}

/// Send one framed request with `sequence`, and wake the server.
fn send(
    client: &iceoryx2::port::client::Client<Ipc, [u8], WireHeader, [u8], WireHeader>,
    wake: &iceoryx2::port::notifier::Notifier<Ipc>,
    service: &str,
    sequence: u64,
    data: bool,
) -> iceoryx2::pending_response::PendingResponse<Ipc, [u8], WireHeader, [u8], WireHeader> {
    let payload = set_bool_request(data);
    let mut request = client.loan_slice_uninit(payload.len()).expect("loan a request");
    *request.user_header_mut() = seal(request_code(service), CLIENT_ID, sequence, &payload);
    let pending = request.write_from_slice(&payload).send().expect("send the request");
    wake.notify().expect("wake the server");
    pending
}

fn the_server_node(service: &Service) -> iceoryx2::identifiers::UniqueNodeId {
    let mut id = None;
    service.dynamic_config().list_servers(|details| {
        id = Some(details.node_id);
        CallbackProgression::Stop
    });
    id.expect("a server is registered")
}

/// The state of the node `id` among the nodes of `service`: "alive", "dead", or another.
fn node_state(service: &Service, id: iceoryx2::identifiers::UniqueNodeId) -> &'static str {
    let mut state = "absent";
    service
        .nodes(|view| {
            let (this, name) = match &view {
                NodeState::Alive(v) => (*v.id(), "alive"),
                NodeState::Dead(v) => (*v.id(), "dead"),
                NodeState::Inaccessible(v) => (*v, "inaccessible"),
                NodeState::Undefined(v) => (*v, "undefined"),
            };
            if this == id {
                state = name;
                return CallbackProgression::Stop;
            }
            CallbackProgression::Continue
        })
        .expect("list the nodes of the service");
    state
}

#[test]
fn a_request_that_no_server_takes_is_seen_at_send() {
    let service = unique("svc_none");
    let client_node = node(&unique("client"));
    let factory = open_service(&client_node, &service);
    let client =
        factory.client_builder().initial_max_slice_len(MAX_PAYLOAD).create().expect("client");
    let wake = open_event(&client_node, &service, "request")
        .notifier_builder()
        .create()
        .expect("notifier");

    let pending = send(&client, &wake, &service, 1, true);

    assert_eq!(pending.number_of_server_connections(), 0, "no server received it");
    assert!(!pending.is_connected(), "nothing is pending");
}

#[test]
fn a_framed_round_trip_between_two_processes_and_one_server_per_name() {
    let service = unique("svc_rtt");
    let mut server = ServerProcess::start(&unique("server"), &service, "serve");
    let client_node = node(&unique("client"));
    let factory = open_service(&client_node, &service);

    // A second server on the name is refused at creation.
    let second = factory.server_builder().create();
    assert!(
        matches!(second, Err(ServerCreateError::ExceedsMaxSupportedServers)),
        "a second server must be refused, got {:?}",
        second.err()
    );

    let client =
        factory.client_builder().initial_max_slice_len(MAX_PAYLOAD).create().expect("client");
    let wake = open_event(&client_node, &service, "request")
        .notifier_builder()
        .create()
        .expect("notifier");
    let replies = open_event(&client_node, &service, "response")
        .listener_builder()
        .create()
        .expect("listener");

    let rounds: u64 = 2000;
    let mut samples = Vec::new();
    for sequence in 1..=rounds {
        let data = sequence % 2 == 0;
        let start = Instant::now();
        let pending = send(&client, &wake, &service, sequence, data);
        assert_eq!(pending.number_of_server_connections(), 1);
        let response = loop {
            if let Some(response) = pending.receive().expect("receive") {
                break response;
            }
            replies.timed_wait(|_| {}, Duration::from_secs(2)).expect("wait for the response");
            assert!(start.elapsed() < Duration::from_secs(5), "no response to request {sequence}");
        };
        samples.push(start.elapsed());

        // The response verifies against its own request, and against nothing else.
        let header = response.user_header();
        let frame =
            verify(response_code(&service, CLIENT_ID, sequence), header, response.payload())
                .expect("the response verifies against the request it answers");
        assert_eq!(frame.payload().first().copied(), Some(u8::from(data)), "SetBool echo");
        assert!(
            verify(
                response_code(&service, CLIENT_ID, sequence.wrapping_add(1)),
                header,
                response.payload()
            )
            .is_err(),
            "a response does not pass as the answer to another request"
        );
        assert!(
            verify(request_code(&service), header, response.payload()).is_err(),
            "a response does not pass as a request"
        );
    }

    samples.sort_unstable();
    let at = |per_mille: usize| {
        let index = samples.len().saturating_mul(per_mille) / 1000;
        samples.get(index.min(samples.len().saturating_sub(1))).copied().unwrap_or_default()
    };
    println!(
        "service_spike: {rounds} round trips (SetBool, 1-byte request, 7-byte response): \
         median {:?}, p90 {:?}, p99 {:?}, max {:?}; load average {}",
        at(500),
        at(900),
        at(990),
        samples.last().copied().unwrap_or_default(),
        load_average()
    );
    server.kill();
}

#[test]
fn a_server_killed_while_it_holds_a_request() {
    let service = unique("svc_kill");
    let mut server = ServerProcess::start(&unique("server"), &service, "hold");
    let client_node = node(&unique("client"));
    let factory = open_service(&client_node, &service);
    let client =
        factory.client_builder().initial_max_slice_len(MAX_PAYLOAD).create().expect("client");
    let wake = open_event(&client_node, &service, "request")
        .notifier_builder()
        .create()
        .expect("notifier");
    let server_node = the_server_node(&factory);

    let pending = send(&client, &wake, &service, 1, true);
    assert_eq!(pending.number_of_server_connections(), 1);
    assert_eq!(server.line(), "holding 1");
    assert!(pending.is_connected(), "the server holds the request");
    assert_eq!(node_state(&factory, server_node), "alive");

    let killed = Instant::now();
    server.kill();
    // How long until the service's node list reports the server's node dead.
    let mut probes: u32 = 0;
    let mut slowest_probe = Duration::ZERO;
    while killed.elapsed() < Duration::from_secs(5) {
        let probe = Instant::now();
        let state = node_state(&factory, server_node);
        slowest_probe = slowest_probe.max(probe.elapsed());
        probes = probes.saturating_add(1);
        if state == "dead" {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let detected = killed.elapsed();
    assert_eq!(node_state(&factory, server_node), "dead", "the killed server's node is dead");

    // What the transport alone tells the client.
    let connected = pending.is_connected();
    let has_response = pending.has_response();
    let servers = factory.dynamic_config().number_of_servers();
    let replacement = node(&unique("server_two"));
    let replacement_factory = open_service(&replacement, &service);
    let refused = replacement_factory.server_builder().create().err();
    println!(
        "service_spike: after SIGKILL: node dead after {detected:?} ({probes} probes, slowest {slowest_probe:?}); \
         pending.is_connected() = {connected}, has_response() = {has_response}, servers = {servers}, \
         a new server under another node name: {refused:?}"
    );
    assert!(connected, "iceoryx2 alone still reports the dead server connected");
    assert!(!has_response);
    assert_eq!(servers, 1, "the dead server keeps its slot");
    assert!(matches!(refused, Some(ServerCreateError::ExceedsMaxSupportedServers)));

    // Reclaiming the dead node frees the slot and closes the pending call.
    let config = hypha_iceoryx2::domain::config().expect("domain");
    let mut reclaimed = false;
    iceoryx2::node::Node::<Ipc>::list(config, |state| {
        if let NodeState::Dead(view) = state
            && *view.id() == server_node
        {
            reclaimed = view.blocking_remove_stale_resources(Duration::from_secs(2)).is_ok();
            return CallbackProgression::Stop;
        }
        CallbackProgression::Continue
    })
    .expect("list the nodes");
    assert!(reclaimed, "the dead server node is reclaimed");
    let connected_after = pending.is_connected();
    let servers_after = factory.dynamic_config().number_of_servers();
    let new_server = replacement_factory.server_builder().create();
    println!(
        "service_spike: after the reclaim: pending.is_connected() = {connected_after}, servers = \
         {servers_after}, a new server: {}",
        if new_server.is_ok() { "created" } else { "refused" }
    );
    assert!(!connected_after, "the reclaim disconnects the pending call");
    assert_eq!(servers_after, 0);
    assert!(new_server.is_ok(), "a new server takes the freed slot");
    drop(new_server);
    drop(replacement_factory);
}

/// Can a held request move to another thread (a deferred reply from a worker)?
#[test]
fn an_active_request_and_a_pending_response_are_send() {
    fn assert_send<T: Send>() {}
    assert_send::<iceoryx2::active_request::ActiveRequest<Ipc, [u8], WireHeader, [u8], WireHeader>>(
    );
    assert_send::<
        iceoryx2::pending_response::PendingResponse<Ipc, [u8], WireHeader, [u8], WireHeader>,
    >();
}
