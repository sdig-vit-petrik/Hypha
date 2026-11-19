//! A host iceoryx2.toml with `enable-safe-overflow = false` has no effect on hypha.

#![allow(clippy::expect_used)]

mod support;
use support::sealed_send;

use std::time::{Duration, Instant};

use hypha_core::port::{QosPolicy, QosRequest, TopicSpec};
use hypha_iceoryx2::Iox2Transport;
use hypha_testkit::unique_name;
use iceoryx2::prelude::{FilePath, SemanticString as _};

fn set_iceoryx2_global_config_safe_overflow_false() {
    let config_dir = std::env::temp_dir().join(unique_name("overflow_config"));
    let config_path = config_dir.join("iceoryx2.toml");
    std::fs::create_dir_all(&config_dir).expect("create temp config directory");
    std::fs::write(&config_path, "[defaults.publish_subscribe]\nenable-safe-overflow = false\n")
        .expect("write temp iceoryx2 config file");
    let config_path_str = config_path.to_str().expect("temp config path is valid UTF-8");
    let file_path =
        FilePath::new(config_path_str.as_bytes()).expect("temp config path is a valid FilePath");
    iceoryx2::config::Config::setup_global_config_from_file(&file_path)
        .expect("install the test's global iceoryx2 config");
    let _ = std::fs::remove_dir_all(&config_dir);
}

#[test]
fn safe_overflow_overrides_host_config_and_drops_without_blocking() {
    set_iceoryx2_global_config_safe_overflow_false();

    let domain_config = hypha_iceoryx2::domain::config().expect("domain config is valid");
    assert!(
        domain_config.defaults.publish_subscribe.enable_safe_overflow,
        "domain Config must pin enable_safe_overflow = true"
    );
    assert_eq!(
        domain_config.defaults.publish_subscribe.backpressure_strategy,
        iceoryx2::prelude::BackpressureStrategy::DiscardData,
        "domain Config must pin backpressure_strategy = DiscardData"
    );

    let transport = Iox2Transport::new()
        .expect("an iceoryx2 transport: /dev/shm and /tmp/iceoryx2 must be writable");

    let topic = format!("/{}", unique_name("safe_overflow"));
    let sub_spec = TopicSpec::new(&topic)
        .with_type("hypha_test/Data")
        .keeping(1)
        .with_qos(QosRequest::Declared(QosPolicy::new("default", 8, 16, 32)));
    let mut _subscriber = transport.subscribe(&sub_spec).expect("subscribe");

    let pub_spec = TopicSpec::new(&topic).with_type("hypha_test/Data");
    let publisher = transport.publish(&pub_spec).expect("publish");

    // More samples than the subscriber buffer holds must not block.
    // With safe_overflow = true and DiscardData, send() completes immediately.
    let started = Instant::now();
    for i in 0..10u8 {
        let mut loan = publisher.loan(16).expect("loan");
        loan.as_mut_bytes().fill(i);
        loan.send_frame(sealed_send::metadata(), 0).expect("send");
    }
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_millis(500),
        "sending into full queue must drop without blocking, took {elapsed:?}"
    );
}
