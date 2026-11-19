//! A topic's service policy: declared, undeclared, stamped, and refused.
#![allow(clippy::expect_used)]

mod support;
use support::sealed_send;

use hypha_core::port::{PayloadCapacity, PortError, QosPolicy, QosRequest, TopicSpec};
use hypha_iceoryx2::{Iox2Transport, qos};
use hypha_testkit::unique_name as unique;

const STRING: &str = "std_msgs/String";
const CLOCK: &str = "rosgraph_msgs/Clock";
const CLOCK_MD5: &str = "a9c97c1d230cfc112e270351a944ee47";

fn topic(label: &str) -> String {
    format!("/{}", unique(label))
}

fn declare(name: &str, publishers: usize, subscribers: usize, depth: usize) -> QosRequest {
    QosRequest::Declared(QosPolicy::new(name, publishers, subscribers, depth))
}

fn transport(label: &str) -> Iox2Transport {
    Iox2Transport::with_name(&unique(label)).expect("transport")
}

#[test]
fn the_default_policy_is_eight_publishers_sixteen_subscribers_depth_thirty_two() {
    assert_eq!(qos::default_policy(), QosPolicy::new("default", 8, 16, 32));
    assert_eq!(qos::framework_policy(), QosPolicy::new("framework", 32, 32, 32));
}

#[test]
fn an_undeclared_creator_gets_the_default_depth() {
    let transport = transport("qos_default");
    let topic = topic("qos_default_topic");
    let _publisher = transport.publish(&TopicSpec::new(&topic).with_type(STRING)).expect("pub");
    let deepest = transport.subscribe(&TopicSpec::new(&topic)).expect("subscriber");
    assert_eq!(deepest.buffer_size(), qos::DEFAULT_MAX_DEPTH);
    let at_max = TopicSpec::new(&topic).with_type(STRING).keeping(32);
    assert!(transport.subscribe(&at_max).is_ok(), "depth 32 is the default maximum");
    let over = TopicSpec::new(&topic).with_type(STRING).keeping(33);
    assert!(matches!(transport.subscribe(&over), Err(PortError::ResourceLimit)));
}

#[test]
fn a_framework_topic_is_created_with_the_framework_depth_by_any_creator() {
    let transport = transport("qos_framework");
    // A tool that subscribes first (undeclared, typed) still gets the framework limits.
    let topic = "/clock_progress";
    let _tool = transport
        .subscribe(&TopicSpec::new(topic).with_type(STRING))
        .expect("first opener of a framework topic");
    let deep = TopicSpec::new(topic).with_type(STRING).keeping(qos::FRAMEWORK_MAX_DEPTH);
    assert!(transport.subscribe(&deep).is_ok(), "the framework depth is admitted");
    let beyond = TopicSpec::new(topic).with_type(STRING).keeping(qos::FRAMEWORK_MAX_DEPTH + 1);
    assert!(matches!(transport.subscribe(&beyond), Err(PortError::ResourceLimit)));
}

#[test]
fn a_declared_creator_sets_the_service_limits() {
    let transport = transport("qos_declared");
    let topic = topic("qos_declared_topic");
    let spec = TopicSpec::new(&topic).with_type(STRING).with_qos(declare("camera", 4, 8, 16));
    let _publisher = transport.publish(&spec).expect("declared publisher");
    let at_depth = TopicSpec::new(&topic).with_type(STRING).keeping(16);
    assert!(transport.subscribe(&at_depth).is_ok(), "the declared depth is admitted");
    let over = TopicSpec::new(&topic).with_type(STRING).keeping(17);
    assert!(matches!(transport.subscribe(&over), Err(PortError::ResourceLimit)));
}

#[test]
fn a_declared_opener_with_the_same_policy_is_admitted() {
    let transport = transport("qos_same");
    let topic = topic("qos_same_topic");
    let spec = TopicSpec::new(&topic).with_type(STRING).with_qos(declare("camera", 4, 8, 16));
    let _first = transport.publish(&spec).expect("first");
    transport.publish(&spec).expect("the same policy opens the same service");
    let subscriber = TopicSpec::new(&topic).with_type(STRING).with_qos(declare("camera", 4, 8, 16));
    transport.subscribe(&subscriber).expect("a declared subscriber of the same policy");
}

#[test]
fn a_declared_opener_with_another_name_is_refused() {
    let transport = transport("qos_name");
    let topic = topic("qos_name_topic");
    let created = TopicSpec::new(&topic).with_type(STRING).with_qos(declare("camera", 4, 8, 16));
    let _publisher = transport.publish(&created).expect("camera publisher");
    let other = TopicSpec::new(&topic).with_type(STRING).with_qos(declare("lidar", 4, 8, 16));
    assert!(matches!(transport.subscribe(&other), Err(PortError::QosMismatch)));
    assert!(matches!(transport.publish(&other), Err(PortError::QosMismatch)));
}

