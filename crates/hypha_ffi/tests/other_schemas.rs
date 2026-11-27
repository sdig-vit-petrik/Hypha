//! What a subscriber says and shows about the other versions of its topic (#209).
//! Each test needs its own process (nextest).
#![allow(clippy::expect_used, clippy::print_stdout)]

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
    subscriber: *mut HyphaSubscriber,
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
    Fixture {
        log_file: dir.join("warning.log"),
        topic: topic.to_owned(),
        node,
        waitset,
        subscriber,
    }
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

/// The subscriber's count of the other schemas published on its topic.
fn other_schemas(fixture: &Fixture) -> u64 {
    let mut counters = HyphaSubscriberCounters::default();
    let status = unsafe { hypha_subscriber_counters(fixture.subscriber, &raw mut counters) };
    assert_eq!(status, HyphaStatus::Ok);
    counters.other_schemas
}

/// The detection FSR-05 asks for: a publisher built from a stale definition, beside an
/// up-to-date one. Its frames are on another stream and reach no reader of this version,
/// the subscriber's own stream has a publisher so nothing is logged, and the subscriber's
/// counters show the other version.
#[test]
fn a_stale_publisher_beside_a_current_one_is_shown_in_the_counters() {
    let fixture = fixture("stale_beside", 100_000_000);
    let current = publish(&fixture, DATA_TYPE, MD5, "uint32 value\n");
    let (md5, definition) = VERSION_2;
    let stale = publish(&fixture, DATA_TYPE, md5, definition);
    run_for(fixture.waitset, 1_500_000_000);
    let text = std::fs::read_to_string(&fixture.log_file).unwrap_or_default();
    let about_topic: Vec<&str> = text
        .lines()
        .filter(|line| line.contains(&fixture.topic))
        .filter(|line| line.contains("[WARN]") || line.contains("[ERROR]"))
        .collect();
    assert!(about_topic.is_empty(), "{about_topic:?}");
    assert_eq!(other_schemas(&fixture), 1, "the stale version is shown");
    unsafe { hypha_publisher_destroy(stale) };
    run_for(fixture.waitset, 1_500_000_000);
    assert_eq!(other_schemas(&fixture), 0, "and is gone when it leaves");
    unsafe {
        hypha_publisher_destroy(current);
        hypha_node_destroy(fixture.node);
    }
}

/// Another version that is published without a definition still marks its stream, so the
/// warning names it, not the container hint.
#[test]
fn a_version_published_without_a_definition_is_named() {
    let fixture = fixture("no_definition", 100_000_000);
    let (md5, _) = VERSION_2;
    let bare = publish(&fixture, DATA_TYPE, md5, "");
    run_for(fixture.waitset, 600_000_000);
    let plain = warning_lines(&fixture);
    let named = lines_with(&fixture, OTHERS_MARKER);
    assert!(plain.is_empty(), "{plain:?}");
    assert_eq!(named.len(), 1, "{named:?}");
    assert!(
        named
            .first()
            .is_some_and(|line| line.contains("published are std_msgs/Counter (f79835de)"))
    );
    assert_eq!(other_schemas(&fixture), 1);
    unsafe {
        hypha_publisher_destroy(bare);
        hypha_node_destroy(fixture.node);
    }
}

/// Another version that comes and goes while the subscriber has no publisher of its own:
/// at most one line in ten seconds, whatever the number of changes.
#[test]
fn a_flapping_other_version_is_said_at_most_once_in_ten_seconds() {
    let fixture = fixture("flapping", 100_000_000);
    let (md5, definition) = VERSION_2;
    for _ in 0..4 {
        let other = publish(&fixture, DATA_TYPE, md5, definition);
        run_for(fixture.waitset, 1_100_000_000);
        unsafe { hypha_publisher_destroy(other) };
        run_for(fixture.waitset, 1_100_000_000);
    }
    let named = lines_with(&fixture, OTHERS_MARKER).len();
    let plain = warning_lines(&fixture).len();
    assert_eq!(named + plain, 1, "named {named}, plain {plain}");
    unsafe {
        hypha_node_destroy(fixture.node);
    }
}

/// The peer of the cross-process test: it publishes version 2 on `HYPHA_REVIEW_TOPIC` for
/// `HYPHA_REVIEW_MILLIS` milliseconds. It does nothing when it runs on its own.
#[test]
#[ignore = "the peer process of the_warning_names_a_version_published_by_another_process"]
fn review_peer_publishes_version_2() {
    let Ok(topic) = std::env::var("HYPHA_REVIEW_TOPIC") else { return };
    let millis = std::env::var("HYPHA_REVIEW_MILLIS")
        .ok()
        .and_then(|text| text.parse().ok())
        .unwrap_or(3000);
    let node = create_node_with_file(&unique("review_peer"), None);
    let fixture = Fixture {
        log_file: PathBuf::new(),
        topic,
        node,
        waitset: ptr::null_mut(),
        subscriber: ptr::null_mut(),
    };
    let (md5, definition) = VERSION_2;
    let publisher = publish(&fixture, DATA_TYPE, md5, definition);
    std::thread::sleep(std::time::Duration::from_millis(millis));
    unsafe {
        hypha_publisher_destroy(publisher);
        hypha_node_destroy(node);
    }
}

/// The warning and the count across processes: version 2 is published by another process.
#[test]
fn the_warning_names_a_version_published_by_another_process() {
    let fixture = fixture("across", 100_000_000);
    let exe = std::env::current_exe().expect("the test binary");
    let mut peer = std::process::Command::new(exe)
        .args(["--ignored", "--exact", "review_peer_publishes_version_2"])
        .env("HYPHA_REVIEW_TOPIC", &fixture.topic)
        .env("HYPHA_REVIEW_MILLIS", "4000")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("the peer process");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while lines_with(&fixture, OTHERS_MARKER).is_empty() && std::time::Instant::now() < deadline {
        run_for(fixture.waitset, 200_000_000);
    }
    let named = lines_with(&fixture, OTHERS_MARKER);
    let count = other_schemas(&fixture);
    let _ = peer.kill();
    let _ = peer.wait();
    assert_eq!(named.len(), 1, "{named:?}");
    assert!(
        named
            .first()
            .is_some_and(|line| line.contains("published are std_msgs/Counter (f79835de)"))
    );
    assert_eq!(count, 1);
    unsafe {
        hypha_node_destroy(fixture.node);
    }
}
