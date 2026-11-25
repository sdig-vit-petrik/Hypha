//! The workload file of `hypha load`: its processes, its topics and their publishers
//! and subscribers, the run's times, the recorder and the pass limits.
//!
//! Every key of `[run]`, of a `[[process]]` and the keys `name`, `bytes`, `rate_hz`,
//! `publisher` and `subscribers` of a `[[topic]]` are required. The optional keys and
//! what their absence means: a subscriber's `depth` (0, the deepest queue the topic's
//! service admits), `work_us` (0, no work per sample) and `limits` (the run's
//! `[limits]`); a topic's `burst` (none); the `[recorder]` (off) and its `compression`
//! (`lz4`); every key of `[limits]` (not checked). An unknown key is refused.

use std::collections::BTreeSet;
use std::fmt;

use clap::ValueEnum as _;
use serde::{Deserialize, Serialize};

use crate::load::probe::PROBE_HEADER_BYTES;
use crate::recorder::McapCompression;
use hypha_node::sdk::DEFAULT_MAX_DEPTH;

/// Subscribers a topic admits from processes that declare no subscriber policy, as
/// `hypha load` workers and `hypha record` are; a further one is refused by the transport.
pub use hypha_node::sdk::UNDECLARED_SUBSCRIBER_SLOTS;

/// The name the report gives the recorder's process, which no worker may take.
pub const RECORDER_PROCESS: &str = "recorder";

/// The shortest silence after which a worker counts as hung; the bound is the larger of
/// this and three sample intervals, so that a loaded host does not fail a run.
pub const MIN_HEARTBEAT_NANOS: u64 = 10_000_000_000;

/// The highest topic rate: one message per nanosecond period.
const MAX_RATE_HZ: f64 = 1e9;

/// A parsed and checked workload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workload {
    /// The run's times.
    pub run: RunSection,
    /// The recorder, off when absent.
    #[serde(default)]
    pub recorder: Option<RecorderSection>,
    /// The worker processes.
    #[serde(rename = "process")]
    pub processes: Vec<ProcessSection>,
    /// The topics.
    #[serde(rename = "topic")]
    pub topics: Vec<TopicSection>,
    /// The pass limits of the whole run.
    #[serde(default)]
    pub limits: Limits,
}

/// `[run]`: the times of a run, in seconds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunSection {
    /// The measuring window.
    pub duration_s: f64,
    /// The time before the window, which no statistic counts.
    pub warmup_s: f64,
    /// The time after the window in which publishers go on sending, so that a lost last
    /// frame of the window shows as a gap.
    pub cooldown_s: f64,
    /// The period of the time series and of the workers' heartbeat.
    pub sample_interval_s: f64,
    /// Whether the workers run as a bounded deployment (`HYPHA_BOUNDED_MEMORY`).
    pub bounded_memory: bool,
}

/// `[recorder]`: a `hypha record` child that records every topic of the run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecorderSection {
    /// Whether the recorder runs.
    pub enabled: bool,
    /// Chunk compression, as `hypha record --compression`.
    #[serde(default)]
    pub compression: Option<String>,
}

/// `[[process]]`: one worker process.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessSection {
    /// Its name, `[a-z][a-z0-9_]*`; its node is `load_<name>`.
    pub name: String,
}

/// The rate of a topic: messages per second, or as fast as the publisher can send.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Rate {
    /// Periodic, at this many messages per second.
    Hz(f64),
    /// `"max"`: back to back, with no period.
    Max(MaxRate),
}

/// The word `"max"` of [`Rate::Max`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MaxRate {
    /// As fast as the publisher can send.
    Max,
}

/// Extra messages sent back to back at a fixed interval, on top of the rate.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Burst {
    /// Messages per burst, at least 1.
    pub count: u32,
    /// Seconds between bursts.
    pub every_s: f64,
}

/// `[[topic]]`: one topic, its one publisher and its subscribers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TopicSection {
    /// The topic, an absolute name without wildcards.
    pub name: String,
    /// The size of each message, at least the probe header.
    pub bytes: usize,
    /// The rate.
    pub rate_hz: Rate,
    /// The process that publishes it.
    pub publisher: String,
    /// Extra messages at a fixed interval.
    #[serde(default)]
    pub burst: Option<Burst>,
    /// Its subscribers.
    pub subscribers: Vec<SubscriberSection>,
}

/// One subscriber of a topic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubscriberSection {
    /// The process it runs in.
    pub process: String,
    /// Its queue depth; 0 is the deepest the topic's service admits.
    #[serde(default)]
    pub depth: usize,
    /// Microseconds of busy work per sample, which makes a slow reader.
    #[serde(default)]
    pub work_us: u64,
    /// Its own limits, which replace the run's subscriber limits for it.
    #[serde(default)]
    pub limits: Option<SubscriberLimits>,
}

