//! A topic's event service has the listener slots of its service policy, and a listener
//! beyond them is refused with the backend error.
#![allow(clippy::expect_used)]

use hypha_core::port::{PortError, QosPolicy, QosRequest, TopicSpec};
use hypha_iceoryx2::{Iox2Transport, qos};
use hypha_testkit::unique_name;

/// Open listeners on a topic under `request` until one is refused, and return how many opened.
fn listener_slots(label: &str, request: QosRequest) -> usize {
    let topic = format!("/{}", unique_name(label));
    let spec = TopicSpec::new(&topic).with_type("std_msgs/String").with_qos(request);
    let transport = Iox2Transport::with_name(&unique_name(label)).expect("transport");

    let _publisher = transport.publish(&spec).expect("publisher");
    let first = transport.subscribe(&spec).expect("first subscriber");
    let mut listeners = Vec::new();
    loop {
        match first.open_listeners() {
            Ok(mut listener) => {
                listeners.push(listener.pop().expect("a literal topic has an event service"));
            }
            Err(cause) => {
                assert_eq!(
                    cause,
                    PortError::Backend(7),
                    "a listener that cannot open is refused with the backend error"
                );
                break;
            }
        }
        assert!(listeners.len() <= 1024, "the event service refused no listener");
    }
    // A subscriber owns no listener, so creating one still succeeds.
    assert!(transport.subscribe(&spec).is_ok(), "a subscriber needs no listener slot");

    let opened = listeners.len();
    drop(listeners.pop());
    assert!(first.open_listeners().is_ok(), "a listener opens once a slot is free");
    opened
}

#[test]
fn a_topic_has_a_listener_slot_for_each_subscriber_its_policy_admits() {
    // A topic's event service is sized by the topic's subscribers plus the observer slots.
    let default = qos::default_policy();
    assert_eq!(
        listener_slots("listener_full", QosRequest::Undeclared),
        qos::subscriber_capacity("/listener", &default)
    );
    let declared = QosPolicy::new("sensor", 4, 4, 8);
    assert_eq!(
        listener_slots("listener_full_declared", QosRequest::Declared(declared.clone())),
        qos::subscriber_capacity("/listener", &declared)
    );
}
