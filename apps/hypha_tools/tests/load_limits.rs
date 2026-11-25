//! The pass limits of `hypha load`, checked as a pure function of a report.

#![expect(clippy::expect_used, reason = "a test fails on the first refused step")]

use hypha_tools::load::histogram::HistogramSummary;
use hypha_tools::load::limits::evaluate;
use hypha_tools::load::protocol::{PublisherResult, SubscriberResult};
use hypha_tools::load::report::{
    Config, DomainMemory, Host, LimitResult, Outcome, ProcessReport, RecordedTopic, RecorderReport,
    Report, SCHEMA, SeriesReport, SubscriberReport, TopicReport,
};
use hypha_tools::load::workload::{MaxRate, Rate, Workload};

const WORKLOAD: &str = r#"
[run]
duration_s = 1.0
warmup_s = 0.0
cooldown_s = 0.0
sample_interval_s = 1.0
bounded_memory = false

[[process]]
name = "talker"

[[process]]
name = "listener"

[[process]]
name = "slow"

[[topic]]
name = "/t"
bytes = 64
rate_hz = 100
publisher = "talker"
subscribers = [
    { process = "listener" },
    { process = "slow", limits = { loss_accounted = true } },
]

[limits]
latency_p99_us = 500
max_interval_latency_p99_us = 900
max_loss = 0
loss_accounted = true
max_refused_publishes = 0
max_publish_call_us = 50
max_missed_periods = 0
max_rss_growth_bytes = 1000
max_shm_growth_bytes = 1000
"#;

fn latency(p99_ns: u64) -> HistogramSummary {
    HistogramSummary { count: 100, p99_ns: Some(p99_ns), ..HistogramSummary::default() }
}

fn received(p99_ns: u64, received_window: u64, missing: u64, gaps: u64) -> SubscriberResult {
    SubscriberResult {
        topic: "/t".into(),
        received_window,
        received_total: received_window,
        missing_sequences: missing,
        harness_gaps: gaps,
        latency: latency(p99_ns),
        worst_interval_p99_ns: Some(p99_ns),
        ..SubscriberResult::default()
    }
}

/// A report of a run that meets every limit of [`WORKLOAD`].
fn passing_report() -> Report {
    let workload = Workload::from_toml(WORKLOAD).expect("the workload");
    let publish = PublisherResult {
        topic: "/t".into(),
        sent_window: 100,
        sent_total: 100,
        publish_call: HistogramSummary { max_ns: Some(50_000), ..HistogramSummary::default() },
        missed_periods: Some(0),
        ..PublisherResult::default()
    };
    let process = |name: &str| ProcessReport {
        name: name.into(),
        rss_at_window_start: Some(10_000),
        rss_peak: Some(11_000),
        ..ProcessReport::default()
    };
    Report {
        schema: SCHEMA.into(),
        outcome: Outcome::Pass,
        reason: None,
        host: Host::default(),
        config: Config {
            workload_path: "w.toml".into(),
            workload_md5: String::new(),
            workload,
            domain: "load_1".into(),
            started_unix_ns: 0,
        },
        not_measured: Vec::new(),
        topics: vec![TopicReport {
            name: "/t".into(),
            bytes: 64,
            rate_hz: Some(100.0),
            publisher: "talker".into(),
            publish: Some(publish),
            subscribers: vec![
                SubscriberReport {
                    process: "listener".into(),
                    result: Some(received(500_000, 100, 0, 0)),
                },
                // A slow reader that lost 60 frames and counted every one.
                SubscriberReport {
                    process: "slow".into(),
                    result: Some(received(9_000_000, 40, 60, 60)),
                },
            ],
        }],
        processes: vec![process("talker"), process("listener"), process("slow")],
        domain_shared_memory: DomainMemory { at_window_start: Some(4096), peak: Some(5096) },
        recorder: None,
        series: SeriesReport::default(),
        limits: Vec::new(),
    }
}