/// The limits of one subscriber. An absent key is not checked.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubscriberLimits {
    /// The 99th percentile latency over the window, send call to callback entry.
    pub latency_p99_us: Option<u64>,
    /// The worst 99th percentile latency of any one sample interval of the window.
    pub max_interval_latency_p99_us: Option<u64>,
    /// The frames lost in the window: missing sequences, or sent but not received.
    pub max_loss: Option<u64>,
    /// Whether the framework's missing-sequence count must equal the harness's own
    /// count of sequence gaps.
    pub loss_accounted: Option<bool>,
}

/// `[limits]`: the pass limits of the run. An absent key is not checked.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    /// As [`SubscriberLimits::latency_p99_us`], for every subscriber without limits.
    pub latency_p99_us: Option<u64>,
    /// As [`SubscriberLimits::max_interval_latency_p99_us`].
    pub max_interval_latency_p99_us: Option<u64>,
    /// As [`SubscriberLimits::max_loss`].
    pub max_loss: Option<u64>,
    /// As [`SubscriberLimits::loss_accounted`].
    pub loss_accounted: Option<bool>,
    /// Publishes the transport refused, per publisher, over the whole run.
    pub max_refused_publishes: Option<u64>,
    /// The longest publish call of the window, per publisher.
    pub max_publish_call_us: Option<u64>,
    /// Periods a publisher missed in the window.
    pub max_missed_periods: Option<u64>,
    /// Per topic: messages sent over the run that the recording does not hold. Not
    /// measured, and so failed, without the recorder.
    pub max_unrecorded: Option<u64>,
    /// Per process: the largest resident memory of the window minus the one at its start.
    pub max_rss_growth_bytes: Option<u64>,
    /// The domain's largest written shared memory of the window minus the one at its start.
    pub max_shm_growth_bytes: Option<u64>,
}

impl Limits {
    /// The subscriber part, which a subscriber without its own limits takes.
    #[must_use]
    pub fn subscriber_part(&self) -> SubscriberLimits {
        SubscriberLimits {
            latency_p99_us: self.latency_p99_us,
            max_interval_latency_p99_us: self.max_interval_latency_p99_us,
            max_loss: self.max_loss,
            loss_accounted: self.loss_accounted,
        }
    }
}

/// Why a workload is refused.
#[derive(Debug, Clone, PartialEq)]
pub enum WorkloadError {
    /// Not TOML, a missing or unknown key, or a value of the wrong type.
    Toml(String),
    /// No `[[process]]` or no `[[topic]]`.
    Empty(&'static str),
    /// A process name that is not `[a-z][a-z0-9_]*`.
    InvalidProcessName(String),
    /// Two processes of one name.
    DuplicateProcess(String),
    /// A process with no publisher and no subscriber.
    IdleProcess(String),
    /// A topic name that is not absolute or holds a wildcard.
    InvalidTopicName(String),
    /// Two topics of one name.
    DuplicateTopic(String),
    /// A publisher or subscriber in a process the workload does not declare.
    UnknownProcess {
        /// The topic.
        topic: String,
        /// The process it names.
        process: String,
    },
    /// Two subscribers of one topic in one process.
    DuplicateSubscriber {
        /// The topic.
        topic: String,
        /// The process.
        process: String,
    },
    /// More subscribers on a topic than its service admits from undeclared ones.
    TooManySubscribers {
        /// The topic.
        topic: String,
        /// Its workload subscribers.
        subscribers: usize,
        /// Whether the recorder takes one more slot.
        recorder: bool,
    },
    /// A message smaller than the probe header.
    PayloadTooSmall {
        /// The topic.
        topic: String,
        /// Its size.
        bytes: usize,
    },
    /// A number out of its range.
    OutOfRange {
        /// The key, such as `run.duration_s` or `topic /x rate_hz`.
        key: String,
        /// What it must be.
        rule: &'static str,
    },
    /// A compression `hypha record` does not know.
    UnknownCompression(String),
    /// The name the recorder's process takes in the report.
    ReservedProcessName,
    /// A process whose subscribers can hold its main loop longer than the heartbeat
    /// bound: a full queue of each, times its work per sample.
    StarvedHeartbeat {
        /// The process.
        process: String,
        /// Its queued work, in microseconds.
        work_us: u64,
        /// The heartbeat bound, in microseconds.
        bound_us: u64,
    },
}

impl fmt::Display for WorkloadError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Toml(message) => write!(out, "{message}"),
            Self::Empty(what) => write!(out, "the workload declares no {what}"),
            Self::InvalidProcessName(name) => {
                write!(out, "process '{name}': a name is [a-z][a-z0-9_]*, at most 32 characters")
            }
            Self::DuplicateProcess(name) => write!(out, "process '{name}' is declared twice"),
            Self::IdleProcess(name) => {
                write!(out, "process '{name}' has no publisher and no subscriber")
            }
            Self::InvalidTopicName(name) => {
                write!(out, "topic '{name}': a topic starts with '/' and holds no '*' or '~'")
            }
            Self::DuplicateTopic(name) => write!(out, "topic '{name}' is declared twice"),
            Self::UnknownProcess { topic, process } => {
                write!(out, "topic '{topic}' names process '{process}', which is not declared")
            }
            Self::DuplicateSubscriber { topic, process } => {
                write!(out, "topic '{topic}' has two subscribers in process '{process}'")
            }
            Self::TooManySubscribers { topic, subscribers, recorder } => {
                let recorder = if *recorder { " and the recorder" } else { "" };
                write!(
                    out,
                    "topic '{topic}' has {subscribers} subscriber(s){recorder}, but a topic \
                     admits {UNDECLARED_SUBSCRIBER_SLOTS} subscribers that declare no policy, \
                     as load workers and the recorder do"
                )
            }
            Self::PayloadTooSmall { topic, bytes } => write!(
                out,
                "topic '{topic}': bytes = {bytes} is below the {PROBE_HEADER_BYTES}-byte probe header"
            ),
            Self::OutOfRange { key, rule } => write!(out, "{key} must be {rule}"),
            Self::UnknownCompression(name) => {
                write!(out, "recorder.compression '{name}' is not lz4, zstd or none")
            }
            Self::ReservedProcessName => {
                write!(out, "process '{RECORDER_PROCESS}': the report names the recorder so")
            }
            Self::StarvedHeartbeat { process, work_us, bound_us } => write!(
                out,
                "process '{process}': full queues times work_us are {work_us} us of work in \
                 one turn, which reaches the heartbeat bound of {bound_us} us (the larger of \
                 10 s and three sample intervals); lower work_us or depth, or give the slow \
                 reader a process of its own"
            ),
        }
    }
}

