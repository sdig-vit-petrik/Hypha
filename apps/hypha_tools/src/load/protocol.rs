//! What the coordinator and a worker say to each other: one JSON object per line, the
//! coordinator on the worker's stdin, the worker on its stdout after [`LINE_PREFIX`]. A
//! worker's other output lines are not protocol lines and pass through to stderr.

use serde::{Deserialize, Serialize};

use crate::load::histogram::HistogramSummary;

/// The start of each protocol line a worker writes.
pub const LINE_PREFIX: &str = "hypha-load ";

/// What one worker publishes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublisherPlan {
    /// The topic.
    pub topic: String,
    /// The size of each message.
    pub bytes: usize,
    /// The period in nanoseconds; `None` sends back to back.
    pub period_ns: Option<u64>,
    /// Extra messages back to back: how many, and every how many nanoseconds.
    pub burst: Option<(u32, u64)>,
}

/// What one worker subscribes to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubscriberPlan {
    /// The topic.
    pub topic: String,
    /// The queue depth; 0 is the deepest the service admits.
    pub depth: usize,
    /// Nanoseconds of busy work per sample.
    pub work_ns: u64,
}

/// Everything one worker does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerPlan {
    /// The process name; its node is `load_<process>`.
    pub process: String,
    /// Its publishers.
    pub publishers: Vec<PublisherPlan>,
    /// Its subscribers.
    pub subscribers: Vec<SubscriberPlan>,
    /// The period of its sample lines, in nanoseconds.
    pub sample_interval_ns: u64,
}

/// The times of a run on the host monotonic clock, in nanoseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Times {
    /// Publishers start.
    pub start: i64,
    /// The measuring window starts.
    pub window_start: i64,
    /// The measuring window ends.
    pub window_end: i64,
    /// Publishers stop.
    pub publish_end: i64,
}

impl Times {
    /// Whether a message sent at `sent` belongs to the window.
    #[must_use]
    pub fn in_window(&self, sent: i64) -> bool {
        (self.window_start..self.window_end).contains(&sent)
    }
}

/// A line from the coordinator to a worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ToWorker {
    /// The first line: what to do. The worker opens its publishers.
    Plan {
        /// The plan.
        plan: WorkerPlan,
    },
    /// Open the subscribers; every publisher of the run is open.
    Subscribe,
    /// Publish from `times.start` to `times.publish_end`.
    Start {
        /// The run's times.
        times: Times,
    },
    /// Report the result and end.
    Stop,
}

/// The stage a worker is ready for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// Its publishers are open.
    Publishers,
    /// Its subscribers are open and attached.
    Subscribers,
}

/// One subscriber over one sample interval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntervalSample {
    /// The topic.
    pub topic: String,
    /// The 99th percentile latency of the interval's samples of the window.
    pub p99_ns: Option<u64>,
}

/// What one publisher did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublisherResult {
    /// The topic.
    pub topic: String,
    /// Messages sent with a stamp inside the window.
    pub sent_window: u64,
    /// Messages sent over the whole run.
    pub sent_total: u64,
    /// Publishes the transport refused over the whole run.
    pub refused: u64,
    /// Deadlines of the window the publisher missed: passed while it was late, or never
    /// reached because it stopped. `None` for a topic without a period (`"max"`).
    pub missed_periods: Option<u64>,
    /// The duration of each publish call of the window.
    pub publish_call: HistogramSummary,
}

/// What one subscriber received.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubscriberResult {
    /// The topic.
    pub topic: String,
    /// Messages received with a send stamp inside the window.
    pub received_window: u64,
    /// Messages received over the whole run.
    pub received_total: u64,
    /// The framework's missing-sequence count, from the window's start to the stop.
    pub missing_sequences: u64,
    /// The harness's own count of skipped probe sequences over the same span.
    pub harness_gaps: u64,
    /// Probes whose sequence was not newer than the one before.
    pub reordered: u64,
    /// Samples that were not probes.
    pub malformed: u64,
    /// Send call to callback entry, over the window.
    pub latency: HistogramSummary,
    /// The worst 99th percentile of any sample interval of the window, taken from every
    /// interval as it ends.
    pub worst_interval_p99_ns: Option<u64>,
}

/// A line from a worker to the coordinator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FromWorker {
    /// The worker finished a stage.
    Ready {
        /// The stage.
        stage: Stage,
    },
    /// One sample interval ended; also the heartbeat.
    Sample {
        /// Its subscribers over the interval.
        subscribers: Vec<IntervalSample>,
    },
    /// Every publisher stopped.
    Published {
        /// Its publishers.
        publishers: Vec<PublisherResult>,
    },
    /// The result, the last line.
    Result {
        /// Its subscribers.
        subscribers: Vec<SubscriberResult>,
        /// Callbacks its `WaitSet` reported slow.
        slow_callbacks: u64,
    },
    /// The worker cannot go on.
    Failed {
        /// Why.
        reason: String,
    },
}

/// `message` as one line, without the newline.
///
/// # Errors
/// A message that does not serialize (none does).
pub fn encode<T: Serialize>(message: &T) -> Result<String, String> {
    serde_json::to_string(message).map_err(|error| format!("cannot encode a line: {error}"))
}

/// The worker line `line` without its newline: `Ok(None)` for a line that is not a
/// protocol line.
///
/// # Errors
/// A protocol line that does not parse.
pub fn decode_worker_line(line: &str) -> Result<Option<FromWorker>, String> {
    let Some(json) = line.strip_prefix(LINE_PREFIX) else {
        return Ok(None);
    };
    serde_json::from_str(json)
        .map(Some)
        .map_err(|error| format!("malformed line ({error}): {line}"))
}

#[cfg(test)]
#[expect(clippy::expect_used, reason = "a test fails on the first refused step")]
mod tests {
    use super::*;

    #[test]
    fn every_line_round_trips() {
        let plan = ToWorker::Plan {
            plan: WorkerPlan {
                process: "sensors".into(),
                publishers: vec![PublisherPlan {
                    topic: "/a".into(),
                    bytes: 64,
                    period_ns: Some(1_000_000),
                    burst: Some((3, 500_000_000)),
                }],
                subscribers: vec![SubscriberPlan { topic: "/b".into(), depth: 4, work_ns: 0 }],
                sample_interval_ns: 1_000_000_000,
            },
        };
        let text = encode(&plan).expect("encode");
        assert_eq!(serde_json::from_str::<ToWorker>(&text).expect("decode"), plan);

        let result = FromWorker::Result {
            subscribers: vec![SubscriberResult { topic: "/b".into(), ..Default::default() }],
            slow_callbacks: 2,
        };
        let line = format!("{LINE_PREFIX}{}", encode(&result).expect("encode"));
        assert_eq!(decode_worker_line(&line), Ok(Some(result)));
    }

    #[test]
    fn a_line_without_the_prefix_is_not_protocol_and_a_broken_one_is_refused() {
        assert_eq!(decode_worker_line("[INFO] something"), Ok(None));
        assert!(decode_worker_line("hypha-load {\"kind\":\"nope\"}").is_err());
    }

    #[test]
    fn the_window_holds_its_start_and_not_its_end() {
        let times = Times { start: 0, window_start: 10, window_end: 20, publish_end: 25 };
        assert!(!times.in_window(9));
        assert!(times.in_window(10));
        assert!(times.in_window(19));
        assert!(!times.in_window(20));
    }
}
