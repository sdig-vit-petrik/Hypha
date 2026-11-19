//! Retained schemas are resolvable across processes without running a `WaitSet`.
#![allow(clippy::expect_used)]

use std::process::{Child, Command};
use std::time::{Duration, Instant};

use hypha_node::sdk::BusNode;
use hypha_node::sdk::schema_hash_from_md5;
use hypha_testkit::unique_name;

const ROLE: &str = "HYPHA_SCHEMA_TEST_ROLE";
const TOPIC: &str = "HYPHA_SCHEMA_TEST_TOPIC";
const DATA_TYPE: &str = "schema_test/Counter";
const MD5: &str = "d9d213e8c146065ec958333c617eb1a6";
const DEFINITION: &str = "uint32 value\n";
const CHILD_LIFETIME: Duration = Duration::from_secs(10);
const RESOLVE_LIMIT: Duration = Duration::from_secs(5);
const RESOLVE_RETRY: Duration = Duration::from_millis(50);

/// The body of the child process. It creates a publisher and never creates a `WaitSet`.
#[test]
fn schema_child_process() {
    let Ok(role) = std::env::var(ROLE) else {
        return;
    };
    if role != "publisher_without_waitset" {
        return;
    }
    let topic = std::env::var(TOPIC).expect("topic");
    let node = BusNode::new(&unique_name("schema_child_node")).expect("node");
    let mut publisher = node.publish_raw(&topic, DATA_TYPE, MD5, DEFINITION).expect("publisher");

    let deadline = Instant::now().checked_add(CHILD_LIFETIME).expect("deadline");
    while Instant::now() < deadline {
        let _ = publisher.publish(&[1, 0, 0, 0]);
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn spawn_child(topic: &str) -> Child {
    Command::new(std::env::current_exe().expect("test binary"))
        .args(["schema_child_process", "--exact", "--nocapture", "--test-threads=1"])
        .env(ROLE, "publisher_without_waitset")
        .env(TOPIC, topic)
        .spawn()
        .expect("child process")
}

fn stop_child(mut child: Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn publisher_without_waitset_has_schema_resolved_by_second_process() {
    let topic = format!("/{}", unique_name("schema_no_waitset"));
    let child = spawn_child(&topic);

    let reader = BusNode::new(&unique_name("schema_reader")).expect("reader node");
    let schema_hash = schema_hash_from_md5(MD5).expect("a valid MD5");

    let deadline = Instant::now().checked_add(RESOLVE_LIMIT).expect("deadline");
    let mut resolved_schema = None;
    while Instant::now() < deadline {
        if let Ok(Some(schema)) = reader.resolve_schema_hash(&topic, schema_hash) {
            resolved_schema = Some(schema);
            break;
        }
        std::thread::sleep(RESOLVE_RETRY);
    }

    let definition_res = reader.lookup_definition(&topic, DATA_TYPE, MD5);

    stop_child(child);

    let schema = resolved_schema.expect("schema resolved from child that never ran a WaitSet");
    assert_eq!(schema.data_type, DATA_TYPE);
    assert_eq!(schema.md5, MD5);
    assert_eq!(schema.definition, DEFINITION);

    let def = definition_res.expect("definition looked up from child that never ran a WaitSet");
    assert_eq!(def.as_deref(), Some(DEFINITION));
}
