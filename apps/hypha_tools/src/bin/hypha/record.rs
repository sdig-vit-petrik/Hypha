//! `hypha record` -- records live bus traffic into an MCAP container file.

#![forbid(unsafe_code)]

use crate::cli::{connect, parse_cli};
use clap::Parser;
use hypha_node::sdk::{BusNode, StreamFollower, StreamKey, StreamSelector};
use hypha_tools::record_session::RecorderSession;
use hypha_tools::record_sync::SyncParticipant;
use hypha_tools::recorder::{McapCompression, Recorder};
use hypha_tools::stream::follower_title;
use std::collections::BTreeMap;
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Pending samples one followed topic may buffer: zero, which the transport reads as the
/// largest depth the topic's service policy admits. An observer may lose samples.
const SUBSCRIPTION_DEPTH: usize = 0;
/// How often an observing recorder reads the lost-sample counts of the followed topics.
const LOSS_REPORT_INTERVAL: Duration = Duration::from_secs(10);
/// Samples written per pass, so one busy topic cannot hold the loop open and
/// leave a stop request unnoticed.
const DRAIN_BUDGET_PER_PASS: usize = 512;
/// The idle wait of a `--sync` recorder, which must answer each chunk promptly.
const SYNC_IDLE_WAIT: Duration = Duration::from_micros(200);
/// The idle wait of a recorder that only observes the bus.
const IDLE_WAIT: Duration = Duration::from_millis(2);

/// Command line of `hypha record`.
#[derive(Debug, Parser)]
#[command(
    name = "hypha record",
    about = "Records live bus traffic into an MCAP container file.",
    after_help = "Finalizes the MCAP index cleanly on SIGINT and SIGTERM."
)]
struct Cli {
    /// Topic names or patterns to record (default: '**').
    #[arg(long = "topics", value_name = "pattern", num_args = 1..)]
    topics: Vec<String>,
    /// Output destination (default: hypha_<timestamp>.mcap).
    #[arg(short = 'o', long = "output", value_name = "file", allow_hyphen_values = true)]
    output: Option<String>,
    /// Chunk compression.
    #[arg(short = 'c', long = "compression", value_name = "format", default_value = "lz4")]
    compression: McapCompression,
    /// Take part in a /clock replay: hold each chunk until it is recorded.
    /// Without --nodes, the first chunk may be acknowledged before a participant that has
    /// not reported yet is heard.
    #[arg(long = "sync")]
    sync: bool,
    /// With --sync: nodes the first chunk waits for, so that none is missed before the
    /// first acknowledgement.
    #[arg(long = "nodes", value_name = "n", num_args = 1..)]
    nodes: Vec<String>,
}

fn default_filename() -> String {
    let sec = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    format!("hypha_{sec}.mcap")
}

