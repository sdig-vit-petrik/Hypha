//! Playback session: publishes a recording onto the bus.

mod barrier;
mod chunks;
mod keys;
mod pace;
mod reports;
mod restart;
mod start;
#[cfg(test)]
#[allow(clippy::arithmetic_side_effects, clippy::expect_used, clippy::unwrap_used)]
mod tests;
mod waiting;

use self::pace::Schedule;
use crate::CLOCK_TOPIC;
use crate::player::{ChannelResolution, PlaybackItem, Player};
use crate::replay_bus;
use crate::replay_report::RateReport;
use crate::replay_sync::{
    ChunkPlan, DEFAULT_CHUNK_NANOS, DEFAULT_SETTLE, DEFAULT_TIMEOUT, PROGRESS_TOPIC, Pace,
};
use crate::stream::{StreamKey, stream_title, type_label};
use crate::terminal_ui::TerminalController;
use crate::utc_time::UtcRange;
use hypha_node::sdk::clock_barrier::{CLOCK_RESEND_INTERVAL, Participants};
use hypha_node::sdk::{BusNode, PublisherOptions, RawPublisher, RawSubscriber, WaitSet};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// ROS's own hardcoded convention (tf2's `StaticTransformBroadcaster`). Published latched so a
/// subscriber that attaches after playback moves on still receives it.
const LATCHED_TOPIC_BY_CONVENTION: &str = "/tf_static";
/// How long a clockless pass keeps its publishers after the last message.
const PUBLISHER_LINGER: Duration = Duration::from_millis(200);

/// Keys are read and the progress line drawn this often while the replay waits.
const CONTROL_POLL_INTERVAL: Duration = Duration::from_millis(20);
/// The progress line is drawn at most this often.
const PROGRESS_RENDER_INTERVAL: Duration = Duration::from_millis(50);

/// Configuration for an MCAP playback session.
#[derive(Debug, Clone)]
pub struct PlaySessionConfig {
    /// Pace of the replay on the wall clock.
    pub pace: Pace,
    /// Loop playback continuously until interrupted.
    pub loop_playback: bool,
    /// Publish `/clock` and pace the replay in chunks that every participant has finished.
    pub publish_clock: bool,
    /// Whitelist of topic names to replay.
    pub topics: Vec<String>,
    /// UTC recording window, with inclusive start and exclusive end.
    pub utc_range: UtcRange,
    /// The chunking and participation rules. Used only with `publish_clock`.
    pub sync: SyncConfig,
}

/// How the replay chunks its data and which nodes it waits for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncConfig {
    /// Data time of one chunk, in nanoseconds.
    pub chunk_nanos: u64,
    /// Nodes that must report before the start. Empty: every node that reports.
    pub nodes: Vec<String>,
    /// How long the start waits after the last new node, when `nodes` is empty.
    pub settle: Duration,
    /// How long a node may report nothing while the replay waits for it.
    pub timeout: Duration,
    /// Wait for the participants at each chunk. `false` (`HYPHA_REPLAY_LOCKSTEP=0`) is an
    /// open-loop playback: `/clock` and the data on the `--rate` schedule, with no barrier,
    /// no acknowledgement and no restart of the participants.
    pub lockstep: bool,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            chunk_nanos: DEFAULT_CHUNK_NANOS,
            nodes: Vec::new(),
            settle: DEFAULT_SETTLE,
            timeout: DEFAULT_TIMEOUT,
            lockstep: true,
        }
    }
}

impl Default for PlaySessionConfig {
    fn default() -> Self {
        Self {
            pace: Pace::Factor(1.0),
            loop_playback: false,
            publish_clock: false,
            topics: Vec::new(),
            utc_range: UtcRange::default(),
            sync: SyncConfig::default(),
        }
    }
}

/// Outcome of a playback run.
#[derive(Debug, Clone, Default)]
pub struct PlaySessionResult {
    /// Total messages published, across every pass.
    pub published: u64,
    /// Per-stream publish counts, in declaration order, each under the stream's topic in
    /// quotes, and its type and MD5 start when the topic has more than one stream.
    pub counters: Vec<(String, u64)>,
    /// A replay timing, clock or participant failure, when the run was refused or stopped.
    pub error: Option<String>,
    /// The effective-rate line of a `--clock` replay that started, `None` otherwise.
    pub rate: Option<String>,
}

