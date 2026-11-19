//! Subscription attachments of the Rust `WaitSet` facade.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use hypha_node::sdk::{BusNode, PublisherOptions, RawPublisher, SubscriberOptions, WaitSet};
use hypha_testkit::unique_name;

const DATA_TYPE: &str = "std_msgs/Counter";
const MD5: &str = "d9d213e8c146065ec958333c617eb1a6";
const DEFINITION: &str = "uint32 value\n";

fn setup(label: &str) -> (BusNode, String, RawPublisher) {
    let node = BusNode::new(&unique_name(label)).expect("node");
    let topic = format!("/{}", unique_name(label));
    let options = PublisherOptions::default();
    let publisher = node
        .publish_raw_configured(&topic, DATA_TYPE, MD5, DEFINITION, options)
        .expect("publisher");
    (node, topic, publisher)
}

type Seen = Arc<Mutex<Vec<Vec<u8>>>>;

fn recorder(
    seen: &Seen,
) -> impl FnMut(&hypha_node::sdk::Delivery<'_>) -> Result<(), String> + Send + use<> {
    let sink = Arc::clone(seen);
    move |delivery| {
        let payload = delivery.payload.ok_or("unparsed frame")?;
        sink.lock().map_err(|_| "poisoned")?.push(payload.to_vec());
        Ok(())
    }
}

#[test]
fn subscription_callback_receives_each_published_payload() {
    let (node, topic, mut publisher) = setup("attach_receive");
    let options = SubscriberOptions::default().keeping(8);
    let subscriber = node.subscribe_raw_with_options(&topic, options).expect("subscriber");
    let seen: Seen = Arc::default();
    let mut waitset = WaitSet::new(&node).expect("waitset");
    let mut record = recorder(&seen);
    let expected_topic = topic.clone();
    waitset
        .attach_subscription(subscriber, move |delivery| {
            if delivery.identity.topic != expected_topic {
                return Err(format!("delivered on {}", delivery.identity.topic));
            }
            record(delivery)
        })
        .expect("attach");
    for value in 1_u8..=3 {
        assert!(publisher.publish(&[value, 0, 0, 0]));
    }
    let result = waitset.run_for(Duration::from_millis(500));
    assert!(result.ok(), "{:?}", result.err());
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen, vec![vec![1, 0, 0, 0], vec![2, 0, 0, 0], vec![3, 0, 0, 0]]);
}

#[test]
fn attach_between_runs_delivers_on_the_next_run() {
    let (node, topic, mut publisher) = setup("attach_between");
    let mut waitset = WaitSet::new(&node).expect("waitset");
    assert!(waitset.run_for(Duration::from_millis(20)).ok());
    let subscriber = node.subscribe_raw(&topic, 8).expect("subscriber");
    let seen: Seen = Arc::default();
    waitset.attach_subscription(subscriber, recorder(&seen)).expect("attach");
    assert!(publisher.publish(&[9, 0, 0, 0]));
    assert!(waitset.run_for(Duration::from_millis(300)).ok());
    assert_eq!(seen.lock().unwrap().len(), 1);
}

#[test]
fn a_callback_error_stops_the_run_with_its_attachment() {
    let (node, topic, mut publisher) = setup("attach_error");
    let subscriber = node.subscribe_raw(&topic, 8).expect("subscriber");
    let mut waitset = WaitSet::new(&node).expect("waitset");
    let id = waitset
        .attach_subscription(subscriber, |_| Err("relay refused the frame".to_owned()))
        .expect("attach");
    assert!(publisher.publish(&[1, 0, 0, 0]));
    let result = waitset.run_for(Duration::from_secs(2));
    let error = result.err().expect("the run fails");
    assert_eq!(result.failed_attachment, id.get());
    assert!(error.contains("relay refused the frame"), "{error}");
}

#[test]
fn detach_stops_delivery() {
    let (node, topic, mut publisher) = setup("attach_detach");
    let subscriber = node.subscribe_raw(&topic, 8).expect("subscriber");
    let seen: Seen = Arc::default();
    let mut waitset = WaitSet::new(&node).expect("waitset");
    let id = waitset.attach_subscription(subscriber, recorder(&seen)).expect("attach");
    waitset.detach(id).expect("detach");
    assert!(waitset.detach(id).is_err(), "a second detach names an unknown attachment");
    assert!(publisher.publish(&[1, 0, 0, 0]));
    assert!(waitset.run_for(Duration::from_millis(200)).ok());
    assert!(seen.lock().unwrap().is_empty());
    assert_eq!(Arc::strong_count(&seen), 1, "the detach released the callback");
}

