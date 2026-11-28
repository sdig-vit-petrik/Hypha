//! `hypha topic` -- rostopic-equivalent bus introspection: list, echo, hz, delay, and bw.

use crate::cli::{connect, parse_cli};
use clap::{Parser, Subcommand};
use hypha_node::sdk::SchemaIdentityCache;
use hypha_node::sdk::{BusNode, ResolvedSchema, StreamKey, TopicInfo};
use hypha_tools::delay::Delays;
use hypha_tools::echo_args::EchoArgs;
use hypha_tools::msg_plot::{plot_header, plot_row, plot_values};
use hypha_tools::msg_reflect::{SchemaCache, decode_message};
use hypha_tools::stream::{StreamFlags, stream_label, type_label};
use hypha_tools::{approx_f64, approx_f64_usize};
use std::collections::{BTreeMap, VecDeque};
use std::process::ExitCode;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

mod follow;

use follow::Followed;

/// Bus introspection: list, echo, hz, delay, and bw.
#[derive(Debug, Parser)]
#[command(
    name = "hypha topic",
    after_help = "`list` shows a topic from the moment a publisher or a subscriber has opened it, \
                  with no sample sent; `hz` and `bw` cannot measure a topic that is not \
                  currently publishing."
)]
struct Cli {
    #[command(subcommand)]
    verb: Verb,
}

#[derive(Debug, Subcommand)]
enum Verb {
    /// Every topic on the bus, published or not: one line for each type of a topic.
    List {
        /// Also the number of publishers and subscribers of each.
        #[arg(short, long)]
        verbose: bool,
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
        streams: StreamFlags,
    },
    /// Time from an input topic to an output topic, from the frames' send stamps alone.
    #[command(
        after_help = "For each output frame, the time since the newest input frame sent at or \
                      before it, from the host send stamps in the frame headers: no payload is \
                      decoded, so a large cloud costs what a small message costs. Both topics \
                      must be sent on this host. Every stream of each topic counts."
    )]
    Delay {
        /// Topic the node reads.
        input: String,
        /// Topic the node writes.
        output: String,
        /// Outputs in the window of the figures.
        #[arg(long, value_name = "n", default_value_t = 100)]
        window: usize,
    },
    /// Bandwidth and mean message size of each stream of one topic.
    Bw {
        /// Topic to measure.
        topic: String,
        #[command(flatten)]
        streams: StreamFlags,
    },
}

/// Lists the streams of the bus, one line each, from the transport's service listing: a
/// stream is listed from the moment a publisher or a declared subscriber creates it, with or
/// without a sample sent. Nothing waits; the type is read from the stream's schema record.
fn run_list(bus: &BusNode, verbose: bool) -> ExitCode {
    let mut listed = match bus.list_topics() {
        Ok(listed) => listed,
        Err(error) => {
            eprintln!("hypha topic list: {error}");
            return ExitCode::FAILURE;
        }
    };
    if listed.is_empty() {
        eprintln!("hypha topic list: no topics on the bus");
        return ExitCode::SUCCESS;
    }
    listed.sort_by(|a, b| (&a.name, a.schema_hash).cmp(&(&b.name, b.schema_hash)));
    for stream in &listed {
        let shared = listed.iter().filter(|other| other.name == stream.name).count() > 1;
        println!("{}", list_line(stream, &list_type(bus, stream, shared), verbose));
    }
    ExitCode::SUCCESS
}

/// The type a listed stream is shown with: its message type, and the start of its MD5 when
/// another stream has its name; its schema hash when no definition is known, and a lookup that
/// fails is said once on standard error.
fn list_type(bus: &BusNode, stream: &TopicInfo, shared: bool) -> String {
    let schema =
        bus.resolve_schema_hash_now(&stream.name, stream.schema_hash).unwrap_or_else(|cause| {
            eprintln!("hypha topic list: schema lookup failed for '{}': {cause}", stream.name);
            None
        });
    match schema {
        Some(schema) if shared => type_label(&schema.data_type, &schema.md5),
        Some(schema) => schema.data_type,
        None => stream_label(None, stream.schema_hash),
    }
}

/// One line of the listing: `name  [type]`, and with `verbose` the endpoints the stream has.
fn list_line(stream: &TopicInfo, data_type: &str, verbose: bool) -> String {
    let line = format!("{}  [{data_type}]", stream.name);
    if !verbose {
        return line;
    }
    format!(
        "{line}  {}, {}",
        count_of(stream.publishers, "publisher"),
        count_of(stream.subscribers, "subscriber")
    )
}