/// The publisher options of one replayed topic: bounded to the largest message the
/// recording holds of it, or growable when the recording has no message index to
/// tell. A fixed-size type is bounded to its exact size either way.
#[must_use]
pub fn channel_publisher_options(latched: bool, max_payload: Option<usize>) -> PublisherOptions {
    let options = PublisherOptions::default().with_latch(latched);
    match max_payload {
        Some(bytes) => options.bounded(bytes.max(1)),
        None => options.growable(),
    }
}

/// Publish one replayed topic.
///
/// # Errors
/// Returns the publisher's refusal when the publication fails.
pub fn publish_channel(
    bus: &BusNode,
    channel: &ChannelResolution,
    latched: bool,
) -> Result<RawPublisher, String> {
    bus.publish_raw_configured(
        &channel.topic,
        &channel.data_type,
        &channel.md5,
        &channel.definition,
        channel_publisher_options(latched, channel.max_payload),
    )
}

/// Whether the replay publishes `topic`: the topics of the filter, and never the
/// recording's `/clock` under `--clock`, which the replay's own clock takes the place of.
fn replays(config: &PlaySessionConfig, topic: &str) -> bool {
    if config.publish_clock && topic == CLOCK_TOPIC {
        return false;
    }
    config.topics.is_empty() || config.topics.iter().any(|allowed| allowed == topic)
}

fn replayed_topics<'a>(player: &'a Player, config: &PlaySessionConfig) -> Vec<&'a str> {
    let mut topics: Vec<&str> = Vec::new();
    for ch in player.channels() {
        if !replays(config, &ch.topic) {
            continue;
        }
        if !topics.contains(&ch.topic.as_str()) {
            topics.push(ch.topic.as_str());
        }
    }
    topics
}

/// Whether the replay publishes more than one stream on `topic`.
fn has_several_streams(channels: &[&ChannelResolution], topic: &str) -> bool {
    let mut streams = channels
        .iter()
        .filter(|channel| channel.topic == topic && !channel.md5.is_empty())
        .map(|channel| &channel.stream);
    streams.next().is_some_and(|first| streams.any(|other| other != first))
}

/// The publishers of the replayed streams and the order the recording declares them in.
struct ChannelPublishers {
    publishers: HashMap<StreamKey, RawPublisher>,
    order: Vec<StreamKey>,
    /// Each stream under its topic, and its type when the topic has several: how a message
    /// of the session names it.
    titles: HashMap<StreamKey, String>,
}

/// Publishes each stream the recording holds: one publisher for each topic and MD5, so a
/// topic recorded with two versions is replayed as two streams.
fn create_channel_publishers(
    bus: &BusNode,
    player: &Player,
    config: &PlaySessionConfig,
) -> Result<ChannelPublishers, String> {
    let mut created =
        ChannelPublishers { publishers: HashMap::new(), order: Vec::new(), titles: HashMap::new() };
    let mut refusals: Vec<String> = Vec::new();
    let channels: Vec<&ChannelResolution> =
        player.channels().iter().filter(|ch| replays(config, &ch.topic)).collect();

    for ch in &channels {
        if ch.md5.is_empty() {
            let data_type = if ch.data_type.is_empty() { "unknown type" } else { &ch.data_type };
            refusals.push(format!(
                "'{}' cannot be published: no resolvable md5 for {data_type}",
                ch.topic
            ));
            continue;
        }
        // Two channels of one stream (one topic, one MD5) are published once.
        if created.publishers.contains_key(&ch.stream) {
            continue;
        }

        let is_latched = ch.topic == LATCHED_TOPIC_BY_CONVENTION;

        match publish_channel(bus, ch, is_latched) {
            Ok(pub_handle) => {
                created.publishers.insert(ch.stream.clone(), pub_handle);
                created.order.push(ch.stream.clone());
                let shared = has_several_streams(&channels, &ch.topic);
                let title = stream_title(
                    &format!("'{}'", ch.topic),
                    &type_label(&ch.data_type, &ch.md5),
                    shared,
                );
                created.titles.insert(ch.stream.clone(), title);
            }
            Err(e) => refusals.push(format!("'{}' cannot be published: {e}", ch.topic)),
        }
    }

    if refusals.is_empty() {
        return Ok(created);
    }
    for line in &refusals {
        println!("hypha play: {line}");
    }
    Err(format!(
        "refusing to start: {} topic(s) cannot be published; nothing was replayed",
        refusals.len()
    ))
}

