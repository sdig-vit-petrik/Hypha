//! The no-publisher warning of a subscription. Each test needs its own process (nextest).
#![allow(clippy::expect_used)]

mod common;
use crate::common::attach;
use crate::common::c_text;
use crate::common::create_node_with_file;
use crate::common::unique;

use std::ffi::c_void;
use std::path::PathBuf;
use std::ptr;

use hypha_ffi::waitset::*;
use hypha_ffi::*;

const DATA_TYPE: &str = "std_msgs/Counter";
const MD5: &str = "d9d213e8c146065ec958333c617eb1a6";
const WARNING_MARKER: &str = "no publisher connected to topic";

extern "C" fn ignore_delivery(
    _delivery: *const HyphaDelivery,
    _context: *mut c_void,
) -> HyphaCallbackResult {
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

struct Fixture {
    log_file: PathBuf,
    topic: String,
    node: *mut HyphaNode,
    waitset: *mut HyphaWaitSet,
}

fn fixture(label: &str, warn_after_nanos: u64) -> Fixture {
    let topic = format!("/{}", unique("no_publisher"));
    fixture_on(label, &topic, warn_after_nanos)
}

/// A node that logs to a file, and a `WaitSet` that dispatches one subscription on
/// `topic`. A wildcard topic is polled.
fn fixture_on(label: &str, topic: &str, warn_after_nanos: u64) -> Fixture {
    let dir = std::env::temp_dir().join(unique(label));
    std::fs::create_dir_all(&dir).expect("create fixture directory");
    let parameters = dir.join("params.toml");
    let table =
        "[log]\nconsole = false\n[log.sink.main]\npath = \"warning.log\"\nlevel = \"info\"\n";
    std::fs::write(&parameters, table).expect("write parameter fixture");
    let node = create_node_with_file(&unique(label), Some(&parameters));

    let mut waitset = ptr::null_mut();
    assert_eq!(unsafe { hypha_node_default_waitset(node, &raw mut waitset) }, HyphaStatus::Ok);

    let topic_c = c_text(topic);
    let data_type = c_text(DATA_TYPE);
    let md5 = c_text(MD5);
    let qos = HyphaSubscriberOptions {
        depth: 4,
        no_publisher_warning_after_nanos: warn_after_nanos,
        qos: crate::common::test_qos(),
    };
    let mut subscriber = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_subscribe_typed(
                node,
                topic_c.as_ptr(),
                topic_c.as_bytes().len(),
                data_type.as_ptr(),
                data_type.as_bytes().len(),
                md5.as_ptr(),
                md5.as_bytes().len(),
                std::ptr::null(),
                0,
                &raw const qos,
                &raw mut subscriber,
            )
        },
        HyphaStatus::Ok
    );
    let record = attach::subscription(subscriber, Some(ignore_delivery), ptr::null_mut(), None);
    let mut attachment: HyphaAttachmentId = 0;
    assert_eq!(
        unsafe { hypha_waitset_attach(waitset, &raw const record, &raw mut attachment) },
        HyphaStatus::Ok
    );
    Fixture { log_file: dir.join("warning.log"), topic: topic.to_owned(), node, waitset }
}

fn run_for(waitset: *mut HyphaWaitSet, nanos: u64) {
    let mut report =
        HyphaWaitSetRunReport { reason: 0, status: 0, attachment: 0, slow_callbacks: 0 };
    let status = unsafe { hypha_waitset_run(waitset, nanos, &raw mut report) };
    assert_eq!(status, HyphaStatus::Ok);
}

fn warning_lines(fixture: &Fixture) -> Vec<String> {
    let text = std::fs::read_to_string(&fixture.log_file).unwrap_or_default();
    text.lines().filter(|line| line.contains(WARNING_MARKER)).map(str::to_owned).collect()
}

#[test]
fn the_waitset_warns_once_naming_the_topic_and_the_domain() {
    let fixture = fixture("warns_once", 100_000_000);
    run_for(fixture.waitset, 600_000_000);
    let lines = warning_lines(&fixture);
    assert_eq!(lines.len(), 1, "log lines: {lines:?}");
    let line = lines.first().expect("one warning");
    assert!(line.contains(&format!("topic '{}'", fixture.topic)), "{line}");
    assert!(line.contains("in domain '"), "{line}");
    assert!(line.contains("after 100 ms"), "{line}");

    run_for(fixture.waitset, 400_000_000);
    assert_eq!(warning_lines(&fixture).len(), 1, "a later turn repeats the warning");
    unsafe {
        hypha_node_destroy(fixture.node);
    }
}

#[test]
fn the_waitset_does_not_warn_before_the_delay_has_elapsed() {
    let fixture = fixture("warns_late", 60_000_000_000);
    run_for(fixture.waitset, 400_000_000);
    assert_eq!(warning_lines(&fixture).len(), 0);
    unsafe {
        hypha_node_destroy(fixture.node);
    }
}

#[test]
fn a_zero_delay_disables_the_warning() {
    let fixture = fixture("warns_off", 0);
    run_for(fixture.waitset, 600_000_000);
    assert_eq!(warning_lines(&fixture).len(), 0);
    unsafe {
        hypha_node_destroy(fixture.node);
    }
}

#[test]
fn a_wildcard_subscription_never_warns_about_a_missing_publisher() {
    let pattern = format!("/{}/**", unique("no_publisher_wild"));
    let fixture = fixture_on("wildcard_quiet", &pattern, 100_000_000);
    run_for(fixture.waitset, 600_000_000);
    assert!(warning_lines(&fixture).is_empty(), "{:?}", warning_lines(&fixture));
    unsafe {
        hypha_node_destroy(fixture.node);
    }
}
