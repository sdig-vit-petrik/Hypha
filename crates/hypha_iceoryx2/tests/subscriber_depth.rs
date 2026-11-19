//! Subscriber depth must fit the service buffer limit; unset, it is the limit.

use hypha_core::port::{PortError, TopicSpec};
use hypha_iceoryx2::Iox2Transport;

#[test]
#[allow(clippy::expect_used)]
fn subscriber_depth_above_the_service_limit_is_refused() {
    let transport = Iox2Transport::new()
        .expect("an iceoryx2 transport: /dev/shm and /tmp/iceoryx2 must be writable");
    let dir = std::env::temp_dir().join(format!("hypha_depth_log_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("log dir");
    let table: toml::Table =
        "console = false\n[sink.main]\npath = \"depth.log\"\n".parse().expect("log table");
    hypha_log::commit(hypha_log::prepare(&table, &dir.join("node.toml")).expect("prepare"))
        .expect("commit");

    let _publisher = transport
        .publish(&TopicSpec::new("/subscriber_depth_limit").with_type("std_msgs/String"))
        .expect("publisher creates the service");
    let spec = TopicSpec::new("/subscriber_depth_limit").with_type("std_msgs/String").keeping(33);
    assert!(matches!(transport.subscribe(&spec), Err(PortError::ResourceLimit)));
    let text = std::fs::read_to_string(dir.join("depth.log")).expect("log file");
    assert!(
        text.contains("depth 33 exceeds the 32 the service's QoS policy allows"),
        "log file holds: {text}"
    );
    assert!(!text.contains("clean-shm"), "log file holds: {text}");
    std::fs::remove_dir_all(&dir).expect("remove log dir");
}

#[test]
#[allow(clippy::expect_used)]
fn an_unset_subscriber_depth_is_the_service_maximum() {
    let transport = Iox2Transport::new()
        .expect("an iceoryx2 transport: /dev/shm and /tmp/iceoryx2 must be writable");
    let _publisher = transport
        .publish(&TopicSpec::new("/subscriber_depth_unset").with_type("std_msgs/String"))
        .expect("publisher creates the service");
    let spec = TopicSpec::new("/subscriber_depth_unset").with_type("std_msgs/String");
    let subscriber = transport.subscribe(&spec).expect("a subscriber with no depth");
    assert_eq!(subscriber.buffer_size(), hypha_iceoryx2::qos::DEFAULT_MAX_DEPTH);
}