fn create_clock_publisher(bus: &BusNode) -> Option<RawPublisher> {
    use hypha_node::sdk::msgs::rosgraph_msgs::Clock;
    // The definition bounds the fixed-size clock to its exact size.
    let options = hypha_node::sdk::PublisherOptions::default();
    match bus.publish_raw_configured(
        CLOCK_TOPIC,
        Clock::TYPE_NAME,
        Clock::SCHEMA_HASH,
        crate::replay_sync::CLOCK_DEFINITION,
        options,
    ) {
        Ok(pub_handle) => Some(pub_handle),
        Err(e) => {
            println!("hypha play: failed to publish {CLOCK_TOPIC}: {e}");
            None
        }
    }
}

/// Whether the replay goes on after a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    Go,
    /// The pass stops here: a restart was asked for, or the replay is quitting.
    Halt,
}

/// Whether the replay runs, waits, or lets one chunk through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Playback {
    Running,
    /// Nothing goes out and the schedule is held.
    Paused,
    /// A step was asked for while paused: the next chunk goes out.
    StepRequested,
    /// The chunk of a step is going out.
    Stepping,
}

/// The state of one replay, from the first pass to the last.
struct Session<'bus> {
    bus: &'bus BusNode,
    /// The recording, to publish its topics again after a restart.
    player: &'bus Player,
    config: PlaySessionConfig,
    stop_signal: Arc<AtomicBool>,
    term: TerminalController,
    publishers: HashMap<StreamKey, RawPublisher>,
    stream_order: Vec<StreamKey>,
    /// How a message of the session names each stream.
    titles: HashMap<StreamKey, String>,
    counters: HashMap<StreamKey, u64>,
    /// The `/clock` publisher, when the replay publishes one.
    clock: Option<RawPublisher>,
    /// The reports of the participants, queued so that a report wakes a wait.
    progress: Option<Arc<std::sync::Mutex<reports::ReportQueue>>>,
    /// A second reader of the reports topic, read only for its connected publishers.
    progress_publishers: Option<RawSubscriber>,
    /// Serves the schema of every replayed topic.
    waitset: WaitSet<'bus>,
    participants: Participants,
    /// Whether the start barrier has run. Later passes start on the known participants.
    discovered: bool,
    /// The participants restarted for this pass and have not rejoined yet.
    rejoining: bool,
    /// The effective-rate figures, read at the end of the replay.
    report: RateReport,
    /// When the first chunk was released, after the start barrier.
    replay_started: Option<Instant>,
    /// The chunk boundary up to which the data has been counted in `report`.
    last_boundary: u64,
    schedule: Schedule,
    /// The chunk boundaries of the current pass, or `None` before its first message.
    plan: Option<ChunkPlan>,
    /// The log time at which the current pass started.
    pass_start_log: u64,
    /// The log time of the message that a restart stopped before it went out.
    halted_at: Option<u64>,
    /// Messages published per stream since the current `/clock` chunk opened.
    in_chunk: HashMap<StreamKey, usize>,
    /// The smallest subscriber queue per stream, looked up on first use in each chunk.
    limits: HashMap<StreamKey, Option<usize>>,
    /// Streams whose chunks have been shortened, reported once each.
    shortened: HashSet<StreamKey>,
    restart_requested: bool,
    playback: Playback,
    current_msg: u64,
    /// The log time of the last message published; the tail of a pass closes after it.
    last_log_time: u64,
    last_topic: String,
    last_render: Instant,
    total_duration: f64,
    total_messages: u64,
    failure: Option<String>,
}

impl<'bus> Session<'bus> {
    fn open(
        bus: &'bus BusNode,
        player: &'bus Player,
        created: ChannelPublishers,
        config: PlaySessionConfig,
        stop_signal: Arc<AtomicBool>,
    ) -> Result<Self, String> {
        let (clock, progress) = if config.publish_clock {
            replay_bus::ensure_sole_clock_publisher(bus)?;
            let clock = create_clock_publisher(bus)
                .ok_or_else(|| "cannot create UTC replay clock".to_string())?;
            // An open-loop playback reads no report: nothing it does waits for a node.
            let progress = if config.sync.lockstep {
                Some(
                    replay_bus::progress_subscriber(bus)
                        .map_err(|error| format!("cannot read {PROGRESS_TOPIC}: {error}"))?,
                )
            } else {
                None
            };
            (Some(clock), progress)
        } else {
            (None, None)
        };
        let mut waitset =
            WaitSet::new(bus).map_err(|error| format!("cannot create waitset: {error}"))?;
        let progress_publishers = match progress.as_ref() {
            Some(_) => Some(
                replay_bus::progress_subscriber(bus)
                    .map_err(|error| format!("cannot read {PROGRESS_TOPIC}: {error}"))?,
            ),
            None => None,
        };
        let progress = match progress {
            Some(subscriber) => Some(
                reports::attach(&mut waitset, subscriber)
                    .map_err(|error| format!("cannot attach {PROGRESS_TOPIC}: {error}"))?,
            ),
            None => None,
        };
        let now = Instant::now();
        let ChannelPublishers { publishers, order: stream_order, titles } = created;
        let counters = stream_order.iter().map(|stream| (stream.clone(), 0)).collect();
        Ok(Self {
            bus,
            player,
            total_duration: player.duration_seconds(),
            total_messages: player.total_messages(),
            term: TerminalController::new(),
            config,
            stop_signal,
            publishers,
            stream_order,
            titles,
            counters,
            clock,
            progress,
            progress_publishers,
            waitset,
            participants: Participants::new(now),
            discovered: false,
            rejoining: false,
            report: RateReport::default(),
            replay_started: None,
            last_boundary: 0,
            schedule: Schedule { anchor_log: 0, anchor_wall: now },
            plan: None,
            pass_start_log: 0,
            halted_at: None,
            in_chunk: HashMap::new(),
            limits: HashMap::new(),
            shortened: HashSet::new(),
            restart_requested: false,
            playback: Playback::Running,
            current_msg: 0,
            last_log_time: 0,
            last_topic: String::new(),
            last_render: now,
            failure: None,
        })
    }

