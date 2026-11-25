//! The coordinator of `hypha load`: it starts one worker per workload process (this
//! executable again, in worker mode) and the recorder, leads them through the run,
//! samples their memory, and writes the one report.
//!
//! The run uses a domain of its own, `load_<pid>` (or `<HYPHA_DOMAIN>_ld<pid>` under a
//! caller's domain), so it cannot touch a running stack, and removes that domain's
//! shared memory at the end. The coordinator itself never joins the bus.

use std::collections::BTreeMap;
use std::io::{BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hypha_node::sdk::clean::{DomainSelection, clean, domain_shared_memory};
use hypha_node::sdk::clock::steady_now_nanos;

use crate::load::protocol::{
    FromWorker, PublisherPlan, PublisherResult, Stage, SubscriberPlan, SubscriberResult, Times,
    ToWorker, WorkerPlan, decode_worker_line, encode,
};
use crate::load::report::{
    Config, DomainMemory, Host, NotMeasured, Outcome, ProcessReport, RecordedTopic, RecorderReport,
    Report, SCHEMA, SeriesReport, SubscriberReport, TopicReport,
};
use crate::load::series::{MemoryPoint, Series, SeriesPoint, larger};
use crate::load::workload::{RECORDER_PROCESS, Rate, Workload};
use crate::load::{EXIT_LIMIT_FAILED, EXIT_PASS, EXIT_RUN_FAILED, limits};
use crate::proc_usage::{self, SHARED_MEMORY_UNAVAILABLE};

/// The longest one start-up or end phase may take.
const PHASE_BOUND_NANOS: i64 = 30_000_000_000;
/// The time between the start line and the first publish, for every worker to read it.
const START_LEAD_NANOS: i64 = 200_000_000;
/// The time between the last publisher's end and the stop line.
const DRAIN_NANOS: i64 = 500_000_000;
/// How long a child has after SIGTERM before it is killed.
const STOP_GRACE: Duration = Duration::from_secs(2);
/// The longest one wait for an event.
const MAX_WAIT_NANOS: i64 = 100_000_000;
/// The most points of the time series.
const SERIES_CAPACITY: usize = 512;
/// The line `hypha record` prints once it follows its topics.
const RECORDER_READY: &str = "Following ";

/// Where the coordinator reads and writes.
#[derive(Debug, Clone)]
pub struct Paths {
    /// The workload file.
    pub workload: PathBuf,
    /// The report.
    pub report: PathBuf,
    /// The recording, when the recorder runs; `None` puts it beside the report.
    pub recording: Option<PathBuf>,
}

/// Runs the workload of `paths`; the result is the exit code: 0 pass, 1 a limit failed,
/// 70 the run failed. `stop` is set by SIGINT and SIGTERM.
#[must_use]
pub fn run(paths: &Paths, stop: &AtomicBool) -> u8 {
    // A report of an earlier run must not stand for this one, whatever fails below.
    match std::fs::remove_file(&paths.report) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            eprintln!("hypha load: cannot remove the old '{}': {error}", paths.report.display());
            return EXIT_RUN_FAILED;
        }
        _ => {}
    }
    let text = match std::fs::read_to_string(&paths.workload) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("hypha load: cannot read '{}': {error}", paths.workload.display());
            return EXIT_RUN_FAILED;
        }
    };
    let workload = match Workload::from_toml(&text) {
        Ok(workload) => workload,
        Err(error) => {
            eprintln!("hypha load: '{}': {error}", paths.workload.display());
            return EXIT_RUN_FAILED;
        }
    };
    let domain = match run_domain() {
        Ok(domain) => domain,
        Err(error) => {
            eprintln!("hypha load: {error}");
            return EXIT_RUN_FAILED;
        }
    };
    let recording = paths.recording.clone().unwrap_or_else(|| paths.report.with_extension("mcap"));
    let started_unix_ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|since| u64::try_from(since.as_nanos()).ok())
        .unwrap_or(0);
    let config = Config {
        workload_path: paths.workload.display().to_string(),
        workload_md5: hypha_node::sdk::md5::md5_hex(text.as_bytes()),
        workload: workload.clone(),
        domain: domain.clone(),
        started_unix_ns,
    };

    let mut session = Session::new(workload, domain.clone(), recording, stop);
    let executed = session.execute();
    session.end_children();
    let removed = clean(&DomainSelection::Exact(domain.clone()));
    for failure in removed.failures {
        eprintln!("hypha load: cannot remove the shared memory of {domain}: {failure}");
    }
    for kept in removed.kept {
        eprintln!("hypha load: kept the shared memory of {}: {}", kept.domain, kept.reason);
    }
    let mut report = session.report(config, executed.err());
    report.limits =
        if report.outcome == Outcome::RunFailed { Vec::new() } else { limits::evaluate(&report) };
    if report.outcome != Outcome::RunFailed && report.limits.iter().any(|limit| !limit.pass) {
        report.outcome = Outcome::LimitFailed;
    }
    if let Err(error) = write_report(&report, &paths.report) {
        println!("hypha load: RUN FAILED: cannot write '{}': {error}", paths.report.display());
        return EXIT_RUN_FAILED;
    }
    print_summary(&report, &paths.report);
    match report.outcome {
        Outcome::Pass => EXIT_PASS,
        Outcome::LimitFailed => EXIT_LIMIT_FAILED,
        Outcome::RunFailed => EXIT_RUN_FAILED,
    }
}

