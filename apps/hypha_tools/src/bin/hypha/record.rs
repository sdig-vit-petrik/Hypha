//! `hypha record` -- records live bus traffic into an MCAP container file.

#![forbid(unsafe_code)]

use crate::cli::{connect, parse_cli};
use clap::Parser;
use hypha_node::sdk::{BusNode, FollowError, StreamFollower, StreamKey, StreamSelector};
use hypha_tools::following::Following;
use hypha_tools::record_session::RecorderSession;
use hypha_tools::record_sync::SyncParticipant;
use hypha_tools::recorder::{McapCompression, Recorder};
use hypha_tools::stream::{follow_error_text, follower_title};
use std::collections::BTreeMap;
use std::process::ExitCode;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Pending samples one followed topic may buffer: zero, which the transport reads as the
/// largest depth the topic's service policy admits. An observer may lose samples.
const SUBSCRIPTION_DEPTH: usize = 0;
/// How often a recorder looks for samples a stream sent before it was followed, so that it
/// tells them as they are found.
const BEFORE_REPORT_INTERVAL: Duration = Duration::from_millis(500);
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
        Ok(bus) => Arc::new(bus),
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
    // of a followed name included. An observer lists them on a thread of its own, so that a
    // listing never stops it reading; a `--sync` recorder lists inline, before it answers.
    let follower = StreamFollower::new(keys.clone(), StreamSelector::default(), SUBSCRIPTION_DEPTH);
    let opened = if sync_requested {
        Following::inline(&bus, follower)
    } else {
        Following::start(&bus, follower)
    };
    let mut follower = match opened {
        Ok(following) => following,
        Err(error) => {
            eprintln!("hypha record: {error}");
            return ExitCode::FAILURE;
        }
    };
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

/// What a recording is known to lack, per followed stream: samples the stream's queue lost
/// before the recorder read them, and samples a stream sent before the recorder followed it
/// (it came after the recording began). Each count is cumulative, so a read at the end covers
/// every loss.
struct LossWatch {
    last_check: Instant,
    last_before_check: Instant,
    /// The count last reported for each stream that lost samples, under its title.
    lost: BTreeMap<StreamKey, (String, u64)>,
    /// The count last reported for each stream that sent samples before it was followed.
    before: BTreeMap<StreamKey, (String, u64)>,
}

impl LossWatch {
    fn new() -> Self {
        Self {
            last_check: Instant::now(),
            last_before_check: Instant::now(),
            lost: BTreeMap::new(),
            before: BTreeMap::new(),
        }
    }

    /// Reads the counts that are due: what was sent before a stream was followed every
    /// [`BEFORE_REPORT_INTERVAL`], so it is told as it is found, and the losses every
    /// [`LOSS_REPORT_INTERVAL`].
    fn poll(&mut self, bus: &BusNode, follower: &StreamFollower) -> Result<(), String> {
        if self.last_before_check.elapsed() >= BEFORE_REPORT_INTERVAL {
            self.last_before_check = Instant::now();
            self.check_before(bus, follower);
        }
        if self.last_check.elapsed() >= LOSS_REPORT_INTERVAL {
            self.last_check = Instant::now();
            self.check(bus, follower)?;
        }
        Ok(())
    }

    /// The final read, at the end of the recording.
    fn finish(&mut self, bus: &BusNode, follower: &StreamFollower) -> Result<(), String> {
        self.check_before(bus, follower);
        self.check(bus, follower)
    }

    /// Prints one warning per stream whose lost count grew since it was last reported.
    fn check(&mut self, bus: &BusNode, follower: &StreamFollower) -> Result<(), String> {
        for (stream, count) in follower.lost()? {
            let previous = self.lost.get(&stream).map_or(0, |(_, reported)| *reported);
            let grew: bool = count > previous;
            if grew {
                let title = follower_title(follower, bus, &stream);
                eprintln!("hypha record: {title}: {count} samples lost");
                self.lost.insert(stream, (title, count));
            }
        }
        Ok(())
    }

