//! Latching must not treat raw bytes as wire frames.

mod support;
use support::sealed_send;

use hypha_core::port::TopicSpec;
use hypha_iceoryx2::{Iox2Publisher, Iox2Transport};
use hypha_testkit::unique_name;

#[allow(clippy::expect_used)]
fn send(publisher: &mut Iox2Publisher, payload: &[u8]) {
    let mut loan = publisher.loan(payload.len()).expect("loan");
    loan.as_mut_bytes().copy_from_slice(payload);
    loan.send_frame(sealed_send::metadata(), 0).expect("send");
}

#[test]
#[allow(clippy::expect_used)]
fn a_latch_retains_only_the_last_raw_payload() {
    let transport = Iox2Transport::with_name(&unique_name("latch_raw")).expect("transport");
    let topic = format!("/{}", unique_name("latch_raw_topic"));
    let spec = TopicSpec::new(&topic).with_type("hypha_test/Data").latched();
    let mut publisher = transport.publish(&spec).expect("publish");
    send(&mut publisher, b"raw-data-0000001");
    send(&mut publisher, b"raw-data-0000002");

    let subscriber = transport.subscribe(&spec).expect("subscribe");
    transport.replay_latched();
    let last = subscriber.try_receive().expect("receive").expect("the last sample");
    assert_eq!(last.payload(), b"raw-data-0000002");
    assert!(subscriber.try_receive().expect("receive").is_none(), "the latch keeps one sample");
}
