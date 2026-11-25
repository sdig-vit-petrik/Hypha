//! The one report of a `hypha load` run, written as JSON.

use serde::{Deserialize, Serialize};

use crate::load::protocol::{PublisherResult, SubscriberResult};
use crate::load::series::SeriesPoint;
use crate::load::workload::Workload;

/// The schema name and version a report carries.
pub const SCHEMA: &str = "hypha.load.report/1";

/// How the run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// It ran, and every limit passed.
    Pass,
    /// It ran, and a limit failed.
    LimitFailed,
    /// The run itself failed; its numbers are incomplete.
    RunFailed,
}

/// The host the run measured.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Host {
    /// The host name (`uname -n`).
    pub hostname: Option<String>,
    /// The operating system, as Rust names it (`linux`, `macos`).
    pub os: String,
    /// The kernel release (`uname -r`).
    pub os_release: Option<String>,
    /// The machine architecture.
    pub arch: String,
    /// The processors the coordinator may run on.
    pub cpus: Option<usize>,
    /// The hypha version.
    pub hypha_version: String,
    /// `release` or `debug`: a debug build measures unoptimised code.
    pub build: String,
}

/// The run's setup.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The workload file, as given.
    pub workload_path: String,
    /// The MD5 of the workload file's text.
    pub workload_md5: String,
    /// The workload as parsed.
    pub workload: Workload,
    /// The private bus domain of the run.
    pub domain: String,
    /// The start of the run, in nanoseconds since the UNIX epoch.
    pub started_unix_ns: u64,
}

/// A quantity this run did not measure, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotMeasured {
    /// The report key it would fill.
    pub quantity: String,
    /// Why it is `null`.
    pub reason: String,
}

/// One subscriber of a topic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubscriberReport {
    /// Its process.
    pub process: String,
    /// What it received; `None` when the run failed before its result.
    pub result: Option<SubscriberResult>,
}

/// One topic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TopicReport {
    /// The topic.
    pub name: String,
    /// The size of each message.
    pub bytes: usize,
    /// Messages per second; `None` for `"max"`.
    pub rate_hz: Option<f64>,
    /// The publishing process.
    pub publisher: String,
    /// What its publisher did; `None` when the run failed before it reported.
    pub publish: Option<PublisherResult>,
    /// Its subscribers, in workload order.
    pub subscribers: Vec<SubscriberReport>,
}

/// One process of the run: a worker, or the recorder.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessReport {
    /// The worker's name, or `recorder`.
    pub name: String,
    /// Its process id.
    pub pid: u32,
    /// Resident bytes at the window's start.
    pub rss_at_window_start: Option<u64>,
    /// The largest resident bytes from the window's start to the end of publishing.
    pub rss_peak: Option<u64>,
    /// Resident shared-memory bytes at the window's start (Linux only).
    pub shmem_at_window_start: Option<u64>,
    /// The largest resident shared-memory bytes, as `rss_peak` (Linux only).
    pub shmem_peak: Option<u64>,
    /// Callbacks its `WaitSet` reported slow; `None` for the recorder.
    pub slow_callbacks: Option<u64>,
}

/// The written shared memory of the run's domain.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainMemory {
    /// At the window's start.
    pub at_window_start: Option<u64>,
    /// The largest from the window's start to the end of publishing.
    pub peak: Option<u64>,
}

/// What the recorder recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecorderReport {
    /// The recording.
    pub output: String,
    /// Its exit code; `None` when it ended by a signal or was not waited for.
    pub exit_code: Option<i32>,
    /// Per topic: messages recorded, and messages sent over the run minus recorded.
    pub topics: Vec<RecordedTopic>,
}

/// One recorded topic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedTopic {
    /// The topic.
    pub topic: String,
    /// Messages in the recording.
    pub recorded: u64,
    /// Messages sent over the whole run that the recording does not hold.
    pub unrecorded: u64,
}

/// The time series of the run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeriesReport {
    /// Milliseconds one point spans: the sample interval times the decimation.
    pub point_ms: u64,
    /// The subscribers of `p99_ns`, as `topic@process`.
    pub subscribers: Vec<String>,
    /// The points, oldest first.
    pub points: Vec<SeriesPoint>,
}

/// One checked limit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LimitResult {
    /// The limit's key, such as `latency_p99_us`.
    pub key: String,
    /// What it was checked on: `topic@process`, a topic, a process, or `domain`.
    pub scope: String,
    /// The limit, in the key's unit; a boolean limit is 1.
    pub limit: u64,
    /// The measured value in the same unit; `None` when it was not measured.
    pub measured: Option<u64>,
    /// Whether it passed. A limit whose value was not measured fails.
    pub pass: bool,
}

/// The report of one run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    /// [`SCHEMA`].
    pub schema: String,
    /// How the run ended.
    pub outcome: Outcome,
    /// Why the run failed; `None` otherwise.
    pub reason: Option<String>,
    /// The host.
    pub host: Host,
    /// The setup.
    pub config: Config,
    /// The quantities this host or run did not measure.
    pub not_measured: Vec<NotMeasured>,
    /// The topics, in workload order.
    pub topics: Vec<TopicReport>,
    /// The workers in workload order, then the recorder.
    pub processes: Vec<ProcessReport>,
    /// The domain's written shared memory.
    pub domain_shared_memory: DomainMemory,
    /// The recorder; `None` when it was off.
    pub recorder: Option<RecorderReport>,
    /// The time series.
    pub series: SeriesReport,
    /// Every checked limit.
    pub limits: Vec<LimitResult>,
}