#[test]
fn the_same_name_with_different_numbers_is_refused() {
    let transport = transport("qos_numbers");
    let topic = topic("qos_numbers_topic");
    let created = TopicSpec::new(&topic).with_type(STRING).with_qos(declare("camera", 4, 8, 16));
    let _publisher = transport.publish(&created).expect("camera publisher");
    for other in
        [declare("camera", 4, 8, 32), declare("camera", 2, 8, 16), declare("camera", 4, 9, 16)]
    {
        let spec = TopicSpec::new(&topic).with_type(STRING).with_qos(other.clone());
        assert!(matches!(transport.subscribe(&spec), Err(PortError::QosMismatch)), "{other:?}");
        assert!(matches!(transport.publish(&spec), Err(PortError::QosMismatch)), "{other:?}");
    }
}

#[test]
fn an_undeclared_opener_adopts_the_stamped_limits() {
    let transport = transport("qos_adopt");
    let topic = topic("qos_adopt_topic");
    let created = TopicSpec::new(&topic).with_type(STRING).with_qos(declare("camera", 4, 8, 16));
    let _publisher = transport.publish(&created).expect("camera publisher");
    // Undeclared: no name to match, and the depth of the stamped policy applies.
    let adopted = transport.subscribe(&TopicSpec::new(&topic).with_type(STRING)).expect("adopts");
    assert_eq!(adopted.buffer_size(), 16);
    let over = TopicSpec::new(&topic).with_type(STRING).keeping(17);
    assert!(matches!(transport.subscribe(&over), Err(PortError::ResourceLimit)));
    let publisher = transport.publish(&TopicSpec::new(&topic).with_type(STRING)).expect("adopts");
    assert!(publisher.max_payload().is_none(), "a variable type is growable by default");
}

#[test]
fn an_undeclared_subscriber_on_a_missing_topic_is_not_available() {
    let transport = transport("qos_missing");
    let topic = topic("qos_missing_topic");
    // Neither an untyped nor a typed undeclared subscriber creates a service.
    let untyped = transport.subscribe(&TopicSpec::new(&topic));
    assert!(matches!(untyped, Err(PortError::NotAvailable)), "untyped refused");
    let typed = transport.subscribe(&TopicSpec::new(&topic).with_type(STRING));
    assert!(matches!(typed, Err(PortError::NotAvailable)), "typed refused");
    // The refused subscribers left no service: a declared producer creates it freely.
    let producer = TopicSpec::new(&topic).with_type(STRING).with_qos(declare("camera", 4, 8, 16));
    let _publisher = transport.publish(&producer).expect("the declared producer creates it");
    transport.subscribe(&TopicSpec::new(&topic).with_type(STRING)).expect("then it is found");
}

#[test]
fn an_undeclared_publisher_still_creates_the_service_with_the_default_policy() {
    let transport = transport("qos_pub_default");
    let topic = topic("qos_pub_default_topic");
    let _publisher = transport.publish(&TopicSpec::new(&topic).with_type(STRING)).expect("pub");
    let declared_default =
        TopicSpec::new(&topic).with_type(STRING).with_qos(declare("default", 8, 16, 32));
    transport.subscribe(&declared_default).expect("the default policy is the stamp");
    let camera = TopicSpec::new(&topic).with_type(STRING).with_qos(declare("camera", 4, 8, 16));
    assert!(matches!(transport.subscribe(&camera), Err(PortError::QosMismatch)));
}

#[test]
fn a_clock_seek_topic_is_an_ordinary_topic_with_the_default_policy() {
    let transport = transport("qos_clock_seek_ordinary");
    let topic = "/clock_seek";
    let _publisher = transport.publish(&TopicSpec::new(topic).with_type(STRING)).expect("pub");
    let deepest = transport.subscribe(&TopicSpec::new(topic)).expect("subscriber");
    assert_eq!(deepest.buffer_size(), qos::DEFAULT_MAX_DEPTH);
    let over = TopicSpec::new(topic).with_type(STRING).keeping(33);
    assert!(matches!(transport.subscribe(&over), Err(PortError::ResourceLimit)));
    // A framework topic ignores a declared policy; an ordinary topic refuses another one.
    let camera = TopicSpec::new(topic).with_type(STRING).with_qos(declare("camera", 4, 8, 16));
    assert!(matches!(transport.subscribe(&camera), Err(PortError::QosMismatch)));
}

