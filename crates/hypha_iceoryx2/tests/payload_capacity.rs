//! A publisher sends up to its payload capacity and refuses larger messages.

mod support;
use support::sealed_send;

use hypha_core::port::{PayloadCapacity, PortError, TopicSpec};
use hypha_iceoryx2::{Iox2Publisher, Iox2Transport};
use hypha_testkit::unique_name;

/// A message size no byte limit of these tests is set to: 64 KiB.
const DEFAULT_PAYLOAD: usize = 64 * 1024;

fn loan_error(publisher: &mut Iox2Publisher, size: usize) -> Option<PortError> {
    publisher.loan(size).err()
}

#[test]
#[allow(clippy::expect_used)]
fn a_default_publisher_of_a_variable_type_is_growable() {
    let transport = Iox2Transport::with_name(&unique_name("cap_default")).expect("transport");
    let topic = format!("/{}", unique_name("cap_default_topic"));
    let mut publisher =
        transport.publish(&TopicSpec::new(&topic).with_type("hypha_test/Data")).expect("publish");

    // No byte limit: a message of 64 KiB, and of 1 MiB, is admitted.
    assert_eq!(loan_error(&mut publisher, DEFAULT_PAYLOAD), None);
    assert_eq!(loan_error(&mut publisher, 1 << 20), None);
}

#[test]
#[allow(clippy::expect_used)]
fn a_bounded_publisher_delivers_a_full_size_message() {
    let transport = Iox2Transport::with_name(&unique_name("cap_bounded")).expect("transport");
    let topic = format!("/{}", unique_name("cap_bounded_topic"));
    let spec = TopicSpec::new(&topic)
        .with_type("hypha_test/Data")
        .with_payload_capacity(PayloadCapacity::Bounded(1024));
    let mut publisher = transport.publish(&spec).expect("publish");
    let subscriber = transport.subscribe(&spec).expect("subscribe");

    assert_eq!(
        loan_error(&mut publisher, 1025),
        Some(PortError::TooLarge { requested: 1025, limit: 1024 })
    );
    let mut loan = publisher.loan(1024).expect("loan at capacity");
    loan.as_mut_bytes().fill(7);
    loan.send_frame(sealed_send::metadata(), 0).expect("send");
    let sample = subscriber.try_receive().expect("receive").expect("the sample");
    assert_eq!(sample.payload(), [7_u8; 1024].as_slice());
}

#[test]
#[allow(clippy::expect_used)]
fn a_growable_publisher_sends_beyond_the_default_capacity() {
    let transport = Iox2Transport::with_name(&unique_name("cap_growable")).expect("transport");
    let topic = format!("/{}", unique_name("cap_growable_topic"));
    let spec = TopicSpec::new(&topic)
        .with_type("hypha_test/Data")
        .with_payload_capacity(PayloadCapacity::Growable);
    let publisher = transport.publish(&spec).expect("publish");
    let subscriber = transport.subscribe(&spec).expect("subscribe");

    let size = 4 * DEFAULT_PAYLOAD;
    let mut loan = publisher.loan(size).expect("a growable publisher grows");
    loan.as_mut_bytes().fill(9);
    loan.send_frame(sealed_send::metadata(), 0).expect("send");
    let sample = subscriber.try_receive().expect("receive").expect("the sample");
    assert_eq!(sample.payload().len(), size);
}

#[test]
#[allow(clippy::expect_used)]
fn a_small_initial_payload_is_a_hint_and_the_publisher_still_grows() {
    let transport = Iox2Transport::with_name(&unique_name("cap_initial")).expect("transport");
    let topic = format!("/{}", unique_name("cap_initial_topic"));
    let spec = TopicSpec::new(&topic)
        .with_type("hypha_test/Data")
        .with_payload_capacity(PayloadCapacity::Growable)
        .with_initial_payload(64);
    let publisher = transport.publish(&spec).expect("publish");
    let subscriber = transport.subscribe(&spec).expect("subscribe");

    // From a 64-byte first slot through 15 growths, every message is delivered whole.
    let mut size = 32;
    while size <= (1 << 20) {
        let mut loan = publisher.loan(size).expect("a growable publisher grows");
        loan.as_mut_bytes().fill(5);
        loan.send_frame(sealed_send::metadata(), 0).expect("send");
        let sample = subscriber.try_receive().expect("receive").expect("the sample");
        assert_eq!(sample.payload().len(), size);
        assert!(sample.payload().iter().all(|byte| *byte == 5));
        size *= 2;
    }
}

#[test]
#[allow(clippy::expect_used)]
fn a_zero_capacity_is_refused_at_creation() {
    let transport = Iox2Transport::with_name(&unique_name("cap_zero")).expect("transport");
    let topic = format!("/{}", unique_name("cap_zero_topic"));
    let spec = TopicSpec::new(&topic)
        .with_type("hypha_test/Data")
        .with_payload_capacity(PayloadCapacity::Bounded(0));
    assert_eq!(transport.publish(&spec).err(), Some(PortError::InvalidArgument));
}
