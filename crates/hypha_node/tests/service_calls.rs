//! Services in a plain node: each call ends in exactly one outcome (FSR-42), a blocking
//! call is refused on a dispatch thread (FSR-43), and a call made before its server
//! exists is held until one appears.
#![allow(clippy::expect_used, reason = "a failed setup is a test failure")]

use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use hypha_node::HyphaCallbackResult;
use hypha_node::safe::{
    Answer, CallOutcome, CallResult, CallStatus, Error, HyphaNode, HyphaRequest, RequestView,
    ServeHandler, ServiceDecl, client_call, service_client_create, service_serve, waitset_run,
};
use hypha_testkit::unique_name;

const SET_BOOL: &str = "std_srvs/SetBool";
const SET_BOOL_MD5: &str = "09fb03525b03e7ea1fd3992bafd87e16";

fn decl(name: &str) -> ServiceDecl<'_> {
    ServiceDecl {
        name,
        literal: true,
        type_name: SET_BOOL,
        md5: SET_BOOL_MD5,
        policy: None,
        timeout: Some(Duration::from_millis(300)),
    }
}

fn node(prefix: &str) -> Arc<HyphaNode> {
    hypha_node::safe::node_create(&unique_name(prefix), None, None).expect("node")
}

/// A handler that answers each request with `answer(payload)`.
struct Handler<F>(F);

impl<F: FnMut(&mut HyphaRequest, &[u8]) + Send> ServeHandler for Handler<F> {
    fn serve(&mut self, request: &mut HyphaRequest, view: &RequestView<'_>) -> HyphaCallbackResult {
        (self.0)(request, view.payload);
        HyphaCallbackResult::continue_run()
    }
}

/// Run the node's `WaitSet` for `limit` on another thread while `body` runs here.
fn while_running<R>(node: &Arc<HyphaNode>, limit: Duration, body: impl FnOnce() -> R) -> R {
    std::thread::scope(|scope| {
        let run = scope.spawn(|| {
            let lease = hypha_node::safe::node_default_waitset(node).expect("waitset");
            waitset_run(lease.waitset().expect("lent"), Some(limit)).expect("run")
        });
        let result = body();
        let _ = run.join();
        result
    })
}

/// A blocking call of `request`, and its result.
fn call_and_wait(client: &hypha_node::safe::HyphaClient, request: &[u8]) -> CallResult {
    let (sender, receiver) = mpsc::channel();
    let done = move |result: &CallResult| {
        let _ = sender.send(result.clone());
        HyphaCallbackResult::continue_run()
    };
    assert_eq!(client_call(client, request, Box::new(done)), Ok(CallStatus::Sent));
    receiver.try_recv().expect("a blocking call delivers its outcome before it returns")
}

#[test]
fn a_blocking_call_is_answered_refused_or_left_unanswered() {
    let server_node = node("srv_node");
    let name = format!("/{}", unique_name("set_bool"));
    let handler = Handler(|request: &mut HyphaRequest, payload: &[u8]| match payload {
        [1] => request_reply(request, b"ok"),
        [2] => drop(hypha_node::safe::request_answer(request, Answer::Refuse("not armed"))),
        _ => {}
    });
    let server = service_serve(&server_node, &decl(&name), Box::new(handler)).expect("server");
    let client_node = node("cli_node");
    let client = service_client_create(&client_node, &decl(&name), true).expect("client");
    let results = while_running(&server_node, Duration::from_secs(2), || {
        [call_and_wait(&client, &[1]), call_and_wait(&client, &[2]), call_and_wait(&client, &[3])]
    });
    assert_eq!(results[0].outcome, CallOutcome::Answered(b"ok".to_vec()));
    assert_eq!(results[1].outcome, CallOutcome::Refused("not armed".to_owned()));
    assert!(
        matches!(&results[2].outcome, CallOutcome::Refused(why) if why.contains("did not answer")),
        "{:?}",
        results[2].outcome
    );
    hypha_node::safe::service_server_destroy(server);
}

