//! The domain's schema lock is never waited for without a bound.

#![allow(clippy::expect_used)]

use std::fs::OpenOptions;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use hypha_core::port::{PortError, TopicSpec};
use hypha_iceoryx2::{Iox2Transport, domain};
use hypha_msgdef::md5::md5_from_definition;
use hypha_testkit::unique_name as unique;

const DATA_TYPE: &str = "hypha_test/SchemaLock";
const DEFINITION: &str = "int32 value\n";

/// The lock file of this domain, as the schema store names it.
#[allow(clippy::expect_used)]
fn domain_schema_lock_path() -> std::path::PathBuf {
    let config = domain::config().expect("the domain configuration");
    let root = std::path::PathBuf::from(config.global.root_path().as_str());
    let prefix = config.global.prefix.as_str();
    root.join(format!("{prefix}hypha_schemas.lock"))
}

fn typed_spec(topic: &str) -> TopicSpec {
    let md5 = md5_from_definition(DATA_TYPE, DEFINITION).expect("a valid message definition");
    TopicSpec::new(topic).with_type(DATA_TYPE).with_md5(&md5).with_definition(DEFINITION)
}

#[test]
#[allow(clippy::expect_used)]
fn a_frozen_schema_lock_fails_a_typed_publisher_within_a_bound() {
    let lock_path = domain_schema_lock_path();
    std::fs::create_dir_all(lock_path.parent().expect("a parent")).expect("the iceoryx2 root");
    let held = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .expect("open the schema lock");
    held.lock().expect("hold the schema lock exclusively, as a frozen process would");

    let transport = Iox2Transport::with_name(&unique("schema_lock")).expect("transport");
    let spec = typed_spec(&format!("/{}", unique("schema_lock_topic")));
    let (sender, receiver) = mpsc::channel();
    let started = Instant::now();
    std::thread::spawn(move || {
        let outcome = transport.publish(&spec).err();
        let _ = sender.send(outcome);
    });
    let outcome = receiver
        .recv_timeout(Duration::from_secs(10))
        .expect("the publisher returned: the schema write must not wait without a bound");
    assert_eq!(
        outcome,
        Some(PortError::Backend(3)),
        "a schema the lock cannot be taken for is not written, and the publisher fails"
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the wait is bounded: {:?}",
        started.elapsed()
    );

    drop(held);
    let transport = Iox2Transport::with_name(&unique("schema_lock_after")).expect("transport");
    let spec = typed_spec(&format!("/{}", unique("schema_lock_topic_after")));
    transport.publish(&spec).expect("with the lock free, the publisher is created");
}
