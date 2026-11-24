//! `hypha topic` -- rostopic-equivalent bus introspection: list, echo, hz, and bw.

use crate::cli::{connect, parse_cli};
use clap::{Parser, Subcommand};
use hypha_node::sdk::SchemaIdentityCache;
use hypha_node::sdk::{BusNode, RawSubscriber, ReceivedSample, ResolvedSchema};
use hypha_tools::echo_args::EchoArgs;
use hypha_tools::msg_plot::{plot_header, plot_row, plot_values};
use hypha_tools::msg_reflect::{SchemaCache, decode_message, format_significant};
use hypha_tools::{approx_f64, approx_f64_usize};
use std::collections::{BTreeMap, VecDeque};
use std::process::ExitCode;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

/// Bus introspection: list, echo, hz, and bw.
#[derive(Debug, Parser)]
#[command(
    name = "hypha topic",
    after_help = "A topic that has never published during this run will not appear in `list`, \
                  and `hz`/`bw` cannot measure a topic that is not currently publishing: there \
                  is no separate topic registry on this bus, only observed traffic."
)]
struct Cli {
    #[command(subcommand)]
    verb: Verb,
}

#[derive(Debug, Subcommand)]
enum Verb {
    /// Topics seen publishing while the tool listens.
    List {
        /// How long to listen, in seconds.
        #[arg(long, value_name = "seconds", default_value_t = 1.0)]
        timeout: f64,
    },
    /// Print each message, decoded; -p: one CSV row each.
    Echo {
        #[command(flatten)]
        echo: EchoArgs,
    },
    /// Publish rate of one topic.
    Hz {
        /// Topic to measure.
        topic: String,
        /// Messages in the averaging window.
        #[arg(long, value_name = "n", default_value_t = 100)]
        window: usize,
    },
    /// Bandwidth and mean message size of one topic.
    Bw {
        /// Topic to measure.
        topic: String,
    },
}

/// Subscribe to `topic` once a publisher or a typed subscriber has opened it, at the
/// depth of its service policy. `None` when a stop (SIGINT or SIGTERM) came first.
fn subscribe_when_open(
    bus: &BusNode,
    topic: &str,
    command: &str,
) -> Result<Option<RawSubscriber>, String> {
    let mut announced = false;
    while bus.ok() {
        let listed = bus.list_topics()?;
        let is_open = listed.iter().any(|entry| entry.name == topic);
        if is_open {
            return bus.subscribe_raw(topic, 0).map(Some);
        }
        if !announced {
            eprintln!("hypha topic {command}: waiting for '{topic}' to be published");
            announced = true;
        }
        thread::sleep(Duration::from_millis(200));
    }
    Ok(None)
}

/// `subscribe_when_open` for the subcommand `label`, printing its failure; `Err` carries
/// the exit code: success after a stop, failure after an error.
fn open_subscriber(
    bus: &BusNode,
    topic: &str,
    command: &str,
    label: &str,
) -> Result<RawSubscriber, ExitCode> {
    match subscribe_when_open(bus, topic, command) {
        Ok(Some(sub)) => Ok(sub),
        Ok(None) => Err(ExitCode::SUCCESS),
        Err(error) => {
            eprintln!("hypha topic {label}: {error}");
            Err(ExitCode::FAILURE)
        }
    }
}

/// The next sample of `sub`; a receive error is printed for the subcommand `label` and
/// counts as no sample.
fn receive_or_report(sub: &mut RawSubscriber, label: &str) -> Option<ReceivedSample> {
    sub.try_receive().unwrap_or_else(|error| {
        eprintln!("hypha topic {label}: {error}");
        None
    })
}

fn run_list(bus: &BusNode, timeout_sec: f64) -> ExitCode {
    let mut sub = match bus.subscribe_raw("**", 0) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("hypha topic list: {e}");
            return ExitCode::FAILURE;
        }
    };

    let mut topic_types = BTreeMap::new();
    let deadline = Instant::now()
        .checked_add(Duration::from_secs_f64(timeout_sec))
        .unwrap_or_else(Instant::now);

    let identities = SchemaIdentityCache::new();
    while bus.ok() && Instant::now() < deadline {
        while let Some(received) = receive_or_report(&mut sub, "list") {
            let Some(sample) = received.to_raw_sample() else { continue };
            let data_type = resolve_data_type(bus, &identities, &sample.topic, sample.schema_hash);
            topic_types.insert(sample.topic, data_type);
        }
        thread::sleep(Duration::from_millis(5));
    }

    if topic_types.is_empty() {
        let timeout_text = format_significant(timeout_sec);
        println!("(no traffic seen in {timeout_text}s)");
    } else {
        for (topic, data_type) in &topic_types {
            println!("{topic}  [{data_type}]");
        }
    }

    ExitCode::SUCCESS
}