impl std::error::Error for WorkloadError {}

/// Whether `name` is a process name: `[a-z][a-z0-9_]*`, at most 32 characters, so that
/// `load_<name>` is a node name.
fn is_process_name(name: &str) -> bool {
    let mut characters = name.chars();
    let starts = characters.next().is_some_and(|first| first.is_ascii_lowercase());
    starts
        && name.len() <= 32
        && characters.all(|next| next.is_ascii_lowercase() || next.is_ascii_digit() || next == '_')
}

fn is_topic_name(name: &str) -> bool {
    name.starts_with('/') && name.len() > 1 && !name.contains(['*', '~', ' '])
}

fn positive(key: String, value: f64) -> Result<(), WorkloadError> {
    if value.is_finite() && value > 0.0 {
        Ok(())
    } else {
        Err(WorkloadError::OutOfRange { key, rule: "a finite number above 0" })
    }
}

fn not_negative(key: String, value: f64) -> Result<(), WorkloadError> {
    if value.is_finite() && value >= 0.0 {
        Ok(())
    } else {
        Err(WorkloadError::OutOfRange { key, rule: "a finite number of at least 0" })
    }
}

impl Workload {
    /// Parses and checks a workload file's text.
    ///
    /// # Errors
    /// The first [`WorkloadError`] the text has.
    pub fn from_toml(text: &str) -> Result<Self, WorkloadError> {
        let workload: Self =
            toml::from_str(text).map_err(|error| WorkloadError::Toml(error.to_string()))?;
        workload.check()?;
        Ok(workload)
    }

    /// Whether the recorder runs.
    #[must_use]
    pub fn recorder_enabled(&self) -> bool {
        self.recorder.as_ref().is_some_and(|recorder| recorder.enabled)
    }

    /// The recorder's compression.
    ///
    /// # Errors
    /// A compression `hypha record` does not know.
    pub fn compression(&self) -> Result<McapCompression, WorkloadError> {
        match self.recorder.as_ref().and_then(|recorder| recorder.compression.as_deref()) {
            None => Ok(McapCompression::default()),
            Some(name) => McapCompression::from_str(name, false)
                .map_err(|_| WorkloadError::UnknownCompression(name.to_owned())),
        }
    }

