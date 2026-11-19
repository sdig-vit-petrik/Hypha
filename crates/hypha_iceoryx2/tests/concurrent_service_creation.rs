//! Several endpoints may open the same service at the same moment.

#![allow(clippy::expect_used)]

use std::sync::{Arc, Barrier};
use std::time::Instant;

use hypha_core::port::TopicSpec;
use hypha_iceoryx2::Iox2Transport;
use hypha_testkit::unique_name as unique;

const DATA_TYPE: &str = "tf2_msgs/TFMessage";
const MD5: &str = "94810edda583a504dfda3829e70d7eec";

fn topic_spec(topic: &str) -> TopicSpec {
    let mut spec = TopicSpec::new(topic).with_type("hypha_test/Data");
    spec.data_type = DATA_TYPE.to_string();
    spec.md5 = MD5.to_string();
    spec
}

/// Advertise `topic` from `workers` nodes that all start together.
fn race_to_advertise(topic: &str, workers: usize) -> Vec<Result<(), String>> {
    let barrier = Arc::new(Barrier::new(workers));
    let mut handles = Vec::with_capacity(workers);
    for index in 0..workers {
        let barrier = Arc::clone(&barrier);
        let topic = topic.to_string();
        handles.push(std::thread::spawn(move || {
            let transport = Iox2Transport::with_name(&unique(&format!("racer_{index}")))
                .expect("transport creation succeeds");
            let spec = topic_spec(&topic);
            barrier.wait();
            let started = Instant::now();
            transport.publish(&spec).map(|_publisher| ()).map_err(|cause| {
                format!("worker {index} failed after {:?}: {cause:?}", started.elapsed())
            })
        }));
    }
    handles.into_iter().map(|handle| handle.join().expect("worker thread finishes")).collect()
}

#[test]
fn eight_nodes_advertising_one_topic_together_all_succeed() {
    let topic = format!("/concurrent/{}", unique("advertise"));

    let results = race_to_advertise(&topic, 8);

    let failures: Vec<&String> =
        results.iter().filter_map(|result| result.as_ref().err()).collect();
    assert!(failures.is_empty(), "concurrent advertise refused a node: {failures:?}");
}

#[test]
fn repeated_concurrent_advertise_rounds_all_succeed() {
    for round in 0..4 {
        let topic = format!("/concurrent/{}/{round}", unique("rounds"));

        let results = race_to_advertise(&topic, 6);

        let failures: Vec<&String> =
            results.iter().filter_map(|result| result.as_ref().err()).collect();
        assert!(failures.is_empty(), "round {round} refused a node: {failures:?}");
    }
}

#[test]
fn a_setting_the_service_does_not_admit_is_refused_at_once() {
    // A depth above the domain limit is a resource limit, not contention: never retried.
    let transport =
        Iox2Transport::with_name(&unique("mismatch")).expect("transport creation succeeds");
    let topic = format!("/concurrent/{}", unique("mismatch_topic"));
    let _publisher =
        transport.publish(&topic_spec(&topic)).expect("the publisher creates the service");
    let spec = topic_spec(&topic).keeping(33);

    let started = Instant::now();
    let refusal = transport.subscribe(&spec).err();
    let elapsed = started.elapsed();

    assert_eq!(
        refusal,
        Some(hypha_core::port::PortError::ResourceLimit),
        "a depth above the limit must keep reporting itself"
    );
    assert!(
        elapsed < std::time::Duration::from_millis(400),
        "a mismatch that cannot change was retried for {elapsed:?}"
    );
}
