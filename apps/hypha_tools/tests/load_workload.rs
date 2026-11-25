//! The workload file of `hypha load`: what parses, and each refusal with its reason.

#![expect(clippy::expect_used, reason = "a test fails on the first refused step")]

use std::path::Path;

use hypha_tools::load::workload::{Rate, Workload, WorkloadError};

const VALID: &str = r#"
[run]
duration_s = 1.0
warmup_s = 0.5
cooldown_s = 0.2
sample_interval_s = 0.5
bounded_memory = false

[[process]]
name = "pub_a"

[[process]]
name = "sub_b"

[[topic]]
name = "/load/a"
bytes = 64
rate_hz = 20
publisher = "pub_a"
burst = { count = 2, every_s = 0.5 }
subscribers = [{ process = "sub_b", depth = 4, work_us = 10, limits = { max_loss = 0 } }]

[[topic]]
name = "/load/flood"
bytes = 20
rate_hz = "max"
publisher = "pub_a"
subscribers = [{ process = "sub_b" }]

[limits]
latency_p99_us = 500
max_interval_latency_p99_us = 800
max_refused_publishes = 0
"#;

fn refusal(text: &str) -> WorkloadError {
    Workload::from_toml(text).expect_err("the workload must be refused")
}

#[test]
fn a_full_workload_parses_with_its_defaults() {
    let workload = Workload::from_toml(VALID).expect("a valid workload");
    assert_eq!(workload.processes.len(), 2);
    let flood = workload.topics.get(1).expect("the flood topic");
    assert!(matches!(flood.rate_hz, Rate::Max(_)));
    let subscriber = flood.subscribers.first().expect("a subscriber");
    assert_eq!((subscriber.depth, subscriber.work_us, subscriber.limits), (0, 0, None));
    assert_eq!(workload.topics.first().map(|topic| topic.rate_hz), Some(Rate::Hz(20.0)));
    assert!(!workload.recorder_enabled());
    assert_eq!(workload.limits.latency_p99_us, Some(500));
    assert_eq!(workload.limits.subscriber_part().max_interval_latency_p99_us, Some(800));
}

#[test]
fn the_example_workloads_parse() {
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/load");
    let workloads: Vec<_> = std::fs::read_dir(&examples)
        .expect("examples/load")
        .map(|entry| entry.expect("an entry").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "toml"))
        .collect();
    for path in &workloads {
        let text = std::fs::read_to_string(path).expect("read the example");
        let parsed = Workload::from_toml(&text);
        assert!(parsed.is_ok(), "{}: {parsed:?}", path.display());
    }
    assert!(!workloads.is_empty(), "examples/load holds a workload");
}

#[test]
fn a_missing_required_key_is_named() {
    let text = VALID.replace("cooldown_s = 0.2\n", "");
    let error = refusal(&text);
    assert!(matches!(error, WorkloadError::Toml(_)), "{error:?}");
    assert!(error.to_string().contains("cooldown_s"), "{error}");
}

#[test]
fn an_unknown_key_is_refused() {
    let error =
        refusal(&VALID.replace("bounded_memory = false", "bounded_memory = false\nspeed = 1"));
    assert!(error.to_string().contains("speed"), "{error}");
    let error = refusal(&VALID.replace("max_loss = 0 }", "max_refused_publishes = 0 }"));
    assert!(
        error.to_string().contains("max_refused_publishes"),
        "a run limit in a subscriber: {error}"
    );
}

#[test]
fn a_process_the_workload_does_not_declare_is_refused() {
    let error =
        refusal(&VALID.replace("publisher = \"pub_a\"\nburst", "publisher = \"nobody\"\nburst"));
    assert_eq!(
        error,
        WorkloadError::UnknownProcess { topic: "/load/a".into(), process: "nobody".into() }
    );
}

#[test]
fn duplicates_and_invalid_names_are_refused() {
    let twice = VALID.replace("name = \"/load/flood\"", "name = \"/load/a\"");
    assert_eq!(refusal(&twice), WorkloadError::DuplicateTopic("/load/a".into()));
    let process = VALID.replace("name = \"sub_b\"", "name = \"pub_a\"");
    assert_eq!(refusal(&process), WorkloadError::DuplicateProcess("pub_a".into()));
    let name = VALID.replace("name = \"sub_b\"", "name = \"Sub-B\"");
    assert_eq!(refusal(&name), WorkloadError::InvalidProcessName("Sub-B".into()));
    let topic = VALID.replace("name = \"/load/flood\"", "name = \"/load/*\"");
    assert_eq!(refusal(&topic), WorkloadError::InvalidTopicName("/load/*".into()));
    let double = VALID.replace(
        "subscribers = [{ process = \"sub_b\" }]",
        "subscribers = [{ process = \"sub_b\" }, { process = \"sub_b\" }]",
    );
    assert!(matches!(refusal(&double), WorkloadError::DuplicateSubscriber { .. }));
}

