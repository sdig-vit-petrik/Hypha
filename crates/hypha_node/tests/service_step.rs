//! The service rules of a cycle step, decided once here on the fed ends of a step test
//! (`HyphaStepServices`, what `hypha::testing::StepHarness` drives): which requests a step
//! at T reads and in which order, what becomes of a request left alone, refused, deferred
//! or held by a failed step, and which outcomes a step reads, carries or drops
//! (`docs/design/native-services.md` §2-§3.2). The C++ harness keeps its wiring cases.
#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    reason = "a failed setup is a test failure"
)]

use std::time::Duration;

use hypha_node::safe::{
    Answer, CallOutcome, CallStatus, EmissionView, FedAnswer, HyphaStepServices, RequestState,
    ServiceDecl, emission_call, emission_next_result, emission_requests, emission_unread_results,
    request_answer, request_defer, step_services_create,
};

const MS: i64 = 1_000_000;

fn decl() -> ServiceDecl<'static> {
    ServiceDecl {
        name: "/enable",
        literal: true,
        type_name: "std_srvs/SetBool",
        md5: "09fb03525b03e7ea1fd3992bafd87e16",
        request_definition: "",
        response_definition: "",
        policy: None,
        timeout: Some(Duration::from_millis(300)),
    }
}

fn server() -> HyphaStepServices {
    let mut services = step_services_create(None, "step_rules");
    assert_eq!(services.bind_server(&decl()).expect("server"), 0);
    services
}

fn client() -> HyphaStepServices {
    let mut services = step_services_create(None, "step_rules");
    assert_eq!(services.bind_client(&decl()).expect("client"), 0);
    services
}

/// A step that answers every request it reads with its client's name and returns `ok`.
fn reply_all(view: &mut EmissionView<'_, '_>, ok: bool) -> bool {
    for request in emission_requests(view, 0).expect("requests") {
        let reply = request.client_node.clone().into_bytes();
        request_answer(&mut request.request, Answer::Reply(&reply)).expect("answer");
    }
    ok
}

fn state(services: &HyphaStepServices, id: u64) -> RequestState {
    services.request_state(0, id).expect("state")
}

fn replied(services: &HyphaStepServices, id: u64) -> Option<(Vec<u8>, i64)> {
    match state(services, id) {
        RequestState::Answered(FedAnswer::Replied(bytes), at) => Some((bytes, at)),
        _ => None,
    }
}

#[test]
fn a_step_reads_the_requests_sent_before_its_time_and_answers_after_it() {
    let mut services = server();
    let early = services.deliver(0, b"", "mission", Some(5 * MS)).expect("deliver");
    let now = services.deliver(0, b"", "mission", Some(10 * MS)).expect("deliver");
    services.step(10 * MS, |view| reply_all(view, true));
    assert_eq!(replied(&services, early), Some((b"mission".to_vec(), 10 * MS)));
    assert_eq!(state(&services, now), RequestState::Queued, "sent at the step's time: later");
    services.step(20 * MS, |view| reply_all(view, true));
    assert_eq!(replied(&services, now).map(|(_, at)| at), Some(20 * MS));
}

#[test]
fn requests_are_read_in_order_of_send_time_then_client() {
    let mut services = server();
    for (client, at) in [("c", 2), ("b", 1), ("a", 2)] {
        services.deliver(0, b"", client, Some(at * MS)).expect("deliver");
    }
    let mut order = Vec::new();
    services.step(10 * MS, |view| {
        order = emission_requests(view, 0)
            .expect("requests")
            .iter()
            .map(|r| r.client_node.clone())
            .collect();
        reply_all(view, true)
    });
    assert_eq!(order, ["b", "a", "c"]);
}

#[test]
fn a_step_that_does_not_read_leaves_its_requests_queued() {
    let mut services = server();
    let id = services.deliver(0, b"", "mission", None).expect("deliver");
    services.step(10 * MS, |_| true);
    assert_eq!(state(&services, id), RequestState::Queued);
    services.step(20 * MS, |view| reply_all(view, true));
    assert!(replied(&services, id).is_some());
}

#[test]
fn a_refusal_and_a_request_left_alone_are_refused_after_the_step() {
    let mut services = server();
    let refused = services.deliver(0, b"", "a", Some(MS)).expect("deliver");
    let left = services.deliver(0, b"", "b", Some(2 * MS)).expect("deliver");
    services.step(10 * MS, |view| {
        let requests = emission_requests(view, 0).expect("requests");
        request_answer(&mut requests[0].request, Answer::Refuse("not armed")).expect("refuse");
        true
    });
    let reason = |id| match state(&services, id) {
        RequestState::Answered(FedAnswer::Refused(reason), _) => reason,
        other => panic!("not refused: {other:?}"),
    };
    assert_eq!(reason(refused), "not armed");
    assert!(reason(left).contains("did not answer"), "{}", reason(left));
}

#[test]
fn a_deferred_request_is_answered_in_a_later_step() {
    let mut services = server();
    let id = services.deliver(0, b"", "mission", None).expect("deliver");
    let mut held = None;
    services.step(10 * MS, |view| {
        let requests = emission_requests(view, 0).expect("requests");
        held = Some(request_defer(&mut requests[0].request).expect("defer"));
        true
    });
    assert_eq!(state(&services, id), RequestState::Deferred);
    services.step(20 * MS, |_| {
        let mut held = held.take().expect("held");
        request_answer(&mut held, Answer::Reply(b"later")).expect("answer");
        true
    });
    assert_eq!(replied(&services, id), Some((b"later".to_vec(), 20 * MS)));
}

