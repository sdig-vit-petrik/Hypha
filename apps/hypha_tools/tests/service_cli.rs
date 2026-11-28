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