fn run_echo(bus: &BusNode, echo: &EchoArgs) -> ExitCode {
    let max_count = echo.max_count;
    let mut sub = match open_subscriber(bus, &echo.topic, "echo", "echo") {
        Ok(sub) => sub,
        Err(code) => return code,
    };

    let mut cache = SchemaCache::new("hypha_topic");
    let identities = SchemaIdentityCache::new();
    let mut count = 0usize;
    let mut header_printed = false;

    while bus.ok() && (max_count == 0 || count < max_count) {
        if let Some(received) = receive_or_report(&mut sub, "echo") {
            let Some(sample) = received.to_raw_sample() else { continue };
            count = count.saturating_add(1);
            if !echo.plot {
                println!("---");
            }
            let resolved = resolve_identity(bus, &identities, &sample.topic, sample.schema_hash);
            let Some(identity) = resolved else {
                let note = format!("<schema not yet resolved, {} bytes>", sample.bytes.len());
                report_unprinted(echo.plot, &note);
                continue;
            };
            let data_type = identity.data_type.as_str();

            let schema_opt =
                cache.get(bus, &sample.topic, data_type, &identity.md5).and_then(Result::ok);
            let Some(schema) = schema_opt else {
                let note =
                    format!("<{data_type}: schema not yet resolved, {} bytes>", sample.bytes.len());
                report_unprinted(echo.plot, &note);
                continue;
            };

            if echo.plot {
                let columns = match plot_values(data_type, schema, &sample.bytes) {
                    Ok(columns) => columns,
                    Err(cause) => {
                        eprintln!(
                            "hypha topic echo: {data_type}: payload does not match its schema: {cause}"
                        );
                        continue;
                    }
                };
                if !header_printed {
                    println!("{}", plot_header(&columns));
                    header_printed = true;
                }
                println!("{}", plot_row(sample.framework_sent_nanos, &columns));
                continue;
            }

            let mut out = String::new();
            decode_message(&mut out, data_type, schema, &sample.bytes);
            print!("{out}");
        } else {
            thread::sleep(Duration::from_millis(2));
        }
    }

    ExitCode::SUCCESS
}

/// A message that cannot print goes to stderr in `-p` mode, so stdout stays a clean table.
fn report_unprinted(plot: bool, note: &str) {
    if plot {
        eprintln!("hypha topic echo: {note}");
    } else {
        println!("{note}");
    }
}

struct RateWindow {
    window_size: usize,
    gaps: VecDeque<f64>,
    last_arrival_nanos: Option<u64>,
}

impl RateWindow {
    fn new(window_size: usize) -> Self {
        Self { window_size, gaps: VecDeque::new(), last_arrival_nanos: None }
    }

    /// Records one arrival at the sender's own wire timestamp, not the local
    /// receive time.
    fn on_arrival(&mut self, stamp_nanos: u64) {
        if let Some(last_nanos) = self.last_arrival_nanos {
            let magnitude = approx_f64(stamp_nanos.abs_diff(last_nanos)) / 1_000_000_000.0;
            let gap_sec = if stamp_nanos >= last_nanos { magnitude } else { -magnitude };
            self.gaps.push_back(gap_sec);
            while self.gaps.len() > self.window_size {
                self.gaps.pop_front();
            }
        }
        self.last_arrival_nanos = Some(stamp_nanos);
    }

    fn has_data(&self) -> bool {
        !self.gaps.is_empty()
    }

    fn print_report(&self) {
        let count = self.gaps.len();
        if count == 0 {
            return;
        }

        let sum: f64 = self.gaps.iter().copied().sum();
        let mut min_gap = self.gaps.front().copied().unwrap_or(0.0);
        let mut max_gap = min_gap;
        for &gap in &self.gaps {
            min_gap = min_gap.min(gap);
            max_gap = max_gap.max(gap);
        }

        let mean_gap = sum / approx_f64_usize(count);
        let variance: f64 = self
            .gaps
            .iter()
            .map(|&gap| {
                let d = gap - mean_gap;
                d * d
            })
            .sum::<f64>()
            / approx_f64_usize(count);

        let std_dev = variance.sqrt();
        let hz = if mean_gap > 0.0 { 1.0 / mean_gap } else { 0.0 };

        println!(
            "average rate: {hz:.3} Hz
	min: {min_gap:.4}s max: {max_gap:.4}s std dev: {std_dev:.5}s window: {}",
            count.saturating_add(1)
        );
    }
}