fn count_of(count: usize, noun: &str) -> String {
    let plural = if count == 1 { "" } else { "s" };
    format!("{count} {noun}{plural}")
}

fn run_echo(bus: &Arc<BusNode>, echo: &EchoArgs) -> ExitCode {
    let max_count = echo.max_count;
    let mut followed = match Followed::open(bus, &echo.topic, &echo.streams, "echo") {
        Ok(followed) => followed,
        Err(code) => return code,
    };

    let mut cache = SchemaCache::new("hypha_topic");
    let identities = SchemaIdentityCache::new();
    let mut count = 0usize;
    let mut header_printed = false;

    while bus.ok() && (max_count == 0 || count < max_count) {
        followed.refresh();
        if let Some(received) = followed.next() {
            let Some(sample) = received.to_raw_sample() else { continue };
            count = count.saturating_add(1);
            let stream = StreamKey { topic: sample.topic.clone(), schema_hash: sample.schema_hash };
            let resolved = resolve_identity(bus, &identities, &sample.topic, sample.schema_hash);
            let shared = followed.is_shared(&stream);
            // One table has one header: the rows of two streams cannot share it, and a header
            // before every row is no table. The streams are named, and the user picks one.
            if echo.plot && shared {
                eprintln!(
                    "hypha topic echo: -p prints one table, and '{}' has several streams: {}; \
                     pick one with --type or --md5",
                    echo.topic,
                    followed.streams().join(", ")
                );
                return ExitCode::FAILURE;
            }
            let label = shared.then(|| stream_label(resolved.as_deref(), sample.schema_hash));
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

fn run_hz(bus: &Arc<BusNode>, topic: &str, window: usize, streams: &StreamFlags) -> ExitCode {
    let mut followed = match Followed::open(bus, topic, streams, "hz") {
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

/// `delay`: each output frame against the newest input frame sent at or before it, by send
/// stamp, and the figures of the last `window` outputs once a second.
fn run_delay(bus: &Arc<BusNode>, input: &str, output: &str, window: usize) -> ExitCode {
    if input == output {
        eprintln!("hypha topic delay: '{input}' is both the input and the output");
        return ExitCode::FAILURE;
    }
    let open = |topic: &str| Followed::open(bus, topic, &StreamFlags::default(), "delay");
    let (mut inputs, mut outputs) = match (open(input), open(output)) {
        (Ok(inputs), Ok(outputs)) => (inputs, outputs),
        (Err(code), _) | (_, Err(code)) => return code,
    };
    let mut delays = Delays::new(window);
    let mut last_report = Instant::now();

    while bus.ok() {
        inputs.refresh();
        outputs.refresh();
        // The outputs are taken before the inputs, and measured after: every input sent
        // before an output that was taken is already on the bus when the inputs are taken.
        let mut sent_outputs = Vec::new();
        while let Some(received) = outputs.next() {
            sent_outputs.extend(received.identity().map(|id| id.transport_sent_nanos));
        }
        while let Some(received) = inputs.next() {
            if let Some(identity) = received.identity() {
                delays.on_input(identity.transport_sent_nanos);
            }
        }
        for sent in sent_outputs {
            delays.on_output(sent);
        }

        if last_report.elapsed() >= Duration::from_secs(1) {
            last_report = Instant::now();
            match delays.report() {
                Some(report) => println!("{report}"),
                None => println!("no output on '{output}' after an input on '{input}' yet"),
            }
        }
        thread::sleep(Duration::from_millis(1));
    }
    ExitCode::SUCCESS
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

fn run_bw(bus: &Arc<BusNode>, topic: &str, streams: &StreamFlags) -> ExitCode {
    let mut followed = match Followed::open(bus, topic, streams, "bw") {
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
        Ok(bus) => Arc::new(bus),
        Err(code) => return code,
    };

    match cli.verb {
        Verb::List { verbose } => run_list(&bus, verbose),
        Verb::Echo { echo } => run_echo(&bus, &echo),
        Verb::Hz { topic, window, streams } => run_hz(&bus, &topic, window, &streams),
        Verb::Delay { input, output, window } => run_delay(&bus, &input, &output, window),
        Verb::Bw { topic, streams } => run_bw(&bus, &topic, &streams),
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