/// Writes `report` to a file beside `path` and renames it to `path`, so that `path`
/// holds a whole report or none.
fn write_report(report: &Report, path: &Path) -> Result<(), String> {
    let json = serde_json::to_string_pretty(report).map_err(|error| error.to_string())?;
    let mut partial = path.as_os_str().to_owned();
    partial.push(".partial");
    let partial = PathBuf::from(partial);
    std::fs::write(&partial, json).map_err(|error| error.to_string())?;
    std::fs::rename(&partial, path).map_err(|error| {
        let _ = std::fs::remove_file(&partial);
        error.to_string()
    })
}

/// The run's private domain.
fn run_domain() -> Result<String, String> {
    let pid = std::process::id();
    let domain = match std::env::var("HYPHA_DOMAIN") {
        Ok(caller) if !caller.is_empty() => format!("{caller}_ld{pid}"),
        _ => format!("load_{pid}"),
    };
    // A prefix selection checks the name the way a domain name is checked.
    DomainSelection::prefix(&domain).map(|_| domain.clone()).map_err(|_| {
        format!("the run's domain '{domain}' is not a domain name (at most 32 characters)")
    })
}

/// Seconds as nanoseconds, for a value the workload checked to be finite and not
/// negative; `None` past the range of `i64`.
fn nanos(seconds: f64) -> Option<i64> {
    let value = (seconds * 1e9).round();
    if !(0.0..9.0e18).contains(&value) {
        return None;
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the value is a whole number in [0, 9e18), inside the range of i64"
    )]
    Some(value as i64)
}

/// The period of a rate the workload checked to be in (0, 1e9].
fn period_ns(rate: Rate) -> Option<u64> {
    match rate {
        Rate::Hz(hz) => nanos(1.0 / hz).and_then(|n| u64::try_from(n.max(1)).ok()),
        Rate::Max(_) => None,
    }
}

/// Who sent an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Worker(usize),
    Recorder,
}

/// A line of a child's stdout, or the end of it.
enum Event {
    Line(Source, String),
    Closed(Source),
}

/// Forwards each stdout line of a child as an event, then its end.
fn forward_lines(source: Source, stdout: std::process::ChildStdout, events: Sender<Event>) {
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if events.send(Event::Line(source, line)).is_err() {
                return;
            }
        }
        let _ = events.send(Event::Closed(source));
    });
}

/// The memory a process had at the window's start, and its peak since.
#[derive(Debug, Clone, Copy, Default)]
struct MemoryTrack {
    at_window_start: Option<MemoryPoint>,
    peak: MemoryPoint,
}

impl MemoryTrack {
    fn peak_with(&mut self, now: MemoryPoint) {
        self.peak.rss = larger(self.peak.rss, now.rss);
        self.peak.shmem = larger(self.peak.shmem, now.shmem);
    }
}

/// One worker process.
struct Worker {
    name: String,
    child: Child,
    stdin: Option<ChildStdin>,
    stage: Option<Stage>,
    last_heard: i64,
    published: Option<Vec<PublisherResult>>,
    result: Option<(Vec<SubscriberResult>, u64)>,
    /// The largest interval p99 of each of its subscribers since the last point.
    latest_p99: Vec<Option<u64>>,
    /// The index in the run's subscriber list of each of its subscribers.
    subscriber_index: Vec<usize>,
    memory: MemoryTrack,
}

/// The `hypha record` child.
struct Recorder {
    child: Child,
    output: PathBuf,
    ready: bool,
    ending: bool,
    exit: Option<ExitStatus>,
    memory: MemoryTrack,
    recorded: Option<BTreeMap<String, u64>>,
}

/// One run.
struct Session<'a> {
    workload: Workload,
    domain: String,
    recording: PathBuf,
    stop: &'a AtomicBool,
    sender: Sender<Event>,
    events: Receiver<Event>,
    workers: Vec<Worker>,
    recorder: Option<Recorder>,
    /// `topic@process` of every subscriber, in workload order.
    subscriber_names: Vec<String>,
    series: Series<SeriesPoint>,
    domain_memory: MemoryTrack,
    domain_error: Option<String>,
    times: Option<Times>,
    heartbeat_nanos: i64,
}

impl<'a> Session<'a> {
    fn new(workload: Workload, domain: String, recording: PathBuf, stop: &'a AtomicBool) -> Self {
        let (sender, events) = mpsc::channel();
        let subscriber_names = workload
            .topics
            .iter()
            .flat_map(|topic| {
                topic.subscribers.iter().map(|s| format!("{}@{}", topic.name, s.process))
            })
            .collect();
        let heartbeat_nanos = i64::try_from(workload.heartbeat_nanos()).unwrap_or(i64::MAX);
        Self {
            workload,
            domain,
            recording,
            stop,
            sender,
            events,
            workers: Vec::new(),
            recorder: None,
            subscriber_names,
            series: Series::new(SERIES_CAPACITY),
            domain_memory: MemoryTrack::default(),
            domain_error: None,
            times: None,
            heartbeat_nanos,
        }
    }