    fn stopping(&self) -> bool {
        self.stop_signal.load(Ordering::Relaxed)
    }

    /// The publishers connected to the progress topic; none while there is no such topic.
    fn progress_publisher_count(&self) -> Result<usize, String> {
        self.progress_publishers.as_ref().map_or(Ok(0), RawSubscriber::publisher_count)
    }

    /// True when the replay publishes `/clock` and waits for its participants at each
    /// chunk; false without `/clock` and in an open-loop playback.
    fn in_lockstep(&self) -> bool {
        self.clock.is_some() && self.config.sync.lockstep
    }

    /// Resets the state that belongs to one pass.
    fn begin_pass(&mut self) {
        self.plan = None;
        self.halted_at = None;
        self.restart_requested = false;
        self.current_msg = 0;
    }
}

/// Runs a playback session over `Player` with interactive controls.
#[must_use]
pub fn run_play_session(
    bus: &BusNode,
    player: &Player,
    config: PlaySessionConfig,
    stop_signal: &Arc<AtomicBool>,
) -> PlaySessionResult {
    let replayed = replayed_topics(player, &config);
    if let Err(error) = replay_bus::warn_live_publishers(bus, &replayed) {
        return PlaySessionResult { error: Some(error), ..PlaySessionResult::default() };
    }
    let created = match create_channel_publishers(bus, player, &config) {
        Ok(created) => created,
        Err(error) => {
            return PlaySessionResult { error: Some(error), ..PlaySessionResult::default() };
        }
    };
    let topics = config.topics.clone();
    let range = config.utc_range;
    let loop_playback = config.loop_playback;
    let mut session = match Session::open(bus, player, created, config, Arc::clone(stop_signal)) {
        Ok(session) => session,
        Err(error) => {
            return PlaySessionResult { error: Some(error), ..PlaySessionResult::default() };
        }
    };
    session.term.print_controls_help();

    let mut error = None;
    loop {
        if session.stopping() {
            break;
        }
        session.begin_pass();
        let read =
            player.for_each_item(&topics, range, |item: &PlaybackItem<'_>| session.on_item(item));
        let started = session.plan.is_some();
        session.finish_pass();
        session.term.clear_progress_line();

        if let Some(failure) = session.failure.take() {
            error = Some(failure);
            break;
        }
        if let Err(read_error) = read {
            error = Some(read_error);
            break;
        }
        if !started {
            break;
        }
        let repeat = (loop_playback || session.restart_requested) && !session.stopping();
        if !repeat {
            break;
        }
        if loop_playback && !session.restart_requested {
            println!("\n[hypha play] looping playback...");
        }
    }

    session.term.clear_progress_line();

    let counters = session.counters_in_order();
    let published = counters.iter().fold(0_u64, |sum, (_, count)| sum.saturating_add(*count));
    let rate = match (session.config.publish_clock, session.replay_started) {
        (true, Some(started)) => {
            Some(session.report.summary(started.elapsed(), session.config.pace))
        }
        _ => None,
    };
    if published > 0 && !session.config.publish_clock {
        // Fixed pause: iceoryx2 loses a sample whose publisher left before a subscriber connects.
        std::thread::sleep(PUBLISHER_LINGER);
    }
    PlaySessionResult { published, counters, error, rate }
}