/// A step at 10 ms that defers the one request it reads; the handle it keeps.
fn deferred(services: &mut HyphaStepServices) -> hypha_node::safe::HyphaRequest {
    let mut held = None;
    services.step(10 * MS, |view| {
        let requests = emission_requests(view, 0).expect("requests");
        held = Some(request_defer(&mut requests[0].request).expect("defer"));
        true
    });
    *held.expect("deferred")
}

#[test]
fn a_deferred_answer_given_in_a_failed_step_is_failed_and_a_dropped_one_refused() {
    let mut services = server();
    let id = services.deliver(0, b"", "mission", None).expect("deliver");
    let mut held = Some(deferred(&mut services));
    services.step(20 * MS, |_| {
        let mut held = held.take().expect("held");
        request_answer(&mut held, Answer::Reply(b"late")).expect("answer");
        false
    });
    assert!(matches!(state(&services, id), RequestState::Answered(FedAnswer::Failed(_), _)));

    let id = services.deliver(0, b"", "mission", None).expect("deliver");
    drop(deferred(&mut services));
    match state(&services, id) {
        RequestState::Answered(FedAnswer::Refused(reason), _) => {
            assert!(reason.contains("destroyed unanswered"), "{reason}");
        }
        other => panic!("not refused: {other:?}"),
    }
}

#[test]
fn a_failed_step_answers_what_it_read_as_failed() {
    let mut services = server();
    let id = services.deliver(0, b"", "mission", None).expect("deliver");
    services.step(10 * MS, |view| reply_all(view, false));
    assert_eq!(
        state(&services, id),
        RequestState::Answered(FedAnswer::Failed("server step failed".to_owned()), 10 * MS)
    );
}

/// The outcomes client 0 reads in a step at `now`.
fn read_outcomes(services: &mut HyphaStepServices, now: i64, ok: bool) -> Vec<CallOutcome> {
    let mut outcomes = Vec::new();
    services.step(now, |view| {
        while let Some(result) = emission_next_result(view, 0).expect("result") {
            outcomes.push(result.outcome);
        }
        ok
    });
    outcomes
}

/// A step at `now` that stages one call of client 0.
fn call(services: &mut HyphaStepServices, now: i64, ok: bool) -> CallStatus {
    let mut status = None;
    services.step(now, |view| {
        status = Some(emission_call(view, 0, b"req").expect("call"));
        ok
    });
    status.expect("staged")
}

#[test]
fn a_call_leaves_after_its_step_and_every_outcome_reaches_a_later_step_in_order() {
    let mut services = client();
    assert_eq!(call(&mut services, 10 * MS, true), CallStatus::Sent);
    let sent = services.sent(0).expect("sent");
    assert_eq!((sent.len(), sent[0].stamp), (1, 10 * MS));
    services.feed(0, CallOutcome::Answered(b"yes".to_vec()), Some(15 * MS)).expect("feed");
    assert_eq!(read_outcomes(&mut services, 15 * MS, true), [], "stamped at the step: later");
    assert_eq!(
        read_outcomes(&mut services, 20 * MS, true),
        [CallOutcome::Answered(b"yes".to_vec())]
    );
    assert_eq!(
        services.feed(0, CallOutcome::Refused(String::new()), None),
        Err(hypha_node::safe::Error::InvalidState),
        "no call in flight"
    );
}

#[test]
fn one_call_is_in_flight_by_default() {
    let mut services = client();
    let mut statuses = Vec::new();
    services.step(10 * MS, |view| {
        statuses.push(emission_call(view, 0, b"a").expect("call"));
        statuses.push(emission_call(view, 0, b"b").expect("call"));
        true
    });
    assert_eq!(statuses, [CallStatus::Sent, CallStatus::Busy]);
}

#[test]
fn a_call_of_a_failed_step_ends_not_sent_in_the_next_step() {
    let mut services = client();
    call(&mut services, 10 * MS, false);
    assert!(services.sent(0).expect("sent").is_empty());
    let outcomes = read_outcomes(&mut services, 20 * MS, true);
    assert!(matches!(outcomes.as_slice(), [CallOutcome::NotSent(_)]), "{outcomes:?}");
}

#[test]
fn a_failed_step_carries_its_unread_outcomes_and_a_successful_one_drops_and_counts_them() {
    let mut services = client();
    call(&mut services, 10 * MS, true);
    services.feed(0, CallOutcome::Answered(b"1".to_vec()), None).expect("feed");
    services.step(20 * MS, |_| false);
    assert_eq!(read_outcomes(&mut services, 30 * MS, true), [CallOutcome::Answered(b"1".to_vec())]);

    call(&mut services, 40 * MS, true);
    services.feed(0, CallOutcome::Answered(b"2".to_vec()), None).expect("feed");
    services.step(50 * MS, |_| true);
    let mut unread = 0;
    let outcomes = {
        let mut outcomes = Vec::new();
        services.step(60 * MS, |view| {
            unread = emission_unread_results(view, 0).expect("unread");
            outcomes.extend(emission_next_result(view, 0).expect("result").map(|r| r.outcome));
            true
        });
        outcomes
    };
    assert_eq!((unread, outcomes.len()), (1, 0));
}

#[test]
fn a_call_left_open_past_its_timeout_ends_timeout() {
    let mut services = client();
    call(&mut services, 10 * MS, true);
    assert_eq!(read_outcomes(&mut services, 300 * MS, true), [], "within its 300 ms");
    let outcomes = read_outcomes(&mut services, 320 * MS, true);
    assert!(matches!(outcomes.as_slice(), [CallOutcome::Timeout(_)]), "{outcomes:?}");
}
