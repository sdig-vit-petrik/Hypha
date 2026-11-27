//! Spike for #132: the server process of `tests/service_spike.rs`.
//!
//! `service_server_helper <node name> <service> serve|hold` creates the one server of
//! `<service>`, prints "ready", and then sleeps in its request event until a client
//! wakes it: no polling. `serve` answers each request at once (a `std_srvs/SetBool`
//! response that echoes the request's `data`); `hold` keeps each request unanswered
//! and prints "holding <sequence>", so the test can kill the process mid-call.

#![allow(clippy::expect_used, clippy::panic, reason = "a test helper process")]

#[path = "service_frame.rs"]
mod service_frame;

use std::io::Write as _;
use std::time::Duration;

use service_frame::{
    MAX_PAYLOAD, node, open_event, open_service, request_code, response_code, seal, verify,
};

/// The serialized `std_srvs/SetBool` response: `bool success`, `string message`.
fn set_bool_response(success: bool) -> Vec<u8> {
    let message = b"ok";
    let mut bytes = vec![u8::from(success)];
    bytes.extend_from_slice(&u32::try_from(message.len()).expect("short").to_le_bytes());
    bytes.extend_from_slice(message);
    bytes
}

fn main() {
    let mut args = std::env::args().skip(1);
    let node_name = args.next().expect("a node name");
    let service = args.next().expect("a service name");
    let hold = match args.next().as_deref() {
        Some("serve") => false,
        Some("hold") => true,
        other => panic!("mode serve or hold, not {other:?}"),
    };

    let node = node(&node_name);
    let factory = open_service(&node, &service);
    let server = factory
        .server_builder()
        .initial_max_slice_len(MAX_PAYLOAD)
        .create()
        .expect("the first server of the service");
    let requests =
        open_event(&node, &service, "request").listener_builder().create().expect("listener");
    let responses =
        open_event(&node, &service, "response").notifier_builder().create().expect("notifier");
    let server_id: u64 = 0x5e5e;
    let mut sequence: u64 = 0;
    let mut held = Vec::new();

    println!("ready");
    let _ = std::io::stdout().flush();
    loop {
        // Sleep until a client notifies; the timeout only bounds a lost wake-up.
        requests.timed_wait(|_| {}, Duration::from_secs(10)).expect("wait for a request");
        while let Some(request) = server.receive().expect("receive a request") {
            let frame =
                match verify(request_code(&service), request.user_header(), request.payload()) {
                    Ok(frame) => frame,
                    Err(cause) => {
                        eprintln!("refused a request frame: {cause:?}");
                        continue;
                    }
                };
            let (client, client_sequence) = (frame.publisher_id(), frame.sequence());
            if hold {
                println!("holding {client_sequence}");
                let _ = std::io::stdout().flush();
                held.push(request);
                continue;
            }
            let data = frame.payload().first().copied().unwrap_or(0) != 0;
            let payload = set_bool_response(data);
            sequence = sequence.wrapping_add(1);
            let mut response = request.loan_slice_uninit(payload.len()).expect("loan a response");
            *response.user_header_mut() = seal(
                response_code(&service, client, client_sequence),
                server_id,
                sequence,
                &payload,
            );
            response.write_from_slice(&payload).send().expect("send the response");
            drop(request);
            responses.notify().expect("wake the client");
        }
    }
}
