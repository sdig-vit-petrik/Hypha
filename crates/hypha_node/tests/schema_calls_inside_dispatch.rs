//! Schema resolution from a dispatcher callback is refused with `InvalidState`, even for
//! a hash the node resolved before the run.
#![allow(clippy::expect_used)]

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use hypha_node::HyphaStatus;
use hypha_node::sdk::{
    BusNode, Delivery, ResolvedSchema, SchemaError, SubscriberOptions, WaitSet,
    schema_hash_from_md5,
};
use hypha_testkit::unique_name;

const DATA_TYPE: &str = "std_msgs/String";
const MD5: &str = "992ce8a1687cec8c8bd883ec73ca41d1";
const DEFINITION: &str = "string data\n";

type Resolution = Result<Option<ResolvedSchema>, SchemaError>;

/// The subscription callback: resolve `hash` on `topic` once, from inside the run.
struct ResolveInCallback {
    bus: Arc<BusNode>,
    topic: String,
    hash: u64,
    observed: Arc<Mutex<Option<Resolution>>>,
}

impl ResolveInCallback {
    fn call(&mut self, _delivery: &Delivery<'_>) {
        let resolved = self.bus.resolve_schema_hash(&self.topic, self.hash);
        let mut slot = self.observed.lock().unwrap_or_else(PoisonError::into_inner);
        *slot = Some(resolved);
    }
}

#[test]
fn a_callback_refuses_resolve_hash_even_for_a_known_hash() {
    let bus = Arc::new(BusNode::new(&unique_name("resolve_in_callback")).expect("node"));
    let topic = format!("/{}", unique_name("topic"));
    let mut publisher = bus.publish_raw(&topic, DATA_TYPE, MD5, DEFINITION).expect("publisher");
    let hash = schema_hash_from_md5(MD5).expect("a valid MD5");
    let before = bus.resolve_schema_hash(&topic, hash);
    assert!(matches!(before, Ok(Some(_))), "the hash resolves before the run: {before:?}");

    let options = SubscriberOptions::default().keeping(4);
    let subscriber = bus.subscribe_raw_with_options(&topic, options).expect("subscriber");
    let observed = Arc::new(Mutex::new(None));
    let mut callback = ResolveInCallback {
        bus: Arc::clone(&bus),
        topic: topic.clone(),
        hash,
        observed: Arc::clone(&observed),
    };
    let mut waitset = WaitSet::new(&bus).expect("waitset");
    let attachment = waitset
        .attach_waking_subscription(subscriber, move |delivery| {
            callback.call(delivery);
            Ok(())
        })
        .expect("attach");
    assert!(publisher.publish(b"\x01\x00\x00\x00a"));
    let report = waitset.run_for(Duration::from_secs(5));
    assert!(report.ok(), "the run fails: {:?}", report.err());
    // The registration holds a share of the node; the detach releases it.
    waitset.detach(attachment).expect("detach");
    drop(waitset);

    let resolved = observed.lock().unwrap_or_else(PoisonError::into_inner).take();
    let resolved = resolved.expect("the callback ran inside the run");
    assert_eq!(
        resolved,
        Err(SchemaError::Unknown(HyphaStatus::InvalidState as i32)),
        "resolve_hash never answers inside a callback, even from what it already knows"
    );
}
