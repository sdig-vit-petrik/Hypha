//! A latch must not outlive its publisher.

mod support;
use support::sealed_send;

use std::thread::sleep;
use std::time::Duration;

use hypha_core::port::{QosPolicy, QosRequest, TopicSpec};
use hypha_iceoryx2::Iox2Transport;
use hypha_testkit::unique_name;

#[test]
#[allow(clippy::expect_used)]
fn dropped_latched_publisher_does_not_replay_to_a_late_subscriber() {
    let transport = Iox2Transport::with_name(&unique_name("latch_lifetime"))
        .expect("an iceoryx2 transport: /dev/shm and /tmp/iceoryx2 must be writable");
    let topic = format!("/{}", unique_name("latch_lifetime_topic"));
    let spec = TopicSpec::new(&topic).with_type("hypha_test/Data").keeping(1).latched();
    let publisher = transport.publish(&spec).expect("publish");
    let mut loan = publisher.loan(4).expect("loan");
    loan.as_mut_bytes().copy_from_slice(b"held");
    loan.send_frame(sealed_send::metadata(), 0).expect("send");
    drop(publisher);

    sleep(Duration::from_millis(350));
    // The publisher is gone, so the late subscriber declares the service it creates.
    let declared = TopicSpec::new(&topic)
        .with_type("hypha_test/Data")
        .keeping(1)
        .with_qos(QosRequest::Declared(QosPolicy::new("default", 8, 16, 32)));
    let subscriber = transport.subscribe(&declared).expect("subscribe");
    sleep(Duration::from_millis(350));
    assert!(subscriber.try_receive().expect("receive").is_none());
}