    /// Prints one line per stream whose sent-before-following count grew.
    fn check_before(&mut self, bus: &BusNode, follower: &StreamFollower) {
        for (stream, count) in follower.sent_before_following() {
            let previous = self.before.get(&stream).map_or(0, |(_, reported)| *reported);
            if count > previous {
                let title = follower_title(follower, bus, &stream);
                eprintln!(
                    "hypha record: {title}: {count} samples were sent before it was followed"
                );
                self.before.insert(stream, (title, count));
            }
        }
    }

    /// Whether a stream lost samples: the recording is then incomplete.
    fn any_lost(&self) -> bool {
        !self.lost.is_empty()
    }

    /// The streams that lost samples, and those that sent samples before they were followed,
    /// with their final counts, under the recorded messages.
    fn print_summary(&self) {
        for (title, count) in self.lost.values() {
            println!("  {title} lost {count} samples");
        }
        for (title, count) in self.before.values() {
            println!("  {title} sent {count} samples before it was followed");
        }
    }
}

/// Records until the bus stops or a `--sync` recording fails. Returns whether it failed.
/// An observer reads the lost-sample counts as it goes; a `--sync` recorder stops at a loss
/// in [`answer_chunk`] instead.
fn record(
    bus: &BusNode,
    session: &mut RecorderSession<'_>,
    follower: &mut Following,
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
    let mut told = None;
    while bus.ok() {
        // A stream that joined, or one that refuses its subscription, is known before any
        // sample is read. A `--sync` recording that cannot follow a stream that exists is not
        // faithful: it ends there.
        if let Err(cause) = follow_new_streams(bus, follower, sync.is_some(), &mut told) {
            eprintln!("hypha record: --sync: {cause}");
            return true;
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

/// Follows what the listings found. A stream that joined is told on standard error (a reader
/// of standard output may have gone, as a pipe closes); a failure is told once while it lasts.
///
/// # Errors
/// A `--sync` recording that a stream refuses to be followed for: the text of the refusal.
fn follow_new_streams(
    bus: &BusNode,
    follower: &mut Following,
    strict: bool,
    told: &mut Option<String>,
) -> Result<(), String> {
    let before = follower.followed();
    let outcome = follower.pass(bus);
    let added = follower.followed().saturating_sub(before);
    if added > 0 {
        eprintln!("hypha record: following {added} more stream(s), {} in all", follower.followed());
    }
    let Err(error) = outcome else { return Ok(()) };
    let text = follow_error_text(bus, &error);
    if strict && matches!(error, FollowError::Refused(_)) {
        return Err(format!("{text}; the recording would lack it"));
    }
    if told.as_ref() != Some(&text) {
        eprintln!("hypha record: {text}");
        *told = Some(text);
    }
    Ok(())
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
/// lost samples, or sent some before it was followed: the chunk is then not what the player
/// sent.
fn answer_chunk(
    bus: &BusNode,
    participant: &mut SyncParticipant,
    follower: &mut Following,
    session: &mut RecorderSession<'_>,
) -> Result<Flow, String> {
    let Some(report) = participant.poll()? else {
        return Ok(Flow::Continue);
    };
    // A stream that came before the chunk's data is followed before the chunk is answered.
    follow_new_streams(bus, follower, true, &mut None)?;
    while let Some(sample) = follower.next_sample()? {
        record_one(session, &sample);
    }
    if let Some(cause) = session.failure() {
        return Err(cause.to_owned());
    }
    let lost = follower.lost()?;
    let before = follower.sent_before_following();
    if lost.is_empty() && before.is_empty() {
        participant.send(&report)?;
        return Ok(Flow::Continue);
    }
    for (stream, count) in &lost {
        let title = follower_title(follower, bus, stream);
        eprintln!(
            "hypha record: --sync: {title} lost {count} sample(s) so far: its queue was full; lower hypha play --chunk so that a chunk holds fewer samples of that stream"
        );
    }
    for (stream, count) in &before {
        let title = follower_title(follower, bus, stream);
        eprintln!(
            "hypha record: --sync: {title} sent {count} sample(s) before it was followed: it appeared during a chunk, so the recording lacks them"
        );
    }
    Ok(Flow::Stop)
}
