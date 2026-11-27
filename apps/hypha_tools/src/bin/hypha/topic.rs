//! `hypha topic` -- rostopic-equivalent bus introspection: list, echo, hz, and bw.

use crate::cli::{connect, parse_cli};
use clap::{Parser, Subcommand};
use hypha_node::sdk::SchemaIdentityCache;
use hypha_node::sdk::{BusNode, RawSubscriber, ReceivedSample, ResolvedSchema};
use hypha_tools::echo_args::EchoArgs;
use hypha_tools::msg_plot::{plot_header, plot_row, plot_values};
use hypha_tools::msg_reflect::{SchemaCache, decode_message, format_significant};
use hypha_tools::stream::{StreamKey, StreamSelector, stream_label};
use hypha_tools::{approx_f64, approx_f64_usize};
use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, VecDeque};
use std::process::ExitCode;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

mod follow;

use follow::Followed;

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
    /// Topics seen publishing while the tool listens: one line for each type of a topic.
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
    /// Publish rate of each stream of one topic.
    Hz {
        /// Topic to measure.
        topic: String,
        /// Messages in the averaging window.
        #[arg(long, value_name = "n", default_value_t = 100)]
        window: usize,
        #[command(flatten)]
        selector: StreamSelector,
    },
    /// Bandwidth and mean message size of each stream of one topic.
    Bw {
        /// Topic to measure.
        topic: String,
        #[command(flatten)]
        selector: StreamSelector,
    },
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
            if let Entry::Vacant(stream) = topic_types.entry((sample.topic, sample.schema_hash)) {
                let (topic, schema_hash) = stream.key();
                let label = schema_label(bus, &identities, topic, *schema_hash);
                stream.insert(label);
            }
        }
        thread::sleep(Duration::from_millis(5));
    }

    if topic_types.is_empty() {
        let timeout_text = format_significant(timeout_sec);
        println!("(no traffic seen in {timeout_text}s)");
    } else {
        for ((topic, _), label) in &topic_types {
            println!("{topic}  [{label}]");
        }
    }

    ExitCode::SUCCESS
}

fn run_echo(bus: &BusNode, echo: &EchoArgs) -> ExitCode {
    let max_count = echo.max_count;
    let mut followed = match Followed::open(bus, &echo.topic, &echo.selector, "echo") {
        Ok(followed) => followed,
        Err(code) => return code,
    };

    let mut cache = SchemaCache::new("hypha_topic");
    let identities = SchemaIdentityCache::new();
    let mut count = 0usize;
    // The stream whose header the last row of `-p` followed: a row has the columns of the
    // header above it, so the header is printed again when the stream changes.
    let mut header_of: Option<StreamKey> = None;

    while bus.ok() && (max_count == 0 || count < max_count) {
        followed.refresh();
        if let Some(received) = followed.next() {
            let Some(sample) = received.to_raw_sample() else { continue };
            count = count.saturating_add(1);
            let stream = StreamKey { topic: sample.topic.clone(), schema_hash: sample.schema_hash };
            let resolved = resolve_identity(bus, &identities, &sample.topic, sample.schema_hash);
            let label = followed
                .is_shared(&stream)
                .then(|| stream_label(resolved.as_deref(), sample.schema_hash));
            if !echo.plot {
                println!("---");
                print_label(label.as_deref());
            }
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
                if header_of.as_ref() != Some(&stream) {
                    print_label(label.as_deref());
                    println!("{}", plot_header(&columns));
                    header_of = Some(stream);
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

/// The comment line that says which stream the lines after it are of, for a topic that has
/// several; a topic of one stream prints none, so its output is what it always was.
fn print_label(label: Option<&str>) {
    if let Some(label) = label {
        println!("# {label}");
    }
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

fn run_hz(bus: &BusNode, topic: &str, window: usize, selector: &StreamSelector) -> ExitCode {
    let mut followed = match Followed::open(bus, topic, selector, "hz") {
        Ok(followed) => followed,
        Err(code) => return code,
    };

    let mut rates: BTreeMap<StreamKey, RateWindow> = BTreeMap::new();
    let mut last_report = Instant::now();

    while bus.ok() {
        followed.refresh();
        while let Some(received) = followed.next() {
            let Some(sample) = received.to_raw_sample() else { continue };
            let stream = StreamKey { topic: sample.topic, schema_hash: sample.schema_hash };
            rates
                .entry(stream)
                .or_insert_with(|| RateWindow::new(window))
                .on_arrival(sample.transport_sent_nanos);
        }

        if last_report.elapsed() >= Duration::from_secs(1) {
            last_report = Instant::now();
            report_streams(&followed, &rates, RateWindow::has_data, RateWindow::print_report);
            if !rates.values().any(RateWindow::has_data) {
                println!("no messages received yet on '{topic}'");
            }
        }
        thread::sleep(Duration::from_millis(5));
    }
    ExitCode::SUCCESS
}

/// Prints the report of each stream that `has_data`, each under its title when the topic has
/// several streams.
fn report_streams<W>(
    followed: &Followed<'_>,
    windows: &BTreeMap<StreamKey, W>,
    has_data: impl Fn(&W) -> bool,
    print: impl Fn(&W),
) {
    for (stream, window) in windows.iter().filter(|(_, window)| has_data(window)) {
        if followed.is_shared(stream) {
            println!("{}:", followed.title(stream));
        }
        print(window);
    }
}

/// What `bw` says before it has two messages of a stream, or before there is a stream.
const NOT_ENOUGH_DATA: &str = "not enough data yet";

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
            println!("{NOT_ENOUGH_DATA}");
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

fn run_bw(bus: &BusNode, topic: &str, selector: &StreamSelector) -> ExitCode {
    let mut followed = match Followed::open(bus, topic, selector, "bw") {
        Ok(followed) => followed,
        Err(code) => return code,
    };

    let mut windows: BTreeMap<StreamKey, BandwidthWindow> = BTreeMap::new();
    let mut last_report = Instant::now();

    while bus.ok() {
        followed.refresh();
        while let Some(received) = followed.next() {
            let Some(sample) = received.to_raw_sample() else { continue };
            let stream = StreamKey { topic: sample.topic, schema_hash: sample.schema_hash };
            windows
                .entry(stream)
                .or_insert_with(|| BandwidthWindow::new(5))
                .on_sample(sample.bytes.len());
        }

        if last_report.elapsed() >= Duration::from_secs(1) {
            last_report = Instant::now();
            if windows.is_empty() {
                println!("{NOT_ENOUGH_DATA}");
            }
            report_streams(&followed, &windows, |_| true, BandwidthWindow::print_report);
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
        Verb::Hz { topic, window, selector } => run_hz(&bus, &topic, window, &selector),
        Verb::Bw { topic, selector } => run_bw(&bus, &topic, &selector),
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

/// The schema behind a frame's schema hash as `hypha topic list` shows it: its type and
/// the first eight hex digits of its MD5, so two versions of one type are two lines. An
/// unresolved schema shows its hash.
fn schema_label(
    bus: &BusNode,
    identities: &SchemaIdentityCache,
    topic: &str,
    schema_hash: u64,
) -> String {
    stream_label(resolve_identity(bus, identities, topic, schema_hash).as_deref(), schema_hash)
}