#[test]
fn a_declared_endpoint_creates_a_framework_topic_with_the_framework_limits() {
    let transport = transport("qos_framework_declared");
    let topic = "/clock_progress";
    let declared = TopicSpec::new(topic).with_type(STRING).with_qos(declare("default", 8, 16, 32));
    let _publisher = transport.publish(&declared).expect("a declared publisher creates it");
    // Depth 32 is the framework's, not the 8 of the declared policy.
    let deep = TopicSpec::new(topic).with_type(STRING).keeping(qos::FRAMEWORK_MAX_DEPTH);
    transport.subscribe(&deep).expect("the topic carries the framework limits");
    let too_deep = TopicSpec::new(topic).with_type(STRING).keeping(qos::FRAMEWORK_MAX_DEPTH + 1);
    assert!(matches!(transport.subscribe(&too_deep), Err(PortError::ResourceLimit)));
}

#[test]
fn a_policy_outside_the_hard_bounds_is_refused_at_creation() {
    let transport = transport("qos_bounds");
    let topic = topic("qos_bounds_topic");
    let refused = [
        declare("camera", 0, 8, 16),
        declare("camera", 4, 0, 16),
        declare("camera", 4, 8, 0),
        declare("camera", 257, 8, 16),
        declare("camera", 4, 257, 16),
        declare("camera", 4, 8, 1025),
        declare("bad name", 4, 8, 16),
        declare("", 4, 8, 16),
        declare("a/b", 4, 8, 16),
    ];
    for request in refused {
        let spec = TopicSpec::new(&topic).with_type(STRING).with_qos(request.clone());
        assert_eq!(transport.publish(&spec).err(), Some(PortError::InvalidArgument), "{request:?}");
    }
    // The bounds themselves are admitted.
    let at_bounds =
        TopicSpec::new(&topic).with_type(STRING).with_qos(declare("camera", 256, 256, 1024));
    assert!(transport.publish(&at_bounds).is_ok(), "the hard bounds are admitted");
}

#[test]
fn a_fixed_size_type_is_bounded_to_its_size_whatever_the_payload_bound() {
    let transport = transport("qos_fixed");
    let topic = topic("qos_fixed_topic");
    let spec = TopicSpec::new(&topic)
        .with_type(CLOCK)
        .with_md5(CLOCK_MD5)
        .with_definition("time clock")
        .with_payload_capacity(PayloadCapacity::Growable);
    let publisher = transport.publish(&spec).expect("clock publisher");
    assert_eq!(publisher.max_payload(), Some(8));
    // A bound has no effect on a fixed-size type, also one below its size.
    let smaller = spec.with_payload_capacity(PayloadCapacity::Bounded(4));
    let bounded = transport.publish(&smaller).expect("a bound below the fixed size is ignored");
    assert_eq!(bounded.max_payload(), Some(8));
}

#[test]
fn a_variable_size_type_is_growable_unless_its_endpoint_names_a_bound() {
    let transport = transport("qos_variable");
    let growable_topic = topic("qos_variable_topic");
    let growable =
        transport.publish(&TopicSpec::new(&growable_topic).with_type(STRING)).expect("pub");
    assert!(growable.max_payload().is_none());
    let mut loan = growable.loan(1 << 20).expect("a megabyte is no limit");
    loan.as_mut_bytes().fill(1);

    let other = topic("qos_variable_bounded");
    let bounded = TopicSpec::new(&other)
        .with_type(STRING)
        .with_payload_capacity(PayloadCapacity::Bounded(300));
    let publisher = transport.publish(&bounded).expect("bounded publisher");
    assert_eq!(publisher.max_payload(), Some(300));
    assert!(matches!(publisher.loan(301), Err(PortError::TooLarge { requested: 301, limit: 300 })));
    let mut sample = publisher.loan(300).expect("the limit is admitted");
    sample.as_mut_bytes().fill(2);
    sample.send_frame(sealed_send::metadata(), 0).expect("send");
}

#[test]
fn a_declared_policy_does_not_bound_the_payload() {
    let transport = transport("qos_declared_payload");
    let topic = topic("qos_declared_payload_topic");
    let spec = TopicSpec::new(&topic).with_type(STRING).with_qos(declare("camera", 4, 8, 16));
    let publisher = transport.publish(&spec).expect("declared publisher");
    assert!(publisher.max_payload().is_none(), "the policy names no payload bound");
    // A second endpoint of the same policy bounds its own messages.
    let bounded = spec.with_payload_capacity(PayloadCapacity::Bounded(64));
    assert_eq!(transport.publish(&bounded).expect("bounded").max_payload(), Some(64));
}