    fn interval_nanos(&self) -> Result<i64, String> {
        nanos(self.workload.run.sample_interval_s).ok_or_else(|| "sample_interval_s".to_owned())
    }

    /// The plan of each process, and where its subscribers sit in the run's list.
    fn plans(&self) -> Result<Vec<(WorkerPlan, Vec<usize>)>, String> {
        let interval = u64::try_from(self.interval_nanos()?).map_err(|_| "sample_interval_s")?;
        let mut plans = Vec::with_capacity(self.workload.processes.len());
        for process in &self.workload.processes {
            let mut publishers = Vec::new();
            let mut subscribers = Vec::new();
            let mut indexes = Vec::new();
            let mut index: usize = 0;
            for topic in &self.workload.topics {
                if topic.publisher == process.name {
                    let burst = match topic.burst {
                        Some(burst) => {
                            let every = nanos(burst.every_s)
                                .and_then(|n| u64::try_from(n.max(1)).ok())
                                .ok_or_else(|| format!("topic '{}' burst.every_s", topic.name))?;
                            Some((burst.count, every))
                        }
                        None => None,
                    };
                    publishers.push(PublisherPlan {
                        topic: topic.name.clone(),
                        bytes: topic.bytes,
                        period_ns: period_ns(topic.rate_hz),
                        burst,
                    });
                }
                for subscriber in &topic.subscribers {
                    if subscriber.process == process.name {
                        subscribers.push(SubscriberPlan {
                            topic: topic.name.clone(),
                            depth: subscriber.depth,
                            work_ns: subscriber.work_us.saturating_mul(1_000),
                        });
                        indexes.push(index);
                    }
                    index = index.saturating_add(1);
                }
            }
            let plan = WorkerPlan {
                process: process.name.clone(),
                publishers,
                subscribers,
                sample_interval_ns: interval,
            };
            plans.push((plan, indexes));
        }
        Ok(plans)
    }

    fn executable() -> Result<PathBuf, String> {
        std::env::current_exe()
            .and_then(std::fs::canonicalize)
            .map_err(|error| format!("cannot find this executable: {error}"))
    }

    fn command(&self, executable: &Path) -> Command {
        let mut command = Command::new(executable);
        let bounded = if self.workload.run.bounded_memory { "1" } else { "0" };
        command
            .env("HYPHA_DOMAIN", &self.domain)
            .env("HYPHA_BOUNDED_MEMORY", bounded)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        #[cfg(target_os = "linux")]
        {
            // A worker ends with the coordinator, even one that misses its stdin closing.
            if let Err(error) =
                hypha_node::sdk::sys::set_parent_death_signal(&mut command, nix::libc::SIGTERM)
            {
                eprintln!("hypha load: no parent-death signal for a child: {error}");
            }
        }
        command
    }

    fn execute(&mut self) -> Result<(), String> {
        let executable = Self::executable()?;
        let plans = self.plans()?;
        for (plan, subscriber_index) in plans {
            let mut child = self
                .command(&executable)
                .args(["load", "--worker", &plan.process])
                .spawn()
                .map_err(|error| format!("cannot start worker {}: {error}", plan.process))?;
            println!("hypha load: process {} runs as pid {}", plan.process, child.id());
            let _ = std::io::stdout().flush();
            let index = self.workers.len();
            if let Some(stdout) = child.stdout.take() {
                forward_lines(Source::Worker(index), stdout, self.sender.clone());
            }
            let stdin = child.stdin.take();
            let subscribers = plan.subscribers.len();
            self.workers.push(Worker {
                name: plan.process.clone(),
                child,
                stdin,
                stage: None,
                last_heard: steady_now_nanos(),
                published: None,
                result: None,
                latest_p99: vec![None; subscribers],
                subscriber_index,
                memory: MemoryTrack::default(),
            });
            self.tell(index, &ToWorker::Plan { plan })?;
        }
        self.wait_for("the publishers", |session| {
            session.workers.iter().all(|w| w.stage == Some(Stage::Publishers))
        })?;
        self.tell_all(&ToWorker::Subscribe)?;
        self.wait_for("the subscribers", |session| {
            session.workers.iter().all(|w| w.stage == Some(Stage::Subscribers))
        })?;
        if self.workload.recorder_enabled() {
            self.start_recorder(&executable)?;
        }

        let run = &self.workload.run;
        let window = nanos(run.duration_s).ok_or("duration_s is too long")?;
        let warmup = nanos(run.warmup_s).ok_or("warmup_s is too long")?;
        let cooldown = nanos(run.cooldown_s).ok_or("cooldown_s is too long")?;
        let start = steady_now_nanos().saturating_add(START_LEAD_NANOS);
        let window_start = start.saturating_add(warmup);
        let window_end = window_start.saturating_add(window);
        let times = Times {
            start,
            window_start,
            window_end,
            publish_end: window_end.saturating_add(cooldown),
        };
        self.times = Some(times);
        for worker in &mut self.workers {
            worker.last_heard = steady_now_nanos();
        }
        self.tell_all(&ToWorker::Start { times })?;
        self.measure(times)?;

        let drained = steady_now_nanos().saturating_add(DRAIN_NANOS);
        self.pump_until(drained, || false)?;
        self.tell_all(&ToWorker::Stop)?;
        self.wait_for("the results", |session| session.workers.iter().all(|w| w.result.is_some()))?;
        self.stop_recorder()?;
        self.wait_for_exits()
    }

