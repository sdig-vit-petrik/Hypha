//! A bounded deployment (`HYPHA_BOUNDED_MEMORY=1`): the shared memory of a domain is
//! the sum of the segments its publishers declare. A publisher without a payload
//! limit is refused, and so is one whose segment cannot fit into the free space of
//! `/dev/shm`; a publisher with a limit that fits is created as before.
#![cfg(target_os = "linux")]
#![allow(clippy::expect_used, clippy::arithmetic_side_effects)]

use std::path::PathBuf;

use hypha_core::port::{PUBLISHER_LOANED_SAMPLES, PayloadCapacity, PortError, TopicSpec};
use hypha_iceoryx2::{Iox2Transport, qos};
use hypha_testkit::unique_name;

/// Turns the bounded deployment on, before the first use of the domain reads it. Every
/// test of this file sets the same value, so they may share a process.
fn enter_bounded_deployment() {
    hypha_testkit::set_env(hypha_iceoryx2::domain::BOUNDED_MEMORY_VARIABLE, "1");
}

/// Sends the log of this process to a file of its own and returns the file.
fn log_file(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(unique_name(label));
    std::fs::create_dir_all(&dir).expect("log dir");
    let table: toml::Table =
        "console = false\n[sink.main]\npath = \"bounded.log\"\n".parse().expect("log table");
    hypha_log::commit(hypha_log::prepare(&table, &dir.join("node.toml")).expect("prepare"))
        .expect("commit");
    dir.join("bounded.log")
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

#[test]
fn a_growable_publisher_is_refused_and_a_bounded_one_is_created() {
    enter_bounded_deployment();
    let log = log_file("hypha_bounded_growable");
    let transport = Iox2Transport::with_name(&unique_name("bounded_growable")).expect("transport");
    let growable = format!("/{}", unique_name("bounded_growable_topic"));
    let bounded = format!("/{}", unique_name("bounded_fixed_topic"));

    let refused = transport.publish(
        &TopicSpec::new(&growable)
            .with_type("hypha_test/Data")
            .with_payload_capacity(PayloadCapacity::Growable),
    );
    assert!(matches!(refused, Err(PortError::ResourceLimit)), "a growable publisher is refused");
    // The refusal comes before the service: a bounded publisher of the same topic,
    // whatever its policy, still creates it.
    let _same_topic = transport
        .publish(
            &TopicSpec::new(&growable)
                .with_type("hypha_test/Data")
                .with_payload_capacity(PayloadCapacity::Bounded(256)),
        )
        .expect("the refused publisher left no service behind");
    let _bounded = transport
        .publish(
            &TopicSpec::new(&bounded)
                .with_type("hypha_test/Data")
                .with_payload_capacity(PayloadCapacity::Bounded(256)),
        )
        .expect("a publisher with a payload limit that fits is created");

    let text = std::fs::read_to_string(&log).expect("log file");
    assert!(
        text.contains(&format!(
            "topic '{growable}': a growable publisher is refused in a bounded deployment; set \
             maxPayloadDynamic or a policy limit"
        )),
        "log file holds: {text}"
    );
    assert!(!text.contains(&bounded), "log file holds: {text}");
}

#[test]
fn a_segment_larger_than_the_free_shared_memory_is_refused() {
    enter_bounded_deployment();
    let log = log_file("hypha_bounded_shortfall");
    // The pool of the topic's policy (undeclared: `default`), sized to twice the free space.
    let policy = qos::default_policy();
    let subscribers = qos::subscriber_capacity("/bounded_shortfall", &policy);
    let samples = qos::pool_samples(&policy, subscribers, PUBLISHER_LOANED_SAMPLES);
    let payload =
        usize::try_from(shm_free_bytes() * 2 / u64::try_from(samples).expect("samples fit u64"))
            .expect("payload fits usize");
    let transport = Iox2Transport::with_name(&unique_name("bounded_shortfall")).expect("transport");
    let topic = format!("/{}", unique_name("bounded_shortfall_topic"));

    let refused = transport.publish(
        &TopicSpec::new(&topic)
            .with_type("hypha_test/Data")
            .with_payload_capacity(PayloadCapacity::Bounded(payload)),
    );

    assert!(matches!(refused, Err(PortError::ResourceLimit)), "the oversized pool is refused");
    let text = std::fs::read_to_string(&log).expect("log file");
    assert!(
        text.contains(&format!("topic '{topic}' needs"))
            && text.contains("a bounded deployment refuses a publisher that cannot fit"),
        "log file holds: {text}"
    );
}