#[test]
fn a_waking_subscription_delivers_every_queued_sample_and_ends_the_run_early() {
    let (node, topic, mut publisher) = setup("attach_waking");
    let options = SubscriberOptions::default().keeping(8);
    let subscriber = node.subscribe_raw_with_options(&topic, options).expect("subscriber");
    let seen: Seen = Arc::default();
    let mut waitset = WaitSet::new(&node).expect("waitset");
    waitset.attach_waking_subscription(subscriber, recorder(&seen)).expect("attach");
    // Two samples queued before the run: one wake must deliver both.
    assert!(publisher.publish(&[1, 0, 0, 0]));
    assert!(publisher.publish(&[2, 0, 0, 0]));
    let began = std::time::Instant::now();
    assert!(waitset.run_for(Duration::from_millis(500)).ok());
    assert!(began.elapsed() < Duration::from_millis(250), "the samples end the run early");
    assert_eq!(seen.lock().unwrap().len(), 2, "every queued sample is delivered");
    // The stop does not carry into the next run: it waits for its whole slice.
    let began = std::time::Instant::now();
    assert!(waitset.run_for(Duration::from_millis(60)).ok());
    assert!(began.elapsed() >= Duration::from_millis(50));
}

#[test]
fn a_closure_subscription_receives_samples_in_a_run() {
    let (node, topic, mut publisher) = setup("closure_receive");
    let options = SubscriberOptions::default().keeping(8);
    let subscriber = node.subscribe_raw_with_options(&topic, options).expect("subscriber");
    let mut waitset = WaitSet::new(&node).expect("waitset");
    // The closure owns its state; nothing outside it holds a C context.
    let mut received: Vec<u8> = Vec::new();
    let seen: Seen = Arc::default();
    let sink = Arc::clone(&seen);
    waitset
        .attach_subscription(subscriber, move |delivery| {
            let payload = delivery.payload.ok_or("unparsed frame")?;
            let first = *payload.first().ok_or("empty payload")?;
            received.push(first);
            sink.lock().map_err(|_| "poisoned")?.push(received.clone());
            Ok(())
        })
        .expect("attach");
    for value in [4_u8, 5, 6] {
        assert!(publisher.publish(&[value, 0, 0, 0]));
    }
    let result = waitset.run_for(Duration::from_millis(500));
    assert!(result.ok(), "{:?}", result.err());
    let last = seen.lock().unwrap().last().cloned();
    assert_eq!(last, Some(vec![4, 5, 6]), "the closure saw every sample, in order");
}

#[test]
fn a_closure_subscription_failure_ends_the_run_with_that_attachment() {
    let (node, topic, mut publisher) = setup("closure_failure");
    let ok_subscriber = node.subscribe_raw(&topic, 8).expect("subscriber");
    let failing_subscriber = node.subscribe_raw(&topic, 8).expect("subscriber");
    let mut waitset = WaitSet::new(&node).expect("waitset");
    let seen: Seen = Arc::default();
    let healthy = waitset.attach_subscription(ok_subscriber, recorder(&seen)).expect("attach");
    let failing = waitset
        .attach_subscription(failing_subscriber, |delivery| {
            let size = delivery.payload.map_or(0, <[u8]>::len);
            Err(format!("closure refused a {size}-byte frame"))
        })
        .expect("attach");
    assert_ne!(healthy, failing);
    assert!(publisher.publish(&[1, 0, 0, 0]));
    let result = waitset.run_for(Duration::from_secs(2));
    assert!(!result.ok(), "a failing closure fails the run");
    assert_eq!(result.reason, hypha_node::sdk::StopReason::RuntimeFailed);
    assert_eq!(result.failed_attachment, failing.get());
    let error = result.err().expect("the run reports the failure");
    assert!(error.contains("closure refused a 4-byte frame"), "{error}");
}
