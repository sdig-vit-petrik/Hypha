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
    let spec = ServiceSpec::new(service, "std_srvs/SetBool", md5);
    let transport = Iox2Transport::with_name(node).expect("the node");

    if mode == "call-and-exit" {
        let mut client = transport.service_client(&spec, None).expect("the client");
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
