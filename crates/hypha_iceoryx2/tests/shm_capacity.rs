//! A bounded publisher whose segment cannot fit into free `/dev/shm` is created with a warning.
#![cfg(target_os = "linux")]
#![allow(clippy::expect_used, clippy::arithmetic_side_effects)]

use hypha_core::port::{PUBLISHER_LOANED_SAMPLES, PayloadCapacity, TopicSpec};
use hypha_iceoryx2::{Iox2Transport, qos};
use hypha_testkit::unique_name;

#[test]
fn a_segment_larger_than_the_free_shared_memory_is_warned_about() {
    let dir = std::env::temp_dir().join(unique_name("hypha_shm_capacity"));
    std::fs::create_dir_all(&dir).expect("log dir");
    let table: toml::Table =
        "console = false\n[sink.main]\npath = \"shm.log\"\n".parse().expect("log table");
    hypha_log::commit(hypha_log::prepare(&table, &dir.join("node.toml")).expect("prepare"))
        .expect("commit");

    let free = shm_free_bytes();
    // The pool of the topic's policy (undeclared: `default`), sized to twice the free space.
    let policy = qos::default_policy();
    let subscribers = qos::subscriber_capacity("/shm_cap", &policy);
    let samples = qos::pool_samples(&policy, subscribers, PUBLISHER_LOANED_SAMPLES);
    let payload = usize::try_from(free * 2 / u64::try_from(samples).expect("samples fit u64"))
        .expect("payload fits usize");
    let transport = Iox2Transport::with_name(&unique_name("shm_cap")).expect("transport");
    let big = format!("/{}", unique_name("shm_cap_big"));
    let small = format!("/{}", unique_name("shm_cap_small"));
    let _big = transport
        .publish(
            &TopicSpec::new(&big)
                .with_type("hypha_test/Data")
                .with_payload_capacity(PayloadCapacity::Bounded(payload)),
        )
        .expect("an oversized segment is still created");
    let _small = transport
        .publish(
            &TopicSpec::new(&small)
                .with_type("hypha_test/Data")
                .with_payload_capacity(PayloadCapacity::Bounded(256)),
        )
        .expect("small publisher");

    let text = std::fs::read_to_string(dir.join("shm.log")).expect("log file");
    assert!(
        text.contains(&format!("topic '{big}' may need")) && text.contains("bytes free"),
        "log file holds: {text}"
    );
    assert!(!text.contains(&format!("topic '{small}' may need")), "log file holds: {text}");
    std::fs::remove_dir_all(&dir).expect("remove log dir");
}

/// Free bytes of `/dev/shm`, read the way `df` does.
fn shm_free_bytes() -> u64 {
    let output = std::process::Command::new("df")
        .args(["--output=avail", "-B1", "/dev/shm"])
        .output()
        .expect("df");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .nth(1)
        .and_then(|line| line.trim().parse().ok())
        .expect("df prints the free bytes")
}
