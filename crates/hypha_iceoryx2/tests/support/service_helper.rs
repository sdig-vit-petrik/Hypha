//! Test helper process for `tests/service_*.rs`: one end of a service, driven by its
//! arguments, reporting on stdout.
//!
//! `hypha_iceoryx2_service_helper <node> <service> <v1|v2> <mode>`:
//!
//! - `serve`: the server; answers each `std_srvs/SetBool` request with its `data` echoed.
//! - `hold`: the server; keeps every request, printing `holding <sequence>`.
//! - `refuse`, `fail`: the server; refuses with `nope`, or fails with `boom`.
//! - `big`: the server; answers above the response bound and prints the reply's error.
//! - `race <file>`: waits until `<file>` exists, then creates the server and prints `won`
//!   or `refused <message>`; then waits to be killed.
//! - `call-and-exit`: a client; sends one call, prints `sent`, waits for a line on
//!   stdin, and exits normally.
//! - `client-hold`: a client; sends until its limit, prints `sent <n>`, and waits to be killed.
//! - `descriptors <limit>`: sets the open-file limit to `<limit>`, then creates servers of
//!   `<service>_<i>` until one is refused, and prints `refused after <count> <message>`.
//!
//! `HELPER_TIGHT=1` gives the service a policy of one client with one call in flight.
//!
//! A server prints `ready` once it exists, then sleeps in its request event: no polling.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    reason = "a test helper process"
)]

use std::io::{BufRead as _, Write as _};
use std::time::Duration;

use hypha_core::port::ServiceSpec;
use hypha_iceoryx2::Iox2Transport;

const V1: &str = "09fb03525b03e7ea1fd3992bafd87e16";
const V2: &str = "29fb03525b03e7ea1fd3992bafd87e16";

fn say(line: &str) {
    println!("{line}");
    let _ = std::io::stdout().flush();
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [node, service, version, mode, rest @ ..] = args.as_slice() else {
        panic!("usage: <node> <service> <v1|v2> <mode> [argument]");
    };
    let md5 = if version == "v2" { V2 } else { V1 };
    let mut spec = ServiceSpec::new(service, "std_srvs/SetBool", md5);
    if std::env::var_os("HELPER_TIGHT").is_some() {
        spec = spec.with_policy(hypha_core::port::ServicePolicy {
            name: "tight".to_owned(),
            max_clients: 1,
            max_outstanding: 1,
            ..hypha_core::port::ServicePolicy::default_service()
        });
    }
    if mode == "descriptors" {
        // Both limits, so the domain's first use cannot raise the soft one again.
        let limit: u64 = rest.first().expect("a limit").parse().expect("a number");
        let both = rustix::process::Rlimit { current: Some(limit), maximum: Some(limit) };
        rustix::process::setrlimit(rustix::process::Resource::Nofile, both).expect("setrlimit");
        let transport = Iox2Transport::with_name(node).expect("the node");
        let mut servers = Vec::new();
        loop {
            let name = format!("{service}_{}", servers.len());
            match transport.serve(&ServiceSpec::new(&name, "std_srvs/SetBool", md5)) {
                Ok(server) => servers.push(server),
                Err(cause) => {
                    say(&format!("refused after {} {cause}", servers.len()));
                    return;
                }
            }
        }
    }
    let transport = Iox2Transport::with_name(node).expect("the node");

    if mode == "client-hold" {
        // A client with every call in flight; it never exits by itself.
        let mut client = transport.service_client(&spec, None, None).expect("the client");
        let mut sent = 0;
        while client.send(&[1], 0).is_ok() {
            sent += 1;
        }
        say(&format!("sent {sent}"));
        loop {
            std::thread::sleep(Duration::from_secs(3600));
        }
    }

    if mode == "call-and-exit" {
        let mut client = transport.service_client(&spec, None, None).expect("the client");
        client.send(&[1], 0).expect("the call");
        say("sent");
        let mut line = String::new();
        let _ = std::io::stdin().lock().read_line(&mut line);
        return;
    }
    if mode == "race" {
        let go = rest.first().expect("the start file");
        while !std::path::Path::new(go).exists() {
            std::thread::sleep(Duration::from_millis(1));
        }
        let server = transport.serve(&spec);
        match &server {
            Ok(_) => say("won"),
            Err(cause) => say(&format!("refused {cause}")),
        }
        loop {
            std::thread::sleep(Duration::from_secs(3600));
        }
    }

    let mut server = transport.serve(&spec).expect("the first server of the name");
    let wake = server.open_listener().expect("the request listener");
    say("ready");
    let mut held = Vec::new();
    loop {
        wake.timed_wait(|_| {}, Duration::from_secs(10)).expect("wait for a request");
        while let Some(request) = server.receive().expect("receive") {
            let data = server.payload(request.handle).and_then(|p| p.first().copied()).unwrap_or(0);
            match mode.as_str() {
                "serve" => {
                    let mut response = vec![data, 2, 0, 0, 0];
                    response.extend_from_slice(b"ok");
                    server.reply(request.handle, &response, 7).expect("reply");
                }
                "hold" => {
                    say(&format!("holding {}", request.sequence));
                    held.push(request.handle);
                }
                "refuse" => server.refuse(request.handle, "nope", 7).expect("refuse"),
                "fail" => server.fail(request.handle, "boom", 7).expect("fail"),
                "big" => {
                    let response = vec![0_u8; server.policy().max_response_bytes + 1];
                    let result = server.reply(request.handle, &response, 7);
                    say(&format!("reply {result:?}"));
                }
                other => panic!("mode {other}"),
            }
        }
    }
}
