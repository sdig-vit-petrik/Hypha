//! Recorder schema-retention integration tests.

#![allow(clippy::expect_used)]

use std::time::Duration;

use hypha_node::sdk::{BusNode, SampleIdentity};
use hypha_testkit::unique_name;
use hypha_tools::record_session::RecorderSession;
use hypha_tools::recorder::Recorder;

#[test]
fn unresolved_schema_never_creates_a_non_self_describing_record() {
    let node_name = unique_name("recorder_schema_test");
    let topic = format!("/{node_name}/unknown");
    let output = std::env::temp_dir().join(format!("{node_name}.mcap"));
    let bus = BusNode::new(&node_name).expect("test node must start");
    let mut session = RecorderSession::new(&bus, Recorder::new(&output), false);
    let sample = SampleIdentity {
        topic: &topic,
        transport_sent_nanos: 1,
        framework_sent_nanos: 1,
        schema_hash: 0,
    };
    let bytes = vec![1, 2, 3];

    session.on_sample(&sample, &bytes);
    std::thread::sleep(Duration::from_millis(1_100));
    session.on_sample(&sample, &bytes);

    assert_eq!(session.recorder().counters().len(), 0);
    drop(session.finish());
    std::fs::remove_file(output).expect("test recording must be removable");
}