fn failed(results: &[LimitResult]) -> Vec<(String, String)> {
    results
        .iter()
        .filter(|limit| !limit.pass)
        .map(|limit| (limit.key.clone(), limit.scope.clone()))
        .collect()
}

fn failed_of(report: &Report) -> Vec<(String, String)> {
    failed(&evaluate(report))
}

fn pair(key: &str, scope: &str) -> (String, String) {
    (key.to_owned(), scope.to_owned())
}

#[test]
fn a_run_on_its_limits_passes_and_every_limit_is_listed() {
    let results = evaluate(&passing_report());
    assert_eq!(failed(&results), Vec::<(String, String)>::new(), "{results:?}");
    // Four subscriber limits for the listener, one for the slow reader, three for the
    // publisher, one per process and one for the domain.
    assert_eq!(results.len(), 4 + 1 + 3 + 3 + 1, "{results:?}");
}

#[test]
fn a_subscriber_with_its_own_limits_is_checked_only_on_those() {
    let results = evaluate(&passing_report());
    let slow: Vec<&str> = results
        .iter()
        .filter(|limit| limit.scope == "/t@slow")
        .map(|limit| limit.key.as_str())
        .collect();
    assert_eq!(slow, vec!["loss_accounted"]);
}

#[test]
fn each_subscriber_limit_fails_on_its_own_value() {
    let mut report = passing_report();
    let listener = report
        .topics
        .first_mut()
        .and_then(|topic| topic.subscribers.first_mut())
        .and_then(|subscriber| subscriber.result.as_mut())
        .expect("the listener");
    listener.latency.p99_ns = Some(500_001);
    listener.worst_interval_p99_ns = Some(900_001);
    listener.missing_sequences = 1;
    assert_eq!(
        failed_of(&report),
        vec![
            pair("latency_p99_us", "/t@listener"),
            pair("max_interval_latency_p99_us", "/t@listener"),
            pair("max_loss", "/t@listener"),
            pair("loss_accounted", "/t@listener"),
        ]
    );
}

#[test]
fn a_frame_sent_and_never_received_is_a_loss_without_a_gap() {
    let mut report = passing_report();
    let listener = report
        .topics
        .first_mut()
        .and_then(|topic| topic.subscribers.first_mut())
        .and_then(|subscriber| subscriber.result.as_mut())
        .expect("the listener");
    listener.received_window = 99;
    let results = evaluate(&report);
    let loss = results
        .iter()
        .find(|limit| limit.key == "max_loss" && limit.scope == "/t@listener")
        .expect("the loss limit");
    assert_eq!((loss.measured, loss.pass), (Some(1), false));
}

#[test]
fn a_loss_the_harness_and_the_framework_count_differently_is_not_accounted() {
    let mut report = passing_report();
    let slow = report
        .topics
        .first_mut()
        .and_then(|topic| topic.subscribers.get_mut(1))
        .and_then(|subscriber| subscriber.result.as_mut())
        .expect("the slow reader");
    slow.harness_gaps = 59;
    assert_eq!(failed_of(&report), vec![pair("loss_accounted", "/t@slow")]);
}

#[test]
fn each_publisher_and_memory_limit_fails_on_its_own_value() {
    let mut report = passing_report();
    let publish =
        report.topics.first_mut().and_then(|topic| topic.publish.as_mut()).expect("publisher");
    publish.refused = 1;
    publish.missed_periods = Some(1);
    publish.publish_call.max_ns = Some(50_001);
    if let Some(process) = report.processes.get_mut(1) {
        process.rss_peak = Some(11_001);
    }
    report.domain_shared_memory.peak = Some(5097);
    assert_eq!(
        failed_of(&report),
        vec![
            pair("max_refused_publishes", "/t"),
            pair("max_publish_call_us", "/t"),
            pair("max_missed_periods", "/t"),
            pair("max_rss_growth_bytes", "listener"),
            pair("max_shm_growth_bytes", "domain"),
        ]
    );
}

