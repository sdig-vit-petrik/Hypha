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
/// The start of the warning that names the other schemas published on the topic.
const OTHERS_MARKER: &str = "has publishers, none of type";

/// Version 2 of `DATA_TYPE`: the same type name, another definition and MD5.
const VERSION_2: (&str, &str) =
    ("f79835def461350458ead76f07016dd7", "int32 value\nstring label\nfloat64 weight\n");

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
    lines_with(fixture, WARNING_MARKER)
}

fn lines_with(fixture: &Fixture, marker: &str) -> Vec<String> {
    let text = std::fs::read_to_string(&fixture.log_file).unwrap_or_default();
    text.lines().filter(|line| line.contains(marker)).map(str::to_owned).collect()
}

/// A publisher on the fixture's topic of `data_type` with `md5` and `definition`.
fn publish(fixture: &Fixture, data_type: &str, md5: &str, definition: &str) -> *mut HyphaPublisher {
    let topic = c_text(&fixture.topic);
    let data_type = c_text(data_type);
    let md5 = c_text(md5);
    let definition = c_text(definition);
    let options =
        HyphaPublisherOptions { latch: 0, loaned_samples: 0, qos: crate::common::test_qos() };
    let mut publisher = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_publish_with_qos(
                fixture.node,
                topic.as_ptr(),
                topic.as_bytes().len(),
                data_type.as_ptr(),
                data_type.as_bytes().len(),
                md5.as_ptr(),
                md5.as_bytes().len(),
                definition.as_ptr(),
                definition.as_bytes().len(),
                &raw const options,
                &raw mut publisher,
            )
        },
        HyphaStatus::Ok
    );
    publisher
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

#[test]
fn a_subscriber_whose_version_nobody_publishes_names_the_versions_that_are() {
    let fixture = fixture("other_version", 100_000_000);
    let (md5, definition) = VERSION_2;
    let version_2 = publish(&fixture, DATA_TYPE, md5, definition);
    run_for(fixture.waitset, 600_000_000);

    let lines = lines_with(&fixture, OTHERS_MARKER);
    assert_eq!(lines.len(), 1, "{lines:?}");
    let expected = format!(
        "hypha: topic '{}' has publishers, none of type std_msgs/Counter (d9d213e8): published \
         are std_msgs/Counter (f79835de)",
        fixture.topic
    );
    assert!(lines.first().is_some_and(|line| line.contains(&expected)), "{lines:?}");
    assert!(lines.first().is_some_and(|line| line.contains("[WARN]")), "{lines:?}");
    assert!(warning_lines(&fixture).is_empty(), "the other version is a publisher of the topic");

    run_for(fixture.waitset, 400_000_000);
    assert_eq!(lines_with(&fixture, OTHERS_MARKER).len(), 1, "the same absence is said once");
    unsafe {
        hypha_publisher_destroy(version_2);
        hypha_node_destroy(fixture.node);
    }
}

#[test]
fn the_warning_is_said_again_when_the_published_schemas_change() {
    let fixture = fixture("others_change", 100_000_000);
    let (md5, definition) = VERSION_2;
    let version_2 = publish(&fixture, DATA_TYPE, md5, definition);
    run_for(fixture.waitset, 600_000_000);
    assert_eq!(lines_with(&fixture, OTHERS_MARKER).len(), 1);

    let other = publish(
        &fixture,
        "std_msgs/Target",
        "c2f7134c29a3ad4daf56996f7fc77409",
        "float64 target\n",
    );
    run_for(fixture.waitset, 1_500_000_000);
    assert_eq!(lines_with(&fixture, OTHERS_MARKER).len(), 1, "at most one line in ten seconds");
    run_for(fixture.waitset, 9_500_000_000);
    let lines = lines_with(&fixture, OTHERS_MARKER);
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(
        lines.last().is_some_and(|line| line
            .contains("published are std_msgs/Counter (f79835de), std_msgs/Target (c2f7134c)")),
        "{lines:?}"
    );

    unsafe {
        hypha_publisher_destroy(version_2);
        hypha_publisher_destroy(other);
    }
    run_for(fixture.waitset, 11_000_000_000);
    assert_eq!(warning_lines(&fixture).len(), 1, "no other schema is left: the plain warning");
    unsafe {
        hypha_node_destroy(fixture.node);
    }
}

#[test]
fn a_subscriber_whose_publisher_leaves_is_told_again() {
    let fixture = fixture("publisher_leaves", 100_000_000);
    let version_1 = publish(&fixture, DATA_TYPE, MD5, "uint32 value\n");
    run_for(fixture.waitset, 600_000_000);
    assert!(lines_with(&fixture, "hypha: ").iter().all(|line| !line.contains(&fixture.topic)));

    let (md5, definition) = VERSION_2;
    let version_2 = publish(&fixture, DATA_TYPE, md5, definition);
    unsafe {
        hypha_publisher_destroy(version_1);
    }
    run_for(fixture.waitset, 1_500_000_000);
    let lines = lines_with(&fixture, OTHERS_MARKER);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(
        lines
            .first()
            .is_some_and(|line| line.contains("published are std_msgs/Counter (f79835de)"))
    );
    unsafe {
        hypha_publisher_destroy(version_2);
        hypha_node_destroy(fixture.node);
    }
}
