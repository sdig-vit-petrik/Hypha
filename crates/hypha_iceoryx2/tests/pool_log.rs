//! A bounded publisher says at start what its pool reserves, so a pool that is far too large
//! is read in the log and not found in `/dev/shm` (issue 241).
#![allow(clippy::expect_used, clippy::arithmetic_side_effects)]

use hypha_core::port::{PayloadCapacity, QosPolicy, QosRequest, TopicSpec};
use hypha_iceoryx2::Iox2Transport;
use hypha_testkit::unique_name;

#[test]
fn a_large_publisher_logs_its_pool_and_a_small_one_does_not_at_info() {
    let dir = std::env::temp_dir().join(unique_name("hypha_pool_log"));
    std::fs::create_dir_all(&dir).expect("log dir");
    let table: toml::Table =
        "console = false\nlevel = \"info\"\n[sink.main]\npath = \"pool.log\"\n"
            .parse()
            .expect("log table");
    hypha_log::commit(hypha_log::prepare(&table, &dir.join("node.toml")).expect("prepare"))
        .expect("commit");

    let transport = Iox2Transport::with_name(&unique_name("pool_log")).expect("transport");
    let camera = format!("/{}", unique_name("pool_camera"));
    let tiny = format!("/{}", unique_name("pool_tiny"));
    // 1 publisher, 2 subscribers (and the 3 observer slots), depth 2, 2 loans: 23 samples.
    let policy = QosRequest::Declared(QosPolicy::new("pool_camera", 1, 2, 2));
    let _camera = transport
        .publish(
            &TopicSpec::new(&camera)
                .with_type("hypha_test/Data")
                .with_qos(policy)
                .with_payload_capacity(PayloadCapacity::Bounded(50_000)),
        )
        .expect("camera publisher");
    let _tiny = transport
        .publish(
            &TopicSpec::new(&tiny)
                .with_type("hypha_test/Data")
                .with_payload_capacity(PayloadCapacity::Bounded(64)),
        )
        .expect("tiny publisher");

    let text = std::fs::read_to_string(dir.join("pool.log")).expect("log file");
    let expected = format!(
        "topic '{camera}' publisher pool: 23 samples of 50104 bytes, {} bytes of shared memory",
        23 * 50_104
    );
    assert!(text.contains(&expected), "log file holds: {text}");
    assert!(!text.contains(&format!("topic '{tiny}' publisher pool")), "log file holds: {text}");
    std::fs::remove_dir_all(&dir).expect("remove log dir");
}
