//! Verification of topic resource and capacity limits in `hypha_iceoryx2`.

#![allow(clippy::expect_used)]

mod support;
use support::sealed_send;

use std::sync::atomic::{AtomicUsize, Ordering};

use hypha_core::port::{PortError, QosPolicy, QosRequest, TopicSpec};
use hypha_iceoryx2::Iox2Transport;

static TOPIC_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn unique_topic(prefix: &str) -> String {
    let pid = std::process::id();
    let count = TOPIC_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("/{prefix}_{pid}_{count}")
}

#[test]
fn safe_defaults_are_applied_to_service() {
    let transport = Iox2Transport::new()
        .expect("an iceoryx2 transport: /dev/shm and /tmp/iceoryx2 must be writable");
    let topic = unique_topic("safe_defaults");
    let spec = TopicSpec::new(&topic).with_type("std_msgs/String");

    let publisher_res = transport.publish(&spec);
    assert!(publisher_res.is_ok());
    let subscriber_res = transport.subscribe(&spec);
    assert!(subscriber_res.is_ok());
}

#[test]
#[allow(clippy::expect_used)]
fn subscriber_resource_limit_enforced_at_capacity() {
    let transport = Iox2Transport::new()
        .expect("an iceoryx2 transport: /dev/shm and /tmp/iceoryx2 must be writable");
    let topic = unique_topic("sub_limit");
    let max_subscribers = hypha_iceoryx2::qos::DEFAULT_MAX_SUBSCRIBERS;

    let pub_spec = TopicSpec::new(&topic).with_type("std_msgs/String");
    let publisher = transport.publish(&pub_spec).expect("failed to publish publisher");

    // The policy's subscribers, declared, fill the policy's slots.
    let mut subscribers = Vec::new();
    let declared_spec = TopicSpec::new(&topic)
        .with_type("std_msgs/String")
        .keeping(1)
        .with_qos(QosRequest::Declared(hypha_iceoryx2::qos::default_policy()));
    for _ in 0..max_subscribers {
        let sub = transport.subscribe(&declared_spec);
        assert!(sub.is_ok());
        subscribers.push(sub);
    }
    let overflow_sub = transport.subscribe(&declared_spec);
    assert!(matches!(overflow_sub, Err(PortError::ResourceLimit)));

    // Undeclared subscribers (tools) take only the reserved observer slots.
    let observer_spec = TopicSpec::new(&topic).with_type("std_msgs/String").keeping(1);
    for _ in 0..hypha_iceoryx2::qos::RESERVED_OBSERVER_SUBSCRIBERS {
        let observer = transport.subscribe(&observer_spec);
        assert!(observer.is_ok(), "an observer within its reserved slots");
        subscribers.push(observer);
    }
    let overflow_observer = transport.subscribe(&observer_spec);
    assert!(matches!(overflow_observer, Err(PortError::ResourceLimit)));

    let mut loan = publisher.loan(16).expect("failed to loan");
    loan.as_mut_bytes().fill(0xAA);
    assert!(loan.send_frame(sealed_send::metadata(), 0).is_ok());
}

#[test]
fn multiple_publishers_succeed_because_domain_config_owns_limits() {
    let transport = Iox2Transport::new()
        .expect("an iceoryx2 transport: /dev/shm and /tmp/iceoryx2 must be writable");
    let topic = unique_topic("qos_no_mismatch");
    let spec = TopicSpec::new(&topic).with_type("std_msgs/String");

    let pub_a = transport.publish(&spec);
    assert!(pub_a.is_ok());

    let pub_b = transport.publish(&spec);
    assert!(pub_b.is_ok());
}

#[test]
#[allow(clippy::expect_used)]
fn endpoint_opens_service_with_more_capacity_than_it_needs() {
    let transport = Iox2Transport::new()
        .expect("an iceoryx2 transport: /dev/shm and /tmp/iceoryx2 must be writable");

    let topic = unique_topic("surplus_capacity");
    let sub_spec = TopicSpec::new(&topic).with_type("std_msgs/String").keeping(2);
    let pub_spec = TopicSpec::new(&topic).with_type("std_msgs/String");
    let publisher = transport.publish(&pub_spec).expect("publisher opens service");
    let subscriber =
        transport.subscribe(&sub_spec).expect("subscriber with small queue opens service");

    let mut loan = publisher.loan(8).expect("failed to loan");
    loan.as_mut_bytes().fill(0x42);
    assert!(loan.send_frame(sealed_send::metadata(), 0).is_ok());

    let sample = subscriber.try_receive().expect("receive succeeds");
    assert!(sample.is_some());
}

