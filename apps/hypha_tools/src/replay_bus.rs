//! The bus rules of `hypha play --clock` that need the bus: one player per domain.

use crate::replay_sync::{PROGRESS_TOPIC, Progress};
use hypha_node::sdk::clock_barrier::PROGRESS_DEPTH;
use hypha_node::sdk::{BusNode, RawSubscriber, SubscriberOptions};

/// The `/clock` topic of the replay.
const CLOCK_TOPIC: &str = "/clock";

/// Refuses when another process in the domain already publishes `/clock`.
///
/// The listing does not count this process's own ports, so a publisher that this process
/// creates later is not seen here. A listing that fails is an error, never "no publisher".
///
/// # Errors
/// Returns a message when the topics cannot be listed, or when `/clock` is published elsewhere.
pub fn ensure_sole_clock_publisher(bus: &BusNode) -> Result<(), String> {
    let topics = bus.list_topics()?;
    let taken = topics.iter().any(|topic| topic.name == CLOCK_TOPIC && topic.published);
    if taken {
        return Err("another process already publishes /clock in this domain; one player per \
                    domain (stop it or use another HYPHA_DOMAIN)"
            .to_string());
    }
    Ok(())
}

/// The warning for a replayed topic that another process in the domain already publishes.
#[must_use]
pub fn live_publisher_warning(topic: &str) -> String {
    format!("{topic} already has a live publisher; replayed data will mix with it")
}

/// Warns once per topic in `topics` that another process in the domain already publishes it.
/// A live publisher does not refuse the start. The listing does not count this process's
/// own ports, so the check is made before the replay creates its publishers.
///
/// # Errors
/// Returns the listing error: a listing that fails is an error, never "no publisher".
pub fn warn_live_publishers(bus: &BusNode, topics: &[&str]) -> Result<(), String> {
    let listed = bus.list_topics()?;
    for topic in topics {
        let is_live = listed.iter().any(|entry| entry.name == *topic && entry.published);
        if is_live {
            eprintln!("hypha play: {}", live_publisher_warning(topic));
        }
    }
    Ok(())
}

/// The player's input on `/clock_progress`: the reports of the other participants.
///
/// # Errors
/// Returns the library's message when the subscription is refused.
pub fn progress_subscriber(bus: &BusNode) -> Result<RawSubscriber, String> {
    let options = SubscriberOptions::default().keeping(PROGRESS_DEPTH);
    // Typed: the player may start before any node has opened the topic.
    bus.subscribe_raw_with_schema(
        PROGRESS_TOPIC,
        Progress::TYPE_NAME,
        Progress::SCHEMA_HASH,
        options,
    )
}