    fn tell(&mut self, index: usize, line: &ToWorker) -> Result<(), String> {
        let text = encode(line)?;
        let worker = self.workers.get_mut(index).ok_or("no such worker")?;
        let name = worker.name.clone();
        let stdin = worker.stdin.as_mut().ok_or_else(|| format!("worker {name} has no stdin"))?;
        writeln!(stdin, "{text}")
            .and_then(|()| stdin.flush())
            .map_err(|error| format!("cannot write to worker {name}: {error}"))
    }

    fn tell_all(&mut self, line: &ToWorker) -> Result<(), String> {
        (0..self.workers.len()).try_for_each(|index| self.tell(index, line))
    }

    fn start_recorder(&mut self, executable: &Path) -> Result<(), String> {
        let compression = self.workload.compression().map_err(|error| error.to_string())?;
        let compression = match compression {
            crate::recorder::McapCompression::Lz4 => "lz4",
            crate::recorder::McapCompression::Zstd => "zstd",
            crate::recorder::McapCompression::None => "none",
        };
        let mut command = self.command(executable);
        command.args(["record", "-c", compression, "-o"]).arg(&self.recording).arg("--topics");
        command.args(self.workload.topics.iter().map(|topic| topic.name.as_str()));
        let mut child =
            command.spawn().map_err(|error| format!("cannot start the recorder: {error}"))?;
        println!("hypha load: the recorder runs as pid {}", child.id());
        let _ = std::io::stdout().flush();
        if let Some(stdout) = child.stdout.take() {
            forward_lines(Source::Recorder, stdout, self.sender.clone());
        }
        // The recorder reads no stdin; closing it changes nothing for it.
        drop(child.stdin.take());
        self.recorder = Some(Recorder {
            child,
            output: self.recording.clone(),
            ready: false,
            ending: false,
            exit: None,
            memory: MemoryTrack::default(),
            recorded: None,
        });
        self.wait_for("the recorder", |session| {
            session.recorder.as_ref().is_some_and(|recorder| recorder.ready)
        })
    }

    /// Handles events until `done` holds, at most [`PHASE_BOUND_NANOS`].
    fn wait_for(&mut self, what: &str, done: impl Fn(&Self) -> bool) -> Result<(), String> {
        let deadline = steady_now_nanos().saturating_add(PHASE_BOUND_NANOS);
        loop {
            if done(self) {
                return Ok(());
            }
            if steady_now_nanos() >= deadline {
                return Err(format!("timed out waiting for {what}"));
            }
            self.next_event(deadline)?;
        }
    }

    /// Handles events until the clock reads `until` or `done` holds.
    fn pump_until(&mut self, until: i64, done: impl Fn() -> bool) -> Result<(), String> {
        while steady_now_nanos() < until && !done() {
            self.next_event(until)?;
        }
        Ok(())
    }

    /// Waits for one event until `deadline`, at most [`MAX_WAIT_NANOS`], and handles it.
    fn next_event(&mut self, deadline: i64) -> Result<(), String> {
        if self.stop.load(Ordering::Relaxed) {
            return Err("interrupted".to_owned());
        }
        let wait = deadline.saturating_sub(steady_now_nanos()).clamp(1_000_000, MAX_WAIT_NANOS);
        let wait = Duration::from_nanos(u64::try_from(wait).unwrap_or(0));
        match self.events.recv_timeout(wait) {
            Ok(event) => self.handle(event),
            Err(RecvTimeoutError::Timeout) => Ok(()),
            Err(RecvTimeoutError::Disconnected) => Err("every child output closed".to_owned()),
        }?;
        self.check_heartbeats()
    }

    fn check_heartbeats(&self) -> Result<(), String> {
        if self.times.is_none() {
            return Ok(());
        }
        let now = steady_now_nanos();
        for worker in self.workers.iter().filter(|worker| worker.result.is_none()) {
            if now.saturating_sub(worker.last_heard) > self.heartbeat_nanos {
                return Err(format!("worker {} sent nothing for too long", worker.name));
            }
        }
        Ok(())
    }

