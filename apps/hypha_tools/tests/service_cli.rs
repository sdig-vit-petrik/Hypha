//! `hypha service list|info|call` against a `std_srvs/SetBool` server of this test process
//! (#132, stage 1b): the request is encoded from YAML or JSON with the definitions the server
//! registered, the response is printed, and each outcome has its exit code.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::process::{Command, Output};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use hypha_node::HyphaCallbackResult;
use hypha_node::safe::{
    Answer, HyphaNode, HyphaRequest, RequestView, ServeHandler, ServiceDecl, request_answer,
    request_defer, service_serve, waitset_run,
};
use hypha_testkit::unique_name;

const SET_BOOL: &str = "std_srvs/SetBool";
const SET_BOOL_MD5: &str = "09fb03525b03e7ea1fd3992bafd87e16";
const REQUEST: &str = "bool data";
const RESPONSE: &str = "bool success\nstring message";

/// What the server does with a request: `data` true is answered, false refused.
struct SetBool {
    /// Requests deferred and never answered, so their calls time out.
    held: Option<Arc<Mutex<Vec<HyphaRequest>>>>,
}

impl ServeHandler for SetBool {
    fn serve(&mut self, request: &mut HyphaRequest, view: &RequestView<'_>) -> HyphaCallbackResult {
        if let Some(held) = &self.held {
            let deferred = request_defer(request).expect("defer");
            held.lock().unwrap_or_else(PoisonError::into_inner).push(*deferred);
            return HyphaCallbackResult::continue_run();
        }
        if view.payload == [1] {
            let message = b"enabled";
            let mut response = vec![1_u8];
            response.extend_from_slice(&u32::try_from(message.len()).expect("len").to_le_bytes());
            response.extend_from_slice(message);
            request_answer(request, Answer::Reply(&response)).expect("reply");
        } else {
            request_answer(request, Answer::Refuse("not armed")).expect("refuse");
        }
        HyphaCallbackResult::continue_run()
    }
}

fn decl(name: &str) -> ServiceDecl<'_> {
    ServiceDecl {
        name,
        literal: true,
        type_name: SET_BOOL,
        md5: SET_BOOL_MD5,
        request_definition: REQUEST,
        response_definition: RESPONSE,
        policy: None,
        timeout: None,
    }
}