    fn check(&self) -> Result<(), WorkloadError> {
        let run = &self.run;
        positive("run.duration_s".into(), run.duration_s)?;
        not_negative("run.warmup_s".into(), run.warmup_s)?;
        not_negative("run.cooldown_s".into(), run.cooldown_s)?;
        positive("run.sample_interval_s".into(), run.sample_interval_s)?;
        self.compression()?;
        if self.processes.is_empty() {
            return Err(WorkloadError::Empty("[[process]]"));
        }
        if self.topics.is_empty() {
            return Err(WorkloadError::Empty("[[topic]]"));
        }

        let mut processes = BTreeSet::new();
        for process in &self.processes {
            if !is_process_name(&process.name) {
                return Err(WorkloadError::InvalidProcessName(process.name.clone()));
            }
            if process.name == RECORDER_PROCESS {
                return Err(WorkloadError::ReservedProcessName);
            }
            if !processes.insert(process.name.as_str()) {
                return Err(WorkloadError::DuplicateProcess(process.name.clone()));
            }
        }

        let mut topics = BTreeSet::new();
        let mut used = BTreeSet::new();
        for topic in &self.topics {
            self.check_topic(topic, &processes)?;
            if !topics.insert(topic.name.as_str()) {
                return Err(WorkloadError::DuplicateTopic(topic.name.clone()));
            }
            used.insert(topic.publisher.as_str());
            used.extend(topic.subscribers.iter().map(|subscriber| subscriber.process.as_str()));
        }
        if let Some(idle) = processes.difference(&used).next() {
            return Err(WorkloadError::IdleProcess((*idle).to_owned()));
        }
        self.check_heartbeat()
    }

    /// The longest a worker may stay silent before it counts as hung, in nanoseconds:
    /// the larger of [`MIN_HEARTBEAT_NANOS`] and three sample intervals.
    #[must_use]
    pub fn heartbeat_nanos(&self) -> u64 {
        let interval = self.run.sample_interval_s * 3e9;
        // A finite interval above 0 (checked); one past the range of u64 saturates.
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a float to integer cast saturates, and the value is finite and positive"
        )]
        let interval = interval as u64;
        interval.max(MIN_HEARTBEAT_NANOS)
    }

    /// Refuses a process whose subscribers' full queues, times their work per sample,
    /// reach the heartbeat bound: one turn of its `WaitSet` would hold its heartbeat.
    fn check_heartbeat(&self) -> Result<(), WorkloadError> {
        let bound_us = self.heartbeat_nanos().checked_div(1_000).unwrap_or(0);
        for process in &self.processes {
            let work_us = self
                .topics
                .iter()
                .flat_map(|topic| &topic.subscribers)
                .filter(|subscriber| subscriber.process == process.name)
                .map(|subscriber| {
                    let depth =
                        if subscriber.depth == 0 { DEFAULT_MAX_DEPTH } else { subscriber.depth };
                    u64::try_from(depth).unwrap_or(u64::MAX).saturating_mul(subscriber.work_us)
                })
                .fold(0_u64, u64::saturating_add);
            if work_us >= bound_us {
                return Err(WorkloadError::StarvedHeartbeat {
                    process: process.name.clone(),
                    work_us,
                    bound_us,
                });
            }
        }
        Ok(())
    }

    fn check_topic(
        &self,
        topic: &TopicSection,
        processes: &BTreeSet<&str>,
    ) -> Result<(), WorkloadError> {
        let name = &topic.name;
        if !is_topic_name(name) {
            return Err(WorkloadError::InvalidTopicName(name.clone()));
        }
        if topic.bytes < PROBE_HEADER_BYTES || u32::try_from(topic.bytes).is_err() {
            return Err(WorkloadError::PayloadTooSmall { topic: name.clone(), bytes: topic.bytes });
        }
        if let Rate::Hz(rate) = topic.rate_hz {
            positive(format!("topic '{name}' rate_hz"), rate)?;
            if rate > MAX_RATE_HZ {
                let key = format!("topic '{name}' rate_hz");
                return Err(WorkloadError::OutOfRange { key, rule: "at most 1e9, or \"max\"" });
            }
        }
        if let Some(burst) = topic.burst {
            positive(format!("topic '{name}' burst.every_s"), burst.every_s)?;
            if burst.count == 0 {
                let key = format!("topic '{name}' burst.count");
                return Err(WorkloadError::OutOfRange { key, rule: "at least 1" });
            }
        }
        let unknown = |process: &str| WorkloadError::UnknownProcess {
            topic: name.clone(),
            process: process.to_owned(),
        };
        if !processes.contains(topic.publisher.as_str()) {
            return Err(unknown(&topic.publisher));
        }
        let mut subscribing = BTreeSet::new();
        for subscriber in &topic.subscribers {
            if !processes.contains(subscriber.process.as_str()) {
                return Err(unknown(&subscriber.process));
            }
            if !subscribing.insert(subscriber.process.as_str()) {
                return Err(WorkloadError::DuplicateSubscriber {
                    topic: name.clone(),
                    process: subscriber.process.clone(),
                });
            }
        }
        let recorder = self.recorder_enabled();
        let taken = topic.subscribers.len().saturating_add(usize::from(recorder));
        if taken > UNDECLARED_SUBSCRIBER_SLOTS {
            return Err(WorkloadError::TooManySubscribers {
                topic: name.clone(),
                subscribers: topic.subscribers.len(),
                recorder,
            });
        }
        Ok(())
    }
}