    fn handle(&mut self, event: Event) -> Result<(), String> {
        match event {
            Event::Line(Source::Worker(index), line) => self.handle_worker_line(index, &line),
            Event::Closed(Source::Worker(index)) => {
                let Some(worker) = self.workers.get_mut(index) else { return Ok(()) };
                if worker.result.is_some() {
                    return Ok(());
                }
                // Its stdout closed, so it is ending; a worker that only closed its stdout is
                // not waited for long.
                let grace = i64::try_from(STOP_GRACE.as_nanos()).unwrap_or(i64::MAX);
                let status = wait_bounded(&mut worker.child, grace)
                    .map_or_else(|| "still running".to_owned(), |status| status.to_string());
                Err(format!("worker {} ended before its result ({status})", worker.name))
            }
            Event::Line(Source::Recorder, line) => self.handle_recorder_line(&line),
            Event::Closed(Source::Recorder) => match &self.recorder {
                Some(recorder) if recorder.ending => Ok(()),
                _ => Err("the recorder ended during the run".to_owned()),
            },
        }
    }

    fn handle_worker_line(&mut self, index: usize, line: &str) -> Result<(), String> {
        let Some(worker) = self.workers.get_mut(index) else { return Ok(()) };
        let Some(message) = interpret_worker_line(&worker.name, line)? else {
            eprintln!("{}: {line}", worker.name);
            return Ok(());
        };
        worker.last_heard = steady_now_nanos();
        match message {
            FromWorker::Ready { stage } => worker.stage = Some(stage),
            FromWorker::Sample { subscribers } => {
                for (latest, sample) in worker.latest_p99.iter_mut().zip(subscribers) {
                    *latest = larger(*latest, sample.p99_ns);
                }
            }
            FromWorker::Published { publishers } => worker.published = Some(publishers),
            FromWorker::Result { subscribers, slow_callbacks } => {
                worker.result = Some((subscribers, slow_callbacks));
            }
            // `interpret_worker_line` turned a failure into an error.
            FromWorker::Failed { .. } => {}
        }
        Ok(())
    }

    fn handle_recorder_line(&mut self, line: &str) -> Result<(), String> {
        let Some(recorder) = self.recorder.as_mut() else { return Ok(()) };
        if recorder.ready {
            return Ok(());
        }
        let Some(rest) = line.strip_prefix(RECORDER_READY) else { return Ok(()) };
        let followed: Option<usize> = rest.split_whitespace().next().and_then(|n| n.parse().ok());
        let topics = self.workload.topics.len();
        if followed != Some(topics) {
            return Err(format!("the recorder follows {rest}, not the run's {topics} topics"));
        }
        recorder.ready = true;
        Ok(())
    }

    /// Runs the measuring phases: samples memory at each interval and at the window's
    /// start, until every worker reported its publishers.
    fn measure(&mut self, times: Times) -> Result<(), String> {
        let interval = self.interval_nanos()?.max(1);
        let deadline = times.publish_end.saturating_add(PHASE_BOUND_NANOS);
        let mut next_point = times.start;
        let mut window_sampled = false;
        loop {
            if self.workers.iter().all(|worker| worker.published.is_some()) {
                return Ok(());
            }
            let now = steady_now_nanos();
            if now >= deadline {
                return Err("timed out waiting for the publishers to end".to_owned());
            }
            if !window_sampled && now >= times.window_start {
                self.sample_window_start();
                window_sampled = true;
            }
            if now >= next_point {
                self.add_point(now, times, window_sampled);
                while next_point <= now {
                    next_point = next_point.saturating_add(interval);
                }
            }
            let wake = if window_sampled { next_point } else { next_point.min(times.window_start) };
            self.next_event(wake)?;
        }
    }

    /// The pids of every process, workers first.
    fn pids(&self) -> Vec<u32> {
        let workers = self.workers.iter().map(|worker| worker.child.id());
        workers.chain(self.recorder.iter().map(|recorder| recorder.child.id())).collect()
    }

