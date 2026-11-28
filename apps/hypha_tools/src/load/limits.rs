//! The pass limits of a workload, checked against a finished report: a pure function
//! of the report, so a stored report can be checked again.
//!
//! A limit whose value was not measured fails: a limit that cannot be checked is not
//! met.

use crate::load::protocol::{PublisherResult, SubscriberResult};
use crate::load::report::{LimitResult, Report};
use crate::load::workload::{RECORDER_PROCESS, Rate, SubscriberLimits};

/// Nanoseconds in a microsecond.
const NANOS_PER_MICRO: u64 = 1_000;

/// A limit on a value that must not exceed it.
fn at_most(key: &str, scope: &str, limit: u64, measured: Option<u64>) -> LimitResult {
    LimitResult {
        key: key.to_owned(),
        scope: scope.to_owned(),
        limit,
        measured,
        pass: measured.is_some_and(|value| value <= limit),
    }
}

/// A microsecond limit on a nanosecond value. Both sides are compared in nanoseconds,
/// and the measured value is reported in microseconds, rounded up.
fn at_most_micros(key: &str, scope: &str, limit_us: u64, measured_ns: Option<u64>) -> LimitResult {
    let bound = limit_us.saturating_mul(NANOS_PER_MICRO);
    LimitResult {
        key: key.to_owned(),
        scope: scope.to_owned(),
        limit: limit_us,
        measured: measured_ns.map(|nanos| nanos.div_ceil(NANOS_PER_MICRO)),
        pass: measured_ns.is_some_and(|nanos| nanos <= bound),
    }
}

/// The frames a subscriber lost in the window: the framework's missing sequences, or
/// the window's messages it did not receive when that is more (a lost last frame leaves
/// no gap).
#[must_use]
pub fn loss(published: Option<&PublisherResult>, received: &SubscriberResult) -> Option<u64> {
    let unreceived = published?.sent_window.saturating_sub(received.received_window);
    Some(received.missing_sequences.max(unreceived))
}

fn subscriber_limits(
    limits: &SubscriberLimits,
    scope: &str,
    published: Option<&PublisherResult>,
    received: Option<&SubscriberResult>,
    out: &mut Vec<LimitResult>,
) {
    if let Some(limit) = limits.latency_p99_us {
        let measured = received.and_then(|result| result.latency.p99_ns);
        out.push(at_most_micros("latency_p99_us", scope, limit, measured));
    }
    if let Some(limit) = limits.max_interval_latency_p99_us {
        let measured = received.and_then(|result| result.worst_interval_p99_ns);
        out.push(at_most_micros("max_interval_latency_p99_us", scope, limit, measured));
    }
    if let Some(limit) = limits.max_loss {
        let measured = received.and_then(|result| loss(published, result));
        out.push(at_most("max_loss", scope, limit, measured));
    }
    if limits.loss_accounted == Some(true) {
        out.push(at_most("loss_accounted", scope, 0, received.map(unaccounted)));
    }
}

/// How far a subscriber's loss is from accounted: the difference between the
/// framework's missing-sequence count and the harness's gap count, plus every probe
/// that came out of order or was no probe. 0 when every loss is counted alike.
#[must_use]
pub fn unaccounted(received: &SubscriberResult) -> u64 {
    received
        .missing_sequences
        .abs_diff(received.harness_gaps)
        .saturating_add(received.reordered)
        .saturating_add(received.malformed)
}

/// Every limit of the report's workload, checked against the report. The workload is
/// walked, and each topic, subscriber and process is looked up in the report by name: an
/// entry the report lacks is not measured, which fails.
#[must_use]
pub fn evaluate(report: &Report) -> Vec<LimitResult> {
    let workload = &report.config.workload;
    let limits = &workload.limits;
    let mut out = Vec::new();
    for topic in &workload.topics {
        let measured = report.topics.iter().find(|measured| measured.name == topic.name);
        let published = measured.and_then(|measured| measured.publish.as_ref());
        for subscriber in &topic.subscribers {
            let scope = format!("{}@{}", topic.name, subscriber.process);
            let received = measured
                .and_then(|measured| {
                    measured.subscribers.iter().find(|s| s.process == subscriber.process)
                })
                .and_then(|measured| measured.result.as_ref());
            let own = subscriber.limits.unwrap_or_else(|| limits.subscriber_part());
            subscriber_limits(&own, &scope, published, received, &mut out);
        }
        let scope = topic.name.as_str();
        if let Some(limit) = limits.max_refused_publishes {
            out.push(at_most("max_refused_publishes", scope, limit, published.map(|p| p.refused)));
        }
        if let Some(limit) = limits.max_publish_call_us {
            let longest = published.and_then(|p| p.publish_call.max_ns);
            out.push(at_most_micros("max_publish_call_us", scope, limit, longest));
        }
        // A topic sent back to back has no periods, so it has no missed-period result.
        if let (Some(limit), Rate::Hz(_)) = (limits.max_missed_periods, topic.rate_hz) {
            let missed = published.and_then(|p| p.missed_periods);
            out.push(at_most("max_missed_periods", scope, limit, missed));
        }
        if let Some(limit) = limits.max_unrecorded {
            let unrecorded = report
                .recorder
                .as_ref()
                .and_then(|recorder| recorder.topics.iter().find(|t| t.topic == topic.name))
                .map(|recorded| recorded.unrecorded);
            out.push(at_most("max_unrecorded", scope, limit, unrecorded));
        }
    }
    if let Some(limit) = limits.max_rss_growth_bytes {
        let recorder = workload.recorder_enabled().then_some(RECORDER_PROCESS);
        let names = workload.processes.iter().map(|process| process.name.as_str());
        for name in names.chain(recorder) {
            let growth = report
                .processes
                .iter()
                .find(|process| process.name == name)
                .and_then(|process| process.rss_peak.zip(process.rss_at_window_start))
                .map(|(peak, start)| peak.saturating_sub(start));
            out.push(at_most("max_rss_growth_bytes", name, limit, growth));
        }
    }
    if let Some(limit) = limits.max_shm_growth_bytes {
        let domain = report.domain_shared_memory;
        let growth =
            domain.peak.zip(domain.at_window_start).map(|(peak, start)| peak.saturating_sub(start));
        out.push(at_most("max_shm_growth_bytes", "domain", limit, growth));
    }
    out
}