fn hypha(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_hypha")).arg("service").args(args).output().expect("hypha")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Serve `name` (and `held`, whose requests are never answered) on `node` while `body` runs.
fn serving(node: &Arc<HyphaNode>, name: &str, held: &str, body: impl FnOnce()) {
    let _server =
        service_serve(node, &decl(name), Box::new(SetBool { held: None })).expect("server");
    let deferred = Arc::new(Mutex::new(Vec::new()));
    let _slow = service_serve(node, &decl(held), Box::new(SetBool { held: Some(deferred) }))
        .expect("a server that never answers");
    std::thread::scope(|scope| {
        let (sender, shared) = std::sync::mpsc::channel();
        let run = scope.spawn(move || {
            let lease = hypha_node::safe::node_default_waitset(node).expect("waitset");
            let waitset = lease.waitset().expect("lent");
            sender.send(Arc::clone(&waitset.shared)).expect("send");
            waitset_run(waitset, Some(Duration::from_secs(60)))
        });
        let shared = shared.recv().expect("the run's WaitSet");
        body();
        shared.request_stop().expect("stop");
        let _ = run.join();
    });
}

#[test]
fn service_call_prints_the_response_and_exits_with_a_code_per_outcome() {
    let node =
        hypha_node::safe::node_create(&unique_name("set_bool_node"), None, None).expect("node");
    let name = format!("/{}", unique_name("set_enabled"));
    let held = format!("/{}", unique_name("never_answers"));
    serving(&node, &name, &held, || {
        // Answered: YAML as rosservice takes it, and JSON.
        for request in ["data: true", r#"{"data": true}"#, "{data: True}"] {
            let answered = hypha(&["call", &name, request]);
            assert_eq!(answered.status.code(), Some(0), "{request}: {}", text(&answered.stderr));
            assert_eq!(text(&answered.stdout), "success: true\nmessage: \"enabled\"\n");
        }

        let refused = hypha(&["call", &name, "data: false"]);
        assert_eq!(refused.status.code(), Some(2), "{}", text(&refused.stderr));
        assert!(text(&refused.stderr).contains("refused: not armed"), "{}", text(&refused.stderr));

        let missing = format!("/{}", unique_name("nobody_serves"));
        let no_server = hypha(&["call", &missing, "data: true", "--timeout", "0.2"]);
        assert_eq!(no_server.status.code(), Some(3), "{}", text(&no_server.stderr));
        assert!(text(&no_server.stderr).contains("no server"), "{}", text(&no_server.stderr));

        let timeout = hypha(&["call", &held, "data: true", "--timeout", "0.3"]);
        assert_eq!(timeout.status.code(), Some(6), "{}", text(&timeout.stderr));
        assert!(
            text(&timeout.stderr).contains("timeout after 300 ms"),
            "{}",
            text(&timeout.stderr)
        );

        let unknown_field = hypha(&["call", &name, "{nope: 1}"]);
        assert_eq!(unknown_field.status.code(), Some(1));
        assert!(
            text(&unknown_field.stderr).contains("has no field 'nope'; its fields: data"),
            "{}",
            text(&unknown_field.stderr)
        );

        let template = hypha(&["call", &name]);
        assert_eq!(template.status.code(), Some(0), "{}", text(&template.stderr));
        assert_eq!(text(&template.stdout), "data: false\n");

        let list = hypha(&["list"]);
        assert_eq!(list.status.code(), Some(0));
        let listed = text(&list.stdout);
        let Some(line) = listed.lines().find(|line| line.starts_with(&name)) else {
            panic!("{name} is listed:\n{listed}");
        };
        assert!(line.contains(SET_BOOL) && line.contains(&SET_BOOL_MD5[..8]), "{line}");
        assert!(line.contains("server set_bool_node"), "{line}");

        let info = hypha(&["info", &name]);
        assert_eq!(info.status.code(), Some(0));
        let info = text(&info.stdout);
        for expected in [
            "  type:     std_srvs/SetBool",
            "  policy:   defaultService:",
            &format!("  call log: {name}/calls (hypha_msgs/ServiceCall)"),
            "  request (std_srvs/SetBoolRequest):\n    bool data",
            "  response (std_srvs/SetBoolResponse):\n    bool success\n    string message",
        ] {
            assert!(info.contains(expected), "'{expected}' in:\n{info}");
        }
    });
}

/// Answers every request with the same encoded response.
struct Fixed(Vec<u8>);

impl ServeHandler for Fixed {
    fn serve(
        &mut self,
        request: &mut HyphaRequest,
        _view: &RequestView<'_>,
    ) -> HyphaCallbackResult {
        request_answer(request, Answer::Reply(&self.0)).expect("reply");
        HyphaCallbackResult::continue_run()
    }
}

/// A service with a half of no fields: the half is an empty definition, not a missing one.
struct Shape {
    type_name: &'static str,
    md5: &'static str,
    request: &'static str,
    response: &'static str,
    /// The encoded response the server answers with.
    reply: Vec<u8>,
}

/// Serve `shape` as `name` on a fresh node while `body` runs.
fn serving_shape(shape: &Shape, name: &str, body: impl FnOnce()) {
    let node = hypha_node::safe::node_create(&unique_name("shape_node"), None, None).expect("node");
    let decl = ServiceDecl {
        name,
        literal: true,
        type_name: shape.type_name,
        md5: shape.md5,
        request_definition: shape.request,
        response_definition: shape.response,
        policy: None,
        timeout: None,
    };
    let _server =
        service_serve(&node, &decl, Box::new(Fixed(shape.reply.clone()))).expect("server");
    let node = &node;
    std::thread::scope(|scope| {
        let (sender, shared) = std::sync::mpsc::channel();
        let run = scope.spawn(move || {
            let lease = hypha_node::safe::node_default_waitset(node).expect("waitset");
            let waitset = lease.waitset().expect("lent");
            sender.send(Arc::clone(&waitset.shared)).expect("send");
            waitset_run(waitset, Some(Duration::from_secs(60)))
        });
        let shared = shared.recv().expect("the run's WaitSet");
        body();
        shared.request_stop().expect("stop");
        let _ = run.join();
    });
}

#[test]
fn a_service_with_an_empty_request_is_called_without_a_body() {
    // std_srvs/Trigger: the request has no fields, the response two.
    let mut reply = vec![1_u8];
    reply.extend_from_slice(&2_u32.to_le_bytes());
    reply.extend_from_slice(b"ok");
    let trigger = Shape {
        type_name: "std_srvs/Trigger",
        md5: "937c9679a518e3a18d831e57125ea522",
        request: "",
        response: "bool success\nstring message",
        reply,
    };
    let name = format!("/{}", unique_name("trigger"));
    serving_shape(&trigger, &name, || {
        let called = hypha(&["call", &name]);
        assert_eq!(called.status.code(), Some(0), "{}", text(&called.stderr));
        assert_eq!(text(&called.stdout), "success: true\nmessage: \"ok\"\n");
        let braces = hypha(&["call", &name, "{}"]);
        assert_eq!(braces.status.code(), Some(0), "{}", text(&braces.stderr));
        let field = hypha(&["call", &name, "{nope: 1}"]);
        assert_eq!(field.status.code(), Some(1));
        assert!(text(&field.stderr).contains("has no field 'nope'"), "{}", text(&field.stderr));
        let info = hypha(&["info", &name]);
        let info = text(&info.stdout);
        assert!(info.contains("  request (std_srvs/TriggerRequest):\n  response"), "{info}");
    });
}

#[test]
fn a_service_with_an_empty_response_answers_with_no_fields() {
    let setter = Shape {
        type_name: "hypha_tests/SetLimit",
        md5: "8b94c1b53db61fb6aed406028ad6332a",
        request: "uint32 limit",
        response: "",
        reply: Vec::new(),
    };
    let name = format!("/{}", unique_name("set_limit"));
    serving_shape(&setter, &name, || {
        let called = hypha(&["call", &name, "limit: 7"]);
        assert_eq!(called.status.code(), Some(0), "{}", text(&called.stderr));
        assert_eq!(text(&called.stdout), "{}\n");
        let template = hypha(&["call", &name]);
        assert_eq!(text(&template.stdout), "limit: 0\n");
    });
}

#[test]
fn a_service_with_two_empty_halves_is_called_and_answers() {
    // std_srvs/Empty: its MD5 is that of the empty text.
    let empty = Shape {
        type_name: "std_srvs/Empty",
        md5: "d41d8cd98f00b204e9800998ecf8427e",
        request: "",
        response: "",
        reply: Vec::new(),
    };
    let name = format!("/{}", unique_name("empty"));
    serving_shape(&empty, &name, || {
        let called = hypha(&["call", &name]);
        assert_eq!(called.status.code(), Some(0), "{}", text(&called.stderr));
        assert_eq!(text(&called.stdout), "{}\n");
    });
}

#[test]
fn a_server_that_gave_no_definitions_is_named_as_such() {
    // Not the empty service: its MD5 says a half has fields, and none were given.
    let silent = Shape {
        type_name: "std_srvs/SetBool",
        md5: SET_BOOL_MD5,
        request: "",
        response: "",
        reply: Vec::new(),
    };
    let name = format!("/{}", unique_name("silent"));
    serving_shape(&silent, &name, || {
        let called = hypha(&["call", &name, "data: true"]);
        assert_eq!(called.status.code(), Some(1));
        assert!(
            text(&called.stderr).contains("registered no definitions"),
            "{}",
            text(&called.stderr)
        );
    });
}