    fn read_memory(&mut self) -> (Vec<MemoryPoint>, Option<u64>) {
        let memory = self
            .pids()
            .into_iter()
            .map(|pid| {
                let read = i32::try_from(pid).ok().and_then(proc_usage::memory);
                MemoryPoint {
                    rss: read.map(|memory| memory.rss_bytes),
                    shmem: read.and_then(|memory| memory.shared_bytes),
                }
            })
            .collect();
        let domain = match domain_shared_memory(&self.domain) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.domain_error = Some(error);
                None
            }
        };
        (memory, domain)
    }

    fn tracks(&mut self) -> Vec<&mut MemoryTrack> {
        let workers = self.workers.iter_mut().map(|worker| &mut worker.memory);
        workers.chain(self.recorder.iter_mut().map(|recorder| &mut recorder.memory)).collect()
    }

    fn sample_window_start(&mut self) {
        let (memory, domain) = self.read_memory();
        for (track, now) in self.tracks().into_iter().zip(memory) {
            track.at_window_start = Some(now);
            track.peak_with(now);
        }
        self.domain_memory.at_window_start = Some(MemoryPoint { rss: None, shmem: domain });
        self.domain_memory.peak.shmem = larger(self.domain_memory.peak.shmem, domain);
    }

    fn add_point(&mut self, now: i64, times: Times, in_window: bool) {
        let (memory, domain) = self.read_memory();
        let in_window = in_window && now <= times.publish_end;
        if in_window {
            for (track, point) in self.tracks().into_iter().zip(&memory) {
                track.peak_with(*point);
            }
            self.domain_memory.peak.shmem = larger(self.domain_memory.peak.shmem, domain);
        }
        let mut p99_ns = vec![None; self.subscriber_names.len()];
        for worker in &mut self.workers {
            for (latest, index) in worker.latest_p99.iter_mut().zip(&worker.subscriber_index) {
                if let Some(slot) = p99_ns.get_mut(*index) {
                    *slot = latest.take();
                }
            }
        }
        let t_ms = now.saturating_sub(times.start).checked_div(1_000_000).unwrap_or(0);
        self.series.push(SeriesPoint { t_ms, memory, domain_shm: domain, p99_ns });
    }

    fn stop_recorder(&mut self) -> Result<(), String> {
        let Some(recorder) = self.recorder.as_mut() else { return Ok(()) };
        recorder.ending = true;
        let pid = i32::try_from(recorder.child.id()).map_err(|_| "recorder pid")?;
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), nix::sys::signal::Signal::SIGTERM)
            .map_err(|error| format!("cannot stop the recorder: {error}"))?;
        let status = wait_bounded(&mut recorder.child, PHASE_BOUND_NANOS)
            .ok_or("the recorder did not end after SIGTERM")?;
        recorder.exit = Some(status);
        recorder.recorded =
            Some(recorder_outcome(status.code(), count_recorded(&recorder.output))?);
        Ok(())
    }

    fn wait_for_exits(&mut self) -> Result<(), String> {
        for worker in &mut self.workers {
            drop(worker.stdin.take());
            let status = wait_bounded(&mut worker.child, PHASE_BOUND_NANOS)
                .ok_or_else(|| format!("worker {} did not end", worker.name))?;
            if !status.success() {
                return Err(format!("worker {} ended with {status}", worker.name));
            }
        }
        Ok(())
    }

    /// Ends every child that still runs: its stdin closes, then SIGTERM, then a kill.
    fn end_children(&mut self) {
        let mut children: Vec<&mut Child> = Vec::new();
        for worker in &mut self.workers {
            drop(worker.stdin.take());
            children.push(&mut worker.child);
        }
        children.extend(self.recorder.iter_mut().map(|recorder| &mut recorder.child));
        for child in children {
            if matches!(child.try_wait(), Ok(Some(_))) {
                continue;
            }
            if let Ok(pid) = i32::try_from(child.id()) {
                let pid = nix::unistd::Pid::from_raw(pid);
                let _ = nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGTERM);
            }
            let grace = i64::try_from(STOP_GRACE.as_nanos()).unwrap_or(i64::MAX);
            if wait_bounded(child, grace).is_none() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    fn not_measured(&self) -> Vec<NotMeasured> {
        let mut missing = Vec::new();
        if !cfg!(target_os = "linux") {
            missing.push(NotMeasured {
                quantity: "processes[].shmem_at_window_start, processes[].shmem_peak, series.points[].memory[].shmem".into(),
                reason: SHARED_MEMORY_UNAVAILABLE.into(),
            });
        }
        let domain_reason = self.domain_error.clone().or_else(|| {
            (!cfg!(target_os = "linux")).then(|| {
                "only Linux tells the written part of a shared-memory object (allocated blocks of /dev/shm)".to_owned()
            })
        });
        if let Some(reason) = domain_reason {
            missing.push(NotMeasured {
                quantity: "domain_shared_memory, series.points[].domain_shm".into(),
                reason,
            });
        }
        missing
    }

    /// The workers in workload order, then the recorder.
    fn process_reports(&self) -> Vec<ProcessReport> {
        let process =
            |name: &str, pid: u32, memory: &MemoryTrack, slow: Option<u64>| ProcessReport {
                name: name.to_owned(),
                pid,
                rss_at_window_start: memory.at_window_start.and_then(|start| start.rss),
                rss_peak: memory.peak.rss,
                shmem_at_window_start: memory.at_window_start.and_then(|start| start.shmem),
                shmem_peak: memory.peak.shmem,
                slow_callbacks: slow,
            };
        let mut processes: Vec<ProcessReport> = self
            .workers
            .iter()
            .map(|worker| {
                let slow = worker.result.as_ref().map(|(_, slow)| *slow);
                process(&worker.name, worker.child.id(), &worker.memory, slow)
            })
            .collect();
        processes.extend(self.recorder.iter().map(|recorder| {
            process(RECORDER_PROCESS, recorder.child.id(), &recorder.memory, None)
        }));
        processes
    }

    fn report(&self, config: Config, failure: Option<String>) -> Report {
        let mut results: BTreeMap<&str, &PublisherResult> = BTreeMap::new();
        for worker in &self.workers {
            for published in worker.published.iter().flatten() {
                results.insert(published.topic.as_str(), published);
            }
        }
        let worker_of = |name: &str| self.workers.iter().find(|worker| worker.name == name);
        let topics = self
            .workload
            .topics
            .iter()
            .map(|topic| TopicReport {
                name: topic.name.clone(),
                bytes: topic.bytes,
                rate_hz: match topic.rate_hz {
                    Rate::Hz(hz) => Some(hz),
                    Rate::Max(_) => None,
                },
                publisher: topic.publisher.clone(),
                publish: results.get(topic.name.as_str()).map(|result| (*result).clone()),
                subscribers: topic
                    .subscribers
                    .iter()
                    .map(|subscriber| SubscriberReport {
                        process: subscriber.process.clone(),
                        result: worker_of(&subscriber.process)
                            .and_then(|worker| worker.result.as_ref())
                            .and_then(|(results, _)| {
                                results.iter().find(|result| result.topic == topic.name)
                            })
                            .cloned(),
                    })
                    .collect(),
            })
            .collect();
        let processes = self.process_reports();
        let recorder = self.recorder.as_ref().map(|recorder| RecorderReport {
            output: recorder.output.display().to_string(),
            exit_code: recorder.exit.and_then(|status| status.code()),
            topics: recorder
                .recorded
                .iter()
                .flat_map(|recorded| {
                    let results = &results;
                    self.workload.topics.iter().map(move |topic| {
                        let count = recorded.get(&topic.name).copied().unwrap_or(0);
                        let sent = results.get(topic.name.as_str()).map_or(0, |r| r.sent_total);
                        RecordedTopic {
                            topic: topic.name.clone(),
                            recorded: count,
                            unrecorded: sent.saturating_sub(count),
                        }
                    })
                })
                .collect(),
        });
        let interval_ms =
            nanos(self.workload.run.sample_interval_s).unwrap_or(0).checked_div(1_000_000);
        let point_ms = interval_ms
            .and_then(|ms| u64::try_from(ms).ok())
            .unwrap_or(0)
            .saturating_mul(self.series.stride());
        Report {
            schema: SCHEMA.to_owned(),
            outcome: if failure.is_some() { Outcome::RunFailed } else { Outcome::Pass },
            reason: failure,
            host: host(),
            config,
            not_measured: self.not_measured(),
            topics,
            processes,
            domain_shared_memory: DomainMemory {
                at_window_start: self.domain_memory.at_window_start.and_then(|start| start.shmem),
                peak: self.domain_memory.peak.shmem,
            },
            recorder,
            series: SeriesReport {
                point_ms,
                subscribers: self.subscriber_names.clone(),
                points: self.series.points(),
            },
            limits: Vec::new(),
        }
    }
}