fn run_hz(bus: &BusNode, topic: &str, window: usize) -> ExitCode {
    let mut sub = match open_subscriber(bus, topic, "hz/bw", "hz") {
        Ok(sub) => sub,
        Err(code) => return code,
    };

    let mut rate = RateWindow::new(window);
    let mut last_report = Instant::now();

    while bus.ok() {
        while let Some(received) = receive_or_report(&mut sub, "hz") {
            let Some(sample) = received.to_raw_sample() else { continue };
            rate.on_arrival(sample.transport_sent_nanos);
        }

        if last_report.elapsed() >= Duration::from_secs(1) {
            last_report = Instant::now();
            if rate.has_data() {
                rate.print_report();
            } else {
                println!("no messages received yet on '{topic}'");
            }
        }
        thread::sleep(Duration::from_millis(5));
    }
    ExitCode::SUCCESS
}

struct BandwidthWindow {
    samples: VecDeque<(Instant, usize)>,
    window_sec: u64,
}

impl BandwidthWindow {
    fn new(window_sec: u64) -> Self {
        Self { samples: VecDeque::new(), window_sec }
    }

    fn on_sample(&mut self, size: usize) {
        let now = Instant::now();
        self.samples.push_back((now, size));
        let cutoff = now.checked_sub(Duration::from_secs(self.window_sec)).unwrap_or(now);
        while let Some(&(time, _)) = self.samples.front() {
            if time < cutoff {
                self.samples.pop_front();
            } else {
                break;
            }
        }
    }

    fn print_report(&self) {
        if self.samples.len() < 2 {
            println!("not enough data yet");
            return;
        }

        let total_bytes: usize = self.samples.iter().map(|&(_, s)| s).sum();
        let first_time = self.samples.front().map_or_else(Instant::now, |&(t, _)| t);
        let last_time = self.samples.back().map_or_else(Instant::now, |&(t, _)| t);
        let elapsed_sec = last_time.duration_since(first_time).as_secs_f64();

        let total = approx_f64_usize(total_bytes);
        let bytes_per_sec = if elapsed_sec > 0.0 { total / elapsed_sec } else { 0.0 };
        let mean_size = total / approx_f64_usize(self.samples.len());

        println!(
            "average: {:.2} KB/s
	mean size: {mean_size:.0} bytes  window: {} messages, {elapsed_sec:.1}s",
            bytes_per_sec / 1024.0,
            self.samples.len()
        );
    }
}

fn run_bw(bus: &BusNode, topic: &str) -> ExitCode {
    let mut sub = match open_subscriber(bus, topic, "hz/bw", "bw") {
        Ok(sub) => sub,
        Err(code) => return code,
    };

    let mut bw = BandwidthWindow::new(5);
    let mut last_report = Instant::now();

    while bus.ok() {
        while let Some(received) = receive_or_report(&mut sub, "bw") {
            let Some(sample) = received.to_raw_sample() else { continue };
            bw.on_sample(sample.bytes.len());
        }

        if last_report.elapsed() >= Duration::from_secs(1) {
            last_report = Instant::now();
            bw.print_report();
        }
        thread::sleep(Duration::from_millis(5));
    }
    ExitCode::SUCCESS
}

/// Runs the tool; `args` is the whole command line, `args[0]` the name it shows.
pub(crate) fn main(args: Vec<String>) -> ExitCode {
    let cli: Cli = match parse_cli(&args) {
        Ok(cli) => cli,
        Err(code) => return code,
    };
    let bus = match connect("hypha topic: ", "_hyphacli_topic") {
        Ok(bus) => bus,
        Err(code) => return code,
    };

    match cli.verb {
        Verb::List { timeout } => run_list(&bus, timeout),
        Verb::Echo { echo } => run_echo(&bus, &echo),
        Verb::Hz { topic, window } => run_hz(&bus, &topic, window),
        Verb::Bw { topic } => run_bw(&bus, &topic),
    }
}

/// How long a tool waits for a retained schema record before it reports the
/// message as undescribed. The record follows the first data frame.
const SCHEMA_WAIT: Duration = Duration::from_millis(1500);

/// Resolves a frame's type identity, reporting a resolver failure once.
fn resolve_identity(
    bus: &BusNode,
    identities: &SchemaIdentityCache,
    topic: &str,
    schema_hash: u64,
) -> Option<Arc<ResolvedSchema>> {
    let deadline = Instant::now().checked_add(SCHEMA_WAIT).unwrap_or_else(Instant::now);
    match identities.resolve_until(bus, topic, schema_hash, deadline) {
        Ok(identity) => identity,
        Err(cause) => {
            eprintln!("hypha topic: schema lookup failed for '{topic}': {cause}");
            None
        }
    }
}

/// Resolves only the type name behind a frame's schema hash.
fn resolve_data_type(
    bus: &BusNode,
    identities: &SchemaIdentityCache,
    topic: &str,
    schema_hash: u64,
) -> String {
    resolve_identity(bus, identities, topic, schema_hash)
        .map_or_else(String::new, |schema| schema.data_type.clone())
}