fn request_reply(request: &mut HyphaRequest, response: &[u8]) {
    hypha_node::safe::request_answer(request, Answer::Reply(response)).expect("reply");
}

#[test]
fn a_call_without_a_server_ends_no_server_and_names_its_timeout() {
    let client_node = node("lonely");
    let name = format!("/{}", unique_name("nobody"));
    let client = service_client_create(&client_node, &decl(&name), true).expect("client");
    let result = call_and_wait(&client, &[1]);
    assert!(
        matches!(&result.outcome, CallOutcome::NoServer(why) if why.contains("within 300 ms (set in code)")),
        "{:?}",
        result.outcome
    );
}

#[test]
fn a_deferred_request_held_past_the_timeout_ends_timeout_then_its_answer_is_late() {
    let server_node = node("slow_srv");
    let name = format!("/{}", unique_name("slow"));
    let held = Arc::new(Mutex::new(Vec::new()));
    let keep = Arc::clone(&held);
    let handler = Handler(move |request: &mut HyphaRequest, _: &[u8]| {
        let deferred = hypha_node::safe::request_defer(request).expect("defer");
        keep.lock().unwrap_or_else(PoisonError::into_inner).push(deferred);
    });
    let server = service_serve(&server_node, &decl(&name), Box::new(handler)).expect("server");
    let client_node = node("impatient");
    let client = service_client_create(&client_node, &decl(&name), true).expect("client");
    let result =
        while_running(&server_node, Duration::from_secs(1), || call_and_wait(&client, &[1]));
    assert_eq!(result.outcome, CallOutcome::Timeout("after 300 ms (set in code)".to_owned()));
    // Answered from this thread, after the run: the client already gave up.
    let mut deferred = held.lock().unwrap_or_else(PoisonError::into_inner).pop().expect("deferred");
    hypha_node::safe::request_answer(&mut deferred, Answer::Reply(b"late"))
        .expect("a late reply is sent");
    assert_eq!(
        hypha_node::safe::request_answer(&mut deferred, Answer::Reply(b"again")),
        Err(Error::InvalidState),
        "a request is answered once"
    );
    hypha_node::safe::service_server_destroy(server);
}

/// A node whose parameter file holds `toml`.
fn node_with(prefix: &str, toml: &str) -> Arc<HyphaNode> {
    let directory = std::env::temp_dir().join(unique_name(prefix));
    std::fs::create_dir_all(&directory).expect("directory");
    let path = directory.join("params.toml");
    std::fs::write(&path, toml).expect("parameter file");
    let path = path.to_str().expect("a text path").to_owned();
    hypha_node::safe::node_create(&unique_name(prefix), Some(&path), None).expect("node")
}

#[test]
fn a_port_reads_its_table_and_a_port_without_one_is_refused_with_the_stanza() {
    let name = format!("/{}", unique_name("tabled"));
    let node = node_with(
        "tables",
        &format!("[ports.clients.trigger]\nservice = \"{name}\"\ntimeout_ms = 120\n"),
    );
    let port = ServiceDecl { name: "trigger", literal: false, timeout: None, ..decl("") };
    let client = service_client_create(&node, &port, true).expect("client of the table");
    let result = call_and_wait(&client, &[1]);
    assert!(
        matches!(&result.outcome, CallOutcome::NoServer(why)
            if why.contains(&name) && why.contains("within 120 ms (timeout_ms of [ports.clients.trigger])")),
        "{:?}",
        result.outcome
    );

    let missing = ServiceDecl { name: "set_exposure", literal: false, ..decl("") };
    let handler = Handler(|_: &mut HyphaRequest, _: &[u8]| {});
    let refused = service_serve(&node, &missing, Box::new(handler)).map(|_| ());
    assert_eq!(refused, Err(Error::InvalidArgument));
    assert_eq!(
        hypha_node::safe::last_error(),
        "server port 'set_exposure' has no table; add to the parameter file:\n\
         [ports.servers.set_exposure]\nservice = \"~/set_exposure\""
    );
}