/// Runs the tool; `args` is the whole command line, `args[0]` the name it shows.
pub(crate) fn main(args: Vec<String>) -> ExitCode {
    let cli: Cli = match parse_cli(&args) {
        Ok(cli) => cli,
        Err(code) => return code,
    };
    let sync_requested = cli.sync;
    let nodes = cli.nodes;
    let compression = cli.compression;
    if !nodes.is_empty() && !sync_requested {
        eprintln!("hypha record: --nodes needs --sync");
        return ExitCode::FAILURE;
    }
    let output = cli.output.unwrap_or_else(default_filename);
    let keys = if cli.topics.is_empty() { vec!["**".to_string()] } else { cli.topics };
    // A `--sync` recorder answers no chunk before it follows every named topic: a topic
    // followed late would miss the outputs of the chunks acknowledged before. A pattern
    // names no topic in advance.
    let sync_topics: Vec<String> = keys.iter().filter(|key| !key.contains('*')).cloned().collect();

    // A `--sync` recorder answers the replay under its node name, so it holds a name of
    // its own. Without `--sync` it is a CLI tool, and any number of recorders can run.
    let node_name = if sync_requested { "hypha_record" } else { "_hyphacli_record" };
    let bus = match connect("hypha record: ", node_name) {
        Ok(bus) => bus,
        Err(code) => return code,
    };

    let recorder = Recorder::with_compression(&output, compression);
    if !recorder.is_open() {
        eprintln!("hypha record: cannot open '{output}': {}", recorder.open_error());
        return ExitCode::FAILURE;
    }

    println!("Recording to '{output}' (compression: {compression:?})...");

    let mut session = RecorderSession::new(&bus, recorder, sync_requested);

    // A replay's participants are opened before the follower lists topics, so the
    // recorder is ready for the first chunk once it prints the follow line below.
    let mut sync = if sync_requested {
        match SyncParticipant::open(&bus, nodes) {
            Ok(participant) => Some(participant),
            Err(error) => {
                eprintln!("hypha record: --sync: {error}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        None
    };

    // A subscription that names no schema reads the streams that exist when it is made, so
    // the follower subscribes to each stream as the graph event shows it, a second version
    // of a followed name included.
    let mut follower = match StreamFollower::new(
        &bus,
        keys.clone(),
        StreamSelector::default(),
        SUBSCRIPTION_DEPTH,
    ) {
        Ok(follower) => follower,
        Err(error) => {
            eprintln!("hypha record: cannot follow the bus: {error}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(error) = follower.refresh(&bus) {
        eprintln!("hypha record: cannot list topics: {error}");
        return ExitCode::FAILURE;
    }
    println!("Following {} stream(s) matching {keys:?}", follower.followed());

    let mut losses = LossWatch::new();
    let mut failed =
        record(&bus, &mut session, &mut follower, sync.as_mut(), &sync_topics, &mut losses);
    // The last read covers a burst that started and ended between two periodic reads.
    if let Err(error) = losses.finish(&bus, &follower) {
        eprintln!("hypha record: cannot read lost samples: {error}");
        failed = true;
    }

    drop(sync);
    drop(follower);

    println!("\nFinalizing '{output}'...");
    let recorder = session.finish();
    println!("Recorded messages:");
    let channels = recorder.counters();
    for channel in &channels {
        println!("  {:<32} {} msgs", channel.title(&channels), channel.messages);
    }
    losses.print_summary();

    // A recording that lost samples is incomplete, so it does not exit 0.
    let lost_any = losses.any_lost();
    let recording_failed = failed || lost_any;
    if recording_failed { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}

/// The lost-sample counts of the followed streams. A subscription's count is cumulative
/// since it was made, so a read at the end covers every loss.
struct LossWatch {
    last_check: Instant,
    /// The cumulative count last reported for each stream that lost samples, under the
    /// stream's title.
    reported: BTreeMap<StreamKey, (String, u64)>,
}

impl LossWatch {
    fn new() -> Self {
        Self { last_check: Instant::now(), reported: BTreeMap::new() }
    }

    /// Reads the counts when the interval has passed since the last read.
    fn poll(&mut self, bus: &BusNode, follower: &StreamFollower) -> Result<(), String> {
        let interval_passed = self.last_check.elapsed() >= LOSS_REPORT_INTERVAL;
        if !interval_passed {
            return Ok(());
        }
        self.last_check = Instant::now();
        self.check(bus, follower)
    }

    /// The final read, at the end of the recording.
    fn finish(&mut self, bus: &BusNode, follower: &StreamFollower) -> Result<(), String> {
        self.check(bus, follower)
    }

    /// Prints one warning per stream whose count grew since it was last reported.
    fn check(&mut self, bus: &BusNode, follower: &StreamFollower) -> Result<(), String> {
        for (stream, count) in follower.lost()? {
            let previous = self.reported.get(&stream).map_or(0, |(_, reported)| *reported);
            let grew: bool = count > previous;
            if grew {
                let title = follower_title(follower, bus, &stream);
                eprintln!("hypha record: {title}: {count} samples lost");
                self.reported.insert(stream, (title, count));
            }
        }
        Ok(())
    }

    fn any_lost(&self) -> bool {
        !self.reported.is_empty()
    }

    /// The streams that lost samples, with their final counts, under the recorded messages.
    fn print_summary(&self) {
        for (title, count) in self.reported.values() {
            println!("  {title} lost {count} samples");
        }
    }
}

/// Records until the bus stops or a `--sync` recording fails. Returns whether it failed.
/// An observer reads the lost-sample counts as it goes; a `--sync` recorder stops at a loss
/// in [`answer_chunk`] instead.
fn record(
    bus: &BusNode,
    session: &mut RecorderSession<'_>,
    follower: &mut StreamFollower,
    mut sync: Option<&mut SyncParticipant>,
    sync_topics: &[String],
    losses: &mut LossWatch,
) -> bool {
    let is_observer = sync.is_none();
    let idle_wait = if sync.is_some() { SYNC_IDLE_WAIT } else { IDLE_WAIT };
    let mut awaiting_topics = sync.is_some() && !follower.missing(sync_topics).is_empty();
    if awaiting_topics {
        println!("--sync: no chunk is answered before every --topics entry has a publisher");
    }
    while bus.ok() {
        // The graph event brings a stream that joins later, before any sample is read.
        match follower.refresh_if_changed(bus) {
            Ok(0) => {}
            Ok(added) => {
                println!("Following {added} more stream(s), {} in all", follower.followed());
            }
            Err(error) => eprintln!("hypha record: cannot list topics: {error}"),
        }
        if awaiting_topics {
            awaiting_topics = !follower.missing(sync_topics).is_empty();
        }
        let mut drained = 0_usize;
        while drained < DRAIN_BUDGET_PER_PASS {
            let next = follower.next_sample().unwrap_or_else(|error| {
                eprintln!("hypha record: {error}");
                None
            });
            let Some(sample) = next else { break };
            drained = drained.saturating_add(1);
            record_one(session, &sample);
            if sync.is_some() && session.failure().is_some() {
                break;
            }
        }
        if sync.is_some()
            && let Some(cause) = session.failure()
        {
            eprintln!("hypha record: --sync: {cause}");
            return true;
        }
        if let Some(participant) = sync.as_deref_mut()
            && let Err(error) = participant.keep_known()
        {
            eprintln!("hypha record: --sync: {error}");
            return true;
        }
        if let Some(participant) = sync.as_deref_mut().filter(|_| !awaiting_topics) {
            match answer_chunk(bus, participant, follower, session) {
                Ok(Flow::Continue) => {}
                Ok(Flow::Stop) => {
                    return true;
                }
                Err(error) => {
                    eprintln!("hypha record: --sync: {error}");
                    return true;
                }
            }
        }
        if is_observer && let Err(error) = losses.poll(bus, follower) {
            eprintln!("hypha record: cannot read lost samples: {error}");
            return true;
        }
        if drained == 0 {
            thread::sleep(idle_wait);
        }
    }

    false
}

/// Records one received sample. A frame the library cannot present is skipped.
fn record_one(session: &mut RecorderSession<'_>, sample: &hypha_node::sdk::ReceivedSample) {
    if let (Some(identity), Some(bytes)) = (sample.identity(), sample.bytes()) {
        session.on_sample(&identity, bytes);
    }
}

/// Whether a `--sync` recorder goes on after answering a chunk.
enum Flow {
    Continue,
    Stop,
}

/// Answers the newest `/clock` sample once every followed topic is drained and every
/// other participant has acknowledged it. Stops the recording when a followed topic
/// lost samples: the chunk is then not what the player sent.
fn answer_chunk(
    bus: &BusNode,
    participant: &mut SyncParticipant,
    follower: &mut StreamFollower,
    session: &mut RecorderSession<'_>,
) -> Result<Flow, String> {
    let Some(report) = participant.poll()? else {
        return Ok(Flow::Continue);
    };
    // A stream that came before the chunk's data is followed before the chunk is answered.
    follower.refresh_if_changed(bus)?;
    while let Some(sample) = follower.next_sample()? {
        record_one(session, &sample);
    }
    if let Some(cause) = session.failure() {
        return Err(cause.to_owned());
    }
    let lost = follower.lost()?;
    if lost.is_empty() {
        participant.send(&report)?;
        return Ok(Flow::Continue);
    }
    for (stream, count) in &lost {
        let title = follower_title(follower, bus, stream);
        eprintln!(
            "hypha record: --sync: {title} lost {count} sample(s) so far: its queue was full; lower hypha play --chunk so that a chunk holds fewer samples of that stream"
        );
    }
    Ok(Flow::Stop)
}