#[test]
fn a_limit_whose_value_was_not_measured_fails() {
    let mut report = passing_report();
    report.domain_shared_memory = DomainMemory::default();
    if let Some(topic) = report.topics.first_mut() {
        topic.publish = None;
        if let Some(subscriber) = topic.subscribers.first_mut() {
            subscriber.result = None;
        }
    }
    let results = evaluate(&report);
    let shm = results.iter().find(|limit| limit.key == "max_shm_growth_bytes").expect("shm");
    assert_eq!((shm.measured, shm.pass), (None, false));
    assert!(failed(&results).contains(&pair("latency_p99_us", "/t@listener")));
    assert!(failed(&results).contains(&pair("max_refused_publishes", "/t")));
}

#[test]
fn an_entry_the_report_lacks_is_not_measured_and_fails() {
    // A truncated or hand-edited report: the slow reader, a process and, in a second
    // report, the whole topic are gone. Every limit on them fails rather than passes.
    let mut report = passing_report();
    if let Some(topic) = report.topics.first_mut() {
        topic.subscribers.retain(|subscriber| subscriber.process != "slow");
    }
    report.processes.retain(|process| process.name != "listener");
    let lacking = failed_of(&report);
    assert!(lacking.contains(&pair("loss_accounted", "/t@slow")), "{lacking:?}");
    assert!(lacking.contains(&pair("max_rss_growth_bytes", "listener")), "{lacking:?}");

    let mut report = passing_report();
    report.topics.clear();
    let results = evaluate(&report);
    assert_eq!(results.len(), 12, "every limit is still listed: {results:?}");
    let lacking = failed(&results);
    for key in ["latency_p99_us", "max_loss", "loss_accounted"] {
        assert!(lacking.contains(&pair(key, "/t@listener")), "{key}: {lacking:?}");
    }
    assert!(lacking.contains(&pair("max_refused_publishes", "/t")), "{lacking:?}");
}

#[test]
fn a_reordered_or_malformed_probe_is_an_unaccounted_loss() {
    for field in ["reordered", "malformed"] {
        let mut report = passing_report();
        let slow = report
            .topics
            .first_mut()
            .and_then(|topic| topic.subscribers.get_mut(1))
            .and_then(|subscriber| subscriber.result.as_mut())
            .expect("the slow reader");
        if field == "reordered" {
            slow.reordered = 1;
        } else {
            slow.malformed = 1;
        }
        let results = evaluate(&report);
        let accounted = results.iter().find(|limit| limit.scope == "/t@slow").expect("limit");
        assert_eq!((accounted.limit, accounted.measured, accounted.pass), (0, Some(1), false));
    }
}

#[test]
fn the_unrecorded_limit_needs_the_recorder() {
    let mut report = passing_report();
    report.config.workload.limits.max_unrecorded = Some(0);
    let failed = failed_of(&report);
    assert!(failed.contains(&pair("max_unrecorded", "/t")), "no recorder, not measured");

    report.recorder = Some(RecorderReport {
        output: "r.mcap".into(),
        exit_code: Some(1),
        topics: vec![RecordedTopic { topic: "/t".into(), recorded: 99, unrecorded: 1 }],
    });
    let results = evaluate(&report);
    let unrecorded = results.iter().find(|limit| limit.key == "max_unrecorded").expect("limit");
    assert_eq!((unrecorded.measured, unrecorded.pass), (Some(1), false));
    report.config.workload.limits.max_unrecorded = Some(1);
    assert!(failed_of(&report).is_empty());
}

#[test]
fn a_topic_sent_back_to_back_has_no_missed_period_limit() {
    let mut report = passing_report();
    if let Some(topic) = report.config.workload.topics.first_mut() {
        topic.rate_hz = Rate::Max(MaxRate::Max);
    }
    let results = evaluate(&report);
    assert!(results.iter().all(|limit| limit.key != "max_missed_periods"), "{results:?}");
}
