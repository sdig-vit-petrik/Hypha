//! Cancel safety of the hypha to ROS 1 message handoff.

#![allow(clippy::expect_used)]

use std::sync::mpsc;
use std::time::Duration;

use hypha_ros_bridge::BridgeMsg;
use hypha_ros_bridge::handoff::HyphaToRosHandoff;

fn message(topic: &str) -> BridgeMsg {
    BridgeMsg {
        route_id: Some(0),
        topic: topic.into(),
        identity: std::sync::Arc::new(hypha_node::sdk::ResolvedSchema {
            data_type: "std_msgs/Float64".to_owned(),
            md5: "fdb28210bfa9d7c91146260178d9a584".to_owned(),
            definition: "float64 data".to_owned(),
        }),
        stamp_nanos: 42,
        bytes: vec![1, 2, 3, 4],
    }
}

/// The receive arm loses to the timeout arm, and a message published while the
/// abandoned receive is still in flight must survive to the next receive.
#[tokio::test(flavor = "multi_thread")]
async fn a_receive_that_loses_the_select_race_keeps_the_message_queued() {
    let (sender, receiver) = mpsc::channel::<BridgeMsg>();
    let mut handoff = HyphaToRosHandoff::new(receiver);

    // The timeout arm is shorter than the handoff's own wait, so it wins and
    // the receive future is dropped exactly as `select!` drops a losing arm.
    let timed_out = tokio::select! {
        result = handoff.receive(Duration::from_millis(200)) => {
            let _ = result.expect("receive");
            false
        },
        () = tokio::time::sleep(Duration::from_millis(20)) => true,
    };
    assert!(timed_out, "the timeout arm must win for this test to mean anything");

    sender.send(message("/vehicle/speed")).expect("send");

    // Give any abandoned receive time to finish its own wait.
    tokio::time::sleep(Duration::from_millis(400)).await;

    let delivered = handoff
        .receive(Duration::from_millis(200))
        .await
        .expect("second receive")
        .map(|found| found.topic);
    assert_eq!(
        delivered.as_deref(),
        Some("/vehicle/speed"),
        "a message taken by an abandoned receive must not be discarded"
    );
}

/// A closed sending end is reported, never read as an empty queue.
#[tokio::test(flavor = "multi_thread")]
async fn a_closed_sender_is_reported_as_a_closed_channel() {
    let (sender, receiver) = mpsc::channel::<BridgeMsg>();
    let mut handoff = HyphaToRosHandoff::new(receiver);
    drop(sender);

    let outcome = handoff.receive(Duration::from_millis(200)).await;
    assert!(outcome.is_err(), "a closed handoff must be an error, not an empty receive");
}

/// An empty queue reports nothing, and does so without an error.
#[tokio::test(flavor = "multi_thread")]
async fn an_idle_handoff_reports_nothing_without_failing() {
    let (sender, receiver) = mpsc::channel::<BridgeMsg>();
    let mut handoff = HyphaToRosHandoff::new(receiver);

    let outcome = handoff.receive(Duration::from_millis(50)).await.expect("idle receive");
    assert!(outcome.is_none(), "an idle handoff reports nothing");
    drop(sender);
}