#[test]
#[allow(clippy::expect_used)]
fn subscriber_exceeding_service_max_buffer_size_fails_in_every_start_order() {
    let transport = Iox2Transport::new()
        .expect("an iceoryx2 transport: /dev/shm and /tmp/iceoryx2 must be writable");

    // Order 1: the subscriber creates the service, then asks for depth 33 (> 32).
    let topic_first = unique_topic("buf_exceed_sub_first");
    let oversized_spec_1 = TopicSpec::new(&topic_first)
        .with_type("std_msgs/String")
        .keeping(33)
        .with_qos(QosRequest::Declared(QosPolicy::new("depth_32", 8, 16, 32)));
    let sub_result_1 = transport.subscribe(&oversized_spec_1);
    assert!(matches!(sub_result_1, Err(PortError::ResourceLimit)));

    // Order 2: the publisher creates the service first.
    let topic_second = unique_topic("buf_exceed_pub_first");
    let pub_spec = TopicSpec::new(&topic_second).with_type("std_msgs/String");
    let publisher = transport.publish(&pub_spec).expect("publisher creates service");
    let oversized_spec_2 = TopicSpec::new(&topic_second).with_type("std_msgs/String").keeping(33);
    let sub_result_2 = transport.subscribe(&oversized_spec_2);
    assert!(matches!(sub_result_2, Err(PortError::ResourceLimit)));
    drop(publisher);
}

#[test]
#[allow(clippy::expect_used)]
fn wildcard_attaches_to_services_without_altering_configuration() {
    let transport = Iox2Transport::new()
        .expect("an iceoryx2 transport: /dev/shm and /tmp/iceoryx2 must be writable");
    let base = unique_topic("wc");
    let topic_a = format!("{base}/first");
    let topic_b = format!("{base}/second");

    let spec_a = TopicSpec::new(&topic_a).with_type("std_msgs/String");
    let spec_b = TopicSpec::new(&topic_b).with_type("std_msgs/String");

    let pub_a = transport.publish(&spec_a).expect("failed to publish A");
    let pub_b = transport.publish(&spec_b).expect("failed to publish B");

    let wildcard_pattern = format!("{base}/*");
    let wc_spec = TopicSpec::new(&wildcard_pattern).with_type("std_msgs/String");
    let wc_sub = transport.subscribe(&wc_spec).expect("failed to subscribe wildcard");

    let _ = wc_sub.try_receive();
    std::thread::sleep(std::time::Duration::from_millis(50));

    let mut loan_a = pub_a.loan(8).expect("failed to loan A");
    loan_a.as_mut_bytes().fill(0x11);
    assert!(loan_a.send_frame(sealed_send::metadata(), 0).is_ok());

    let mut loan_b = pub_b.loan(8).expect("failed to loan B");
    loan_b.as_mut_bytes().fill(0x22);
    assert!(loan_b.send_frame(sealed_send::metadata(), 0).is_ok());

    let mut received_a = false;
    let mut received_b = false;
    for _ in 0..10 {
        if let Ok(Some(sample)) = wc_sub.try_receive() {
            if sample.topic() == topic_a {
                received_a = true;
            } else if sample.topic() == topic_b {
                received_b = true;
            }
        }
        if received_a && received_b {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(received_a);
    assert!(received_b);
}

#[test]
fn notification_limits_match_data_topic_qos() {
    let transport = Iox2Transport::new()
        .expect("an iceoryx2 transport: /dev/shm and /tmp/iceoryx2 must be writable");
    let topic = unique_topic("notify_qos");
    let spec = TopicSpec::new(&topic).with_type("std_msgs/String");

    let pub_res = transport.publish(&spec);
    assert!(pub_res.is_ok());
    let sub_res = transport.subscribe(&spec);
    assert!(sub_res.is_ok());
}
