use super::{Instant, Schedule};
use std::time::Duration;

fn schedule_at(anchor_log: u64, anchor_wall: Instant) -> Schedule {
    Schedule { anchor_log, anchor_wall }
}

#[test]
fn a_deadline_is_anchored_to_the_pass_start() {
    let start = Instant::now();
    let schedule = schedule_at(1_000_000_000, start);

    let first = schedule.deadline(1_500_000_000, 1.0);
    let later = schedule.deadline(3_000_000_000, 1.0);

    assert_eq!(first.map(|d| d.duration_since(start)), Some(Duration::from_millis(500)));
    assert_eq!(later.map(|d| d.duration_since(start)), Some(Duration::from_secs(2)));
}

#[test]
fn a_rate_too_slow_for_the_schedule_is_refused_not_a_panic() {
    let schedule = schedule_at(0, Instant::now());
    assert_eq!(schedule.deadline(u64::MAX, 1e-15), None);
}

#[test]
fn a_deadline_refuses_a_non_finite_rate() {
    let schedule = schedule_at(0, Instant::now());
    assert_eq!(schedule.deadline(1, f64::NAN), None);
}

#[test]
fn a_rate_change_keeps_the_schedule_where_it_was() {
    let start = Instant::now();
    let mut schedule = schedule_at(0, start);

    // 100 s into the pass at rate 1, the rate falls to 0.8.
    let now = start + Duration::from_secs(100);
    schedule.freeze(now, 1.0);
    assert_eq!(schedule.virtual_log_time(now, 0.8), Some(100_000_000_000));

    // The next message, 10 s of log time on, is due 12.5 s later.
    let deadline = schedule.deadline(110_000_000_000, 0.8);
    assert_eq!(deadline.map(|d| d.duration_since(now)), Some(Duration::from_millis(12_500)));
}

#[test]
fn a_pause_holds_the_schedule_at_its_position() {
    let start = Instant::now();
    let mut schedule = schedule_at(0, start);
    schedule.freeze(start + Duration::from_secs(10), 1.0);
    // Resuming ten seconds later continues from ten seconds of log time.
    let resumed = start + Duration::from_secs(20);
    let anchor = schedule.anchor_log;
    schedule.reanchor(anchor, resumed);
    assert_eq!(schedule.virtual_log_time(resumed, 1.0), Some(10_000_000_000));
}

/// A session that replays only the recorded `/clock`, which it never publishes, with no
/// participant: each chunk it closes is a `/clock` that `clocks` reads back.
struct ClockedSession<'bus> {
    session: super::Session<'bus>,
    clocks: hypha_node::sdk::RawSubscriber,
}

const START: u64 = 1_000_000_000;
const MS: u64 = 1_000_000;

fn bus() -> hypha_node::sdk::BusNode {
    hypha_node::sdk::BusNode::new("play_session_tests").expect("a bus node")
}

fn player() -> crate::player::Player {
    // The session reads the recording only for its progress line and a restart's publishers.
    crate::player::Player::new("/nonexistent/play_session_tests.mcap")
}

impl<'bus> ClockedSession<'bus> {
    fn open(bus: &'bus hypha_node::sdk::BusNode, player: &'bus crate::player::Player) -> Self {
        let clocks = bus
            .subscribe_raw_with_schema(
                super::CLOCK_TOPIC,
                hypha_node::sdk::msgs::rosgraph_msgs::Clock::TYPE_NAME,
                hypha_node::sdk::msgs::rosgraph_msgs::Clock::SCHEMA_HASH,
                hypha_node::sdk::SubscriberOptions::default().keeping(16),
            )
            .expect("a clock reader");
        let config = super::PlaySessionConfig {
            pace: super::Pace::Max,
            publish_clock: true,
            sync: super::SyncConfig { settle: Duration::ZERO, ..super::SyncConfig::default() },
            ..super::PlaySessionConfig::default()
        };
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let session = super::Session::open(
            bus,
            player,
            super::ChannelPublishers {
                publishers: std::collections::HashMap::new(),
                order: Vec::new(),
                titles: std::collections::HashMap::new(),
            },
            config,
            stop,
        )
        .expect("a session");
        let mut session = Self { session, clocks };
        session.session.begin_pass();
        session
    }

    /// Hands the session a message at `offset` from the start; true when it went on.
    fn item(&mut self, offset: u64) -> bool {
        let clock =
            hypha_node::sdk::StreamKey { topic: super::CLOCK_TOPIC.to_owned(), schema_hash: 0 };
        let item = crate::player::PlaybackItem {
            stream: &clock,
            bytes: &[0; 8],
            delay_nanos: 0,
            log_time_nanos: START + offset,
        };
        self.session.on_item(&item)
    }

    /// The `/clock` values sent so far, as offsets from the start.
    fn clocks(&mut self) -> Vec<u64> {
        let mut values = Vec::new();
        while let Some(sample) = self.clocks.try_receive().expect("a clock receive") {
            let payload = sample.bytes().expect("a clock payload");
            let nanos = hypha_node::sdk::RosTime::decode_clock(payload)
                .map(hypha_node::sdk::RosTime::as_nanos)
                .expect("a clock value");
            values.push(nanos - START);
        }
        values
    }
}

#[test]
fn a_restart_closes_the_open_chunk_one_nanosecond_before_the_message_it_stopped_at() {
    let (bus, player) = (bus(), player());
    let mut replay = ClockedSession::open(&bus, &player);
    assert!(replay.item(0) && replay.item(50 * MS));
    // The restart key: the pass stops before the message at 150 ms, before the boundary at
    // 100 ms went out.
    replay.session.restart_requested = true;
    assert!(!replay.item(150 * MS));
    replay.session.finish_pass();
    assert!(replay.session.failure.is_none());
    assert_eq!(replay.clocks(), [0, 150 * MS - 1]);
}

#[test]
fn a_restart_at_the_first_instant_sends_the_start_clock_again() {
    let (bus, player) = (bus(), player());
    let mut replay = ClockedSession::open(&bus, &player);
    assert!(replay.item(0));
    replay.session.restart_requested = true;
    // Stopped before a second message at the start: the clock never goes back in a pass.
    assert!(!replay.item(0));
    replay.session.finish_pass();
    assert_eq!(replay.clocks(), [0, 0]);
}

#[test]
fn a_pass_closes_a_last_message_on_the_grid_then_ends_one_chunk_later() {
    let (bus, player) = (bus(), player());
    let mut replay = ClockedSession::open(&bus, &player);
    assert!(replay.item(0) && replay.item(100 * MS));
    replay.session.finish_pass();
    assert!(replay.session.failure.is_none());
    assert_eq!(replay.clocks(), [0, 100 * MS, 200 * MS]);
}

#[test]
fn a_restart_during_the_tail_still_ends_the_pass() {
    let (bus, player) = (bus(), player());
    let mut replay = ClockedSession::open(&bus, &player);
    assert!(replay.item(0) && replay.item(100 * MS));
    // The tail halts before the boundary at the last message; the end still closes it.
    replay.session.restart_requested = true;
    replay.session.finish_pass();
    assert!(replay.session.failure.is_none());
    assert_eq!(replay.clocks(), [0, 200 * MS]);
}