/// A worker's stdout line: `Ok(None)` for a line that is no protocol line, an error for
/// a malformed line or a worker's `Failed`.
///
/// # Errors
/// What the line says went wrong, naming `worker`.
pub fn interpret_worker_line(worker: &str, line: &str) -> Result<Option<FromWorker>, String> {
    match decode_worker_line(line) {
        Ok(Some(FromWorker::Failed { reason })) => Err(format!("worker {worker} failed: {reason}")),
        Ok(message) => Ok(message),
        Err(error) => Err(format!("worker {worker}: {error}")),
    }
}

/// The recorder's end: its exit code (`None` for a signal) and the messages per topic of
/// its recording. Exit 0, or exit 1 (it lost samples) with a readable recording, is a
/// recording whose losses `max_unrecorded` judges; any other end fails the run.
///
/// # Errors
/// Why the recorder's end fails the run.
pub fn recorder_outcome(
    code: Option<i32>,
    recorded: Result<BTreeMap<String, u64>, String>,
) -> Result<BTreeMap<String, u64>, String> {
    match code {
        Some(0 | 1) => recorded.map_err(|error| format!("the recorder's recording: {error}")),
        Some(code) => Err(format!("the recorder failed with exit code {code}")),
        None => Err("the recorder ended by a signal".to_owned()),
    }
}

/// Waits at most `bound_nanos` for `child` to end.
fn wait_bounded(child: &mut Child, bound_nanos: i64) -> Option<ExitStatus> {
    let deadline = steady_now_nanos().saturating_add(bound_nanos);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if steady_now_nanos() < deadline => thread::sleep(Duration::from_millis(10)),
            _ => return None,
        }
    }
}

/// The messages per topic of the recording at `path`, from its summary.
fn count_recorded(path: &Path) -> Result<BTreeMap<String, u64>, String> {
    let cannot =
        |cause: &dyn std::fmt::Display| format!("cannot read '{}': {cause}", path.display());
    let file = std::fs::File::open(path).map_err(|error| cannot(&error))?;
    let map = hypha_node::sdk::sys::map_read_only(&file).map_err(|error| cannot(&error))?;
    let summary = mcap::Summary::read(&map)
        .map_err(|error| cannot(&error))?
        .ok_or_else(|| cannot(&"the recording has no summary"))?;
    let statistics = summary.stats.ok_or_else(|| cannot(&"the recording has no statistics"))?;
    let mut counts = BTreeMap::new();
    for (channel, count) in statistics.channel_message_counts {
        if let Some(channel) = summary.channels.get(&channel) {
            let entry = counts.entry(channel.topic.clone()).or_insert(0_u64);
            *entry = entry.saturating_add(count);
        }
    }
    Ok(counts)
}

