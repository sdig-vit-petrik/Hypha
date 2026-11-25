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

#[test]
#[expect(clippy::float_cmp, reason = "whole numbers below 2^53 add and compare exactly")]
fn the_rated_workload_has_the_totals_of_nfr_04_and_the_limits_of_nfr_03_and_nfr_04() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/load/rated.toml");
    let text = std::fs::read_to_string(&path).expect("examples/load/rated.toml");
    let workload = Workload::from_toml(&text).expect("the rated workload parses");

    assert_eq!(workload.topics.len(), 46);
    // Every rate and size is a whole number well below 2^53, so the sums are exact.
    let rated: Vec<(f64, f64)> = workload
        .topics
        .iter()
        .filter_map(|topic| match topic.rate_hz {
            Rate::Hz(rate) => {
                let size = u32::try_from(topic.bytes).expect("a size of the rated load");
                Some((rate, rate * f64::from(size)))
            }
            Rate::Max(_) => None,
        })
        .collect();
    assert_eq!(rated.len(), 46, "every topic has a rate");
    let messages: f64 = rated.iter().map(|(rate, _)| rate).sum();
    let bytes: f64 = rated.iter().map(|(_, bytes)| bytes).sum();
    assert_eq!(messages, 9_960.0, "messages per second");
    assert_eq!(bytes, 72_500_000.0, "bytes per second: 72.5 MB/s, MB = 10^6 bytes");

    let keeps_pace = workload
        .topics
        .iter()
        .flat_map(|topic| &topic.subscribers)
        .all(|subscriber| subscriber.work_us == 0 && subscriber.limits.is_none());
    assert!(keeps_pace, "every reader keeps pace and takes the run's limits");
    assert!(!workload.recorder_enabled(), "NFR-04 names no recorder");
    let limits = workload.limits;
    assert_eq!(limits.latency_p99_us, Some(500), "NFR-03");
    assert_eq!(limits.max_loss, Some(0), "NFR-04");
    assert_eq!(limits.loss_accounted, Some(true));
    assert_eq!(limits.max_refused_publishes, Some(0));
    assert_eq!(limits.max_missed_periods, Some(0), "the rated load was offered");
    assert_eq!(workload.run.duration_s, 60.0);
}

fn example(name: &str) -> Workload {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/load").join(name);
    let text = std::fs::read_to_string(&path).expect("an example workload");
    Workload::from_toml(&text).expect("the example workload parses")
}

#[test]
fn the_overload_workload_bounds_memory_counts_every_loss_and_isolates_the_control_topics() {
    let workload = example("overload.toml");
    assert!(workload.run.bounded_memory, "a bounded deployment, the condition of NFR-05");
    assert!(!workload.recorder_enabled());
    let limits = workload.limits;
    assert_eq!(limits.max_refused_publishes, Some(0), "a slow reader refuses no publish");
    assert_eq!(limits.max_rss_growth_bytes, Some(1_048_576));
    assert_eq!(limits.max_shm_growth_bytes, Some(1_048_576));
    assert_eq!(limits.latency_p99_us, None, "a latency is no fact of NFR-05");
    assert_eq!(limits.max_publish_call_us, None, "a call's duration is a speed");
    assert_eq!(limits.max_missed_periods, None);

    for topic in &workload.topics {
        for subscriber in &topic.subscribers {
            let own = subscriber.limits.unwrap_or_else(|| limits.subscriber_part());
            assert_eq!(own.loss_accounted, Some(true), "{}@{}", topic.name, subscriber.process);
            assert_eq!(own.latency_p99_us, None, "{}@{}", topic.name, subscriber.process);
        }
    }

    // The control topics share no process with the flood, its publisher or its readers.
    let is_control = |name: &str| name == "/overload/control" || name == "/overload/state";
    let mut flooding = std::collections::BTreeSet::new();
    let mut control = std::collections::BTreeSet::new();
    for topic in &workload.topics {
        let processes = std::iter::once(topic.publisher.as_str())
            .chain(topic.subscribers.iter().map(|subscriber| subscriber.process.as_str()));
        if is_control(&topic.name) {
            control.extend(processes);
            for subscriber in &topic.subscribers {
                let own = subscriber.limits.expect("a control reader has its own limits");
                assert_eq!(own.max_loss, Some(0), "{} keeps pace", topic.name);
                assert_eq!(subscriber.work_us, 0);
            }
        } else {
            flooding.extend(processes);
        }
    }
    assert_eq!(control.len(), 2, "{control:?}");
    assert!(flooding.is_disjoint(&control), "{flooding:?} and {control:?}");
    assert!(workload.topics.iter().any(|topic| matches!(topic.rate_hz, Rate::Max(_))));
}

#[test]
fn the_isolation_latency_workload_is_the_overload_one_with_control_latency_limits() {
    let overload = example("overload.toml");
    let mut latency = example("isolation-latency.toml");
    for topic in &mut latency.topics {
        for subscriber in &mut topic.subscribers {
            if let Some(own) = subscriber.limits.as_mut() {
                assert_eq!(own.latency_p99_us, Some(500), "{}", topic.name);
                own.latency_p99_us = None;
            }
        }
    }
    assert_eq!(latency, overload, "the two files differ only in the control latency limits");
}
