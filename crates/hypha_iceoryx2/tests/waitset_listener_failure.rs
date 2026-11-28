//! A `WaitSet` that cannot open a subscriber's listener reports the real cause.
#![allow(clippy::expect_used)]

use hypha_core::port::{PortError, QosPolicy, QosRequest, TopicSpec};
use hypha_iceoryx2::Iox2Transport;
use hypha_iceoryx2::waitset::Trigger;
use hypha_testkit::unique_name;

#[test]
fn a_waitset_attach_reports_a_full_event_service_as_a_backend_error() {
    let topic = format!("/{}", unique_name("waitset_listener_full"));
    let spec = TopicSpec::new(&topic)
        .with_type("std_msgs/String")
        .with_qos(QosRequest::Declared(QosPolicy::new("default", 8, 16, 32)));
    let transport =
        Iox2Transport::with_name(&unique_name("waitset_listener_full_node")).expect("transport");
    let control = unique_name("waitset_listener_full_ctl");
    let mut waitset = transport.waitset(&control).expect("waitset");

    let first = transport.subscribe(&spec).expect("first subscriber");
    let mut listeners = Vec::new();
    // One listener slot per subscriber the service admits, observer slots included.
    let slots =
        hypha_iceoryx2::qos::subscriber_capacity(&topic, &QosPolicy::new("default", 8, 16, 32));
    for _ in 0..slots {
        listeners.push(first.open_listeners().expect("listener"));
    }

    let attached = match first.open_listeners() {
        Ok(listener) => waitset.attach(listener, Trigger::Event),
        Err(cause) => Err(cause),
    };
    assert_eq!(attached.err(), Some(PortError::Backend(7)));
}