/// One word of `uname`, such as `-n` for the host name.
fn uname(flag: &str) -> Option<String> {
    let output = Command::new("uname").arg(flag).stderr(Stdio::null()).output().ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim();
    (output.status.success() && !text.is_empty()).then(|| text.to_owned())
}

fn host() -> Host {
    Host {
        hostname: uname("-n"),
        os: std::env::consts::OS.to_owned(),
        os_release: uname("-r"),
        arch: std::env::consts::ARCH.to_owned(),
        cpus: thread::available_parallelism().ok().map(std::num::NonZeroUsize::get),
        hypha_version: env!("CARGO_PKG_VERSION").to_owned(),
        build: if cfg!(debug_assertions) { "debug" } else { "release" }.to_owned(),
    }
}

/// Microseconds of a nanosecond value, rounded up, for the summary.
fn micros(nanos: u64) -> u64 {
    nanos.div_ceil(1_000)
}

fn print_summary(report: &Report, path: &Path) {
    let verdict = match report.outcome {
        Outcome::Pass => "pass".to_owned(),
        Outcome::LimitFailed => "LIMIT FAILED".to_owned(),
        Outcome::RunFailed => {
            format!("RUN FAILED: {}", report.reason.as_deref().unwrap_or("unknown"))
        }
    };
    println!("hypha load: {verdict}");
    let workload = &report.config.workload;
    println!(
        "  {} s window, {} topic(s), {} process(es), domain {}, {} build on {}",
        workload.run.duration_s,
        workload.topics.len(),
        workload.processes.len(),
        report.config.domain,
        report.host.build,
        report.host.os,
    );
    let mut worst: Option<(u64, String)> = None;
    let mut loss: u64 = 0;
    for topic in &report.topics {
        for subscriber in &topic.subscribers {
            let Some(result) = &subscriber.result else { continue };
            if let Some(p99) = result.latency.p99_ns
                && worst.as_ref().is_none_or(|(known, _)| p99 > *known)
            {
                worst = Some((p99, format!("{}@{}", topic.name, subscriber.process)));
            }
            loss = loss.saturating_add(limits::loss(topic.publish.as_ref(), result).unwrap_or(0));
        }
    }
    if let Some((p99, scope)) = worst {
        println!("  worst p99 latency: {} us ({scope})", micros(p99));
    }
    let refused: u64 = report
        .topics
        .iter()
        .filter_map(|topic| topic.publish.as_ref())
        .map(|publish| publish.refused)
        .fold(0, u64::saturating_add);
    println!("  frames lost: {loss}, publishes refused: {refused}");
    if let Some(peak) = report.processes.iter().filter_map(|process| process.rss_peak).max() {
        println!("  largest resident memory: {}", proc_usage::format_memory(peak));
    }
    for limit in report.limits.iter().filter(|limit| !limit.pass) {
        let measured = limit.measured.map_or_else(|| "not measured".to_owned(), |v| v.to_string());
        println!("  limit failed: {} on {}: {measured} > {}", limit.key, limit.scope, limit.limit);
    }
    println!("  report: {}", path.display());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::load::protocol::{LINE_PREFIX, Stage};

    #[test]
    fn a_failed_or_malformed_worker_line_fails_the_run_and_a_foreign_line_does_not() {
        let failed = format!("{LINE_PREFIX}{{\"kind\":\"failed\",\"reason\":\"no bus\"}}");
        assert_eq!(interpret_worker_line("w", &failed), Err("worker w failed: no bus".into()));
        let broken = format!("{LINE_PREFIX}{{\"kind\":\"ready\"}}");
        let error = interpret_worker_line("w", &broken).err().unwrap_or_default();
        assert!(error.starts_with("worker w: malformed line"), "{error}");
        assert_eq!(interpret_worker_line("w", "[WARN] a log line"), Ok(None));
        let ready = format!("{LINE_PREFIX}{{\"kind\":\"ready\",\"stage\":\"publishers\"}}");
        assert_eq!(
            interpret_worker_line("w", &ready),
            Ok(Some(FromWorker::Ready { stage: Stage::Publishers }))
        );
    }

    #[test]
    fn the_recorder_fails_the_run_unless_it_ends_with_0_or_1_and_a_readable_recording() {
        let counts = BTreeMap::from([("/t".to_owned(), 5_u64)]);
        assert_eq!(recorder_outcome(Some(0), Ok(counts.clone())), Ok(counts.clone()));
        assert_eq!(recorder_outcome(Some(1), Ok(counts.clone())), Ok(counts.clone()));
        assert!(recorder_outcome(Some(1), Err("no summary".into())).is_err());
        assert!(recorder_outcome(Some(0), Err("no summary".into())).is_err());
        assert!(recorder_outcome(Some(2), Ok(counts.clone())).is_err());
        assert!(recorder_outcome(None, Ok(counts)).is_err(), "ended by a signal");
    }
}