#[test]
fn a_process_with_nothing_to_do_is_refused() {
    let text = VALID.replace(
        "[[process]]\nname = \"sub_b\"",
        "[[process]]\nname = \"sub_b\"\n\n[[process]]\nname = \"idle\"",
    );
    assert_eq!(refusal(&text), WorkloadError::IdleProcess("idle".into()));
}

#[test]
fn a_message_smaller_than_the_probe_header_is_refused() {
    let text = VALID.replace("bytes = 20", "bytes = 19");
    assert_eq!(
        refusal(&text),
        WorkloadError::PayloadTooSmall { topic: "/load/flood".into(), bytes: 19 }
    );
}

#[test]
fn numbers_out_of_range_are_refused() {
    for (from, to, key) in [
        ("duration_s = 1.0", "duration_s = 0.0", "run.duration_s"),
        ("duration_s = 1.0", "duration_s = nan", "run.duration_s"),
        ("warmup_s = 0.5", "warmup_s = -1.0", "run.warmup_s"),
        ("sample_interval_s = 0.5", "sample_interval_s = inf", "run.sample_interval_s"),
        ("rate_hz = 20", "rate_hz = 0", "rate_hz"),
        ("rate_hz = 20", "rate_hz = 2e9", "rate_hz"),
        ("count = 2", "count = 0", "burst.count"),
        ("every_s = 0.5", "every_s = 0.0", "burst.every_s"),
    ] {
        let error = refusal(&VALID.replace(from, to));
        assert!(matches!(error, WorkloadError::OutOfRange { .. }), "{to}: {error:?}");
        assert!(error.to_string().contains(key), "{to}: {error}");
    }
    let word = refusal(&VALID.replace("rate_hz = \"max\"", "rate_hz = \"fast\""));
    assert!(matches!(word, WorkloadError::Toml(_)), "{word:?}");
}

#[test]
fn more_subscribers_than_a_topic_admits_are_refused_with_the_recorder_counted() {
    let three = VALID.replace(
        "subscribers = [{ process = \"sub_b\" }]",
        "subscribers = [{ process = \"sub_b\" }, { process = \"pub_a\" }, { process = \"sub_c\" }]",
    ) + "\n[[process]]\nname = \"sub_c\"\n";
    Workload::from_toml(&three).expect("three subscribers fit the three slots");

    let recorded = three.clone() + "\n[recorder]\nenabled = true\n";
    let error = refusal(&recorded);
    assert_eq!(
        error,
        WorkloadError::TooManySubscribers {
            topic: "/load/flood".into(),
            subscribers: 3,
            recorder: true
        }
    );
    assert!(error.to_string().contains("the recorder"), "{error}");
}

#[test]
fn an_unknown_compression_is_refused() {
    let text = VALID.to_owned() + "\n[recorder]\nenabled = true\ncompression = \"gzip\"\n";
    assert_eq!(refusal(&text), WorkloadError::UnknownCompression("gzip".into()));
}

#[test]
fn a_process_named_as_the_recorder_is_refused() {
    let text = VALID
        .replace("name = \"sub_b\"", "name = \"recorder\"")
        .replace("process = \"sub_b\"", "process = \"recorder\"");
    assert_eq!(refusal(&text), WorkloadError::ReservedProcessName);
}

#[test]
fn a_slow_reader_that_would_hold_the_heartbeat_is_refused_with_both_numbers() {
    // 32 queued samples (depth 0 is the service's 32) of 400 ms work are 12.8 s, above
    // the 10 s heartbeat bound of a 0.5 s sample interval.
    let text = VALID.replace(
        "subscribers = [{ process = \"sub_b\" }]",
        "subscribers = [{ process = \"sub_b\", work_us = 400000 }]",
    );
    let error = refusal(&text);
    assert_eq!(
        error,
        WorkloadError::StarvedHeartbeat {
            process: "sub_b".into(),
            work_us: 32 * 400_000 + 4 * 10,
            bound_us: 10_000_000,
        }
    );
    let message = error.to_string();
    assert!(message.contains("12800040") && message.contains("10000000"), "{message}");
}
