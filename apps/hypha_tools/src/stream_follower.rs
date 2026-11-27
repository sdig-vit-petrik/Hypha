//! Follows every stream of the bus whose topic name matches a set of subscription keys,
//! including a stream that joins a followed name later (topic name overloading, #209).
//!
//! A subscription that names no schema reads the streams that exist when it is made, so a
//! tool that outlives the system it observes opens one subscription for each stream it is
//! shown. It is shown them by the graph event, the one a node's housekeeping hears too: a
//! publisher or a subscriber that came or went. The watch is opened before the first listing,
//! so no stream falls between the two.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use hypha_node::sdk::{
    BusNode, GraphWatch, RawSubscriber, ReceivedSample, SubscriberOptions, topic_matches_pattern,
};

use crate::stream::{StreamKey, StreamSelector, stream_label, stream_title};

/// The longest a stream goes unlisted when its notification is lost: the sender logs a
/// failed notification and the listener never hears it. Slow, because the event is what
/// follows streams; this only bounds the damage of a lost one.
const BACKSTOP: Duration = Duration::from_secs(2);

/// One subscription for each followed stream, refreshed when the graph changes.
pub struct StreamFollower {
    keys: Vec<String>,
    selector: StreamSelector,
    depth: usize,
    graph: GraphWatch,
    next_backstop: Instant,
    subscribers: BTreeMap<StreamKey, RawSubscriber>,
    order: Vec<StreamKey>,
    next_index: usize,
}

impl StreamFollower {
    /// Follows the streams of topics matching any of `keys` that `selector` selects,
    /// buffering `depth` samples each. Follows none until the first [`Self::refresh`].
    ///
    /// # Errors
    /// Returns an error when the graph event cannot be watched.
    pub fn new(
        bus: &BusNode,
        keys: Vec<String>,
        selector: StreamSelector,
        depth: usize,
    ) -> Result<Self, String> {
        Ok(Self {
            keys,
            selector,
            depth,
            graph: bus.graph_watch()?,
            next_backstop: Instant::now(),
            subscribers: BTreeMap::new(),
            order: Vec::new(),
            next_index: 0,
        })
    }

    /// Count of streams this follower currently reads.
    #[must_use]
    pub fn followed(&self) -> usize {
        self.subscribers.len()
    }

    /// Whether `stream`'s topic name has more than one followed stream, so that its type
    /// and MD5 are what tells its samples from another's.
    #[must_use]
    pub fn shares_its_name(&self, stream: &StreamKey) -> bool {
        self.order.iter().filter(|followed| followed.topic == stream.topic).take(2).count() > 1
    }

    /// Lists the streams and subscribes to every new match. Reports how many were added.
    ///
    /// # Errors
    /// Returns an error when the streams cannot be listed or a matching stream refuses
    /// its subscription. The other matching streams are still added.
    pub fn refresh(&mut self, bus: &BusNode) -> Result<usize, String> {
        self.next_backstop = Instant::now().checked_add(BACKSTOP).unwrap_or_else(Instant::now);
        let mut added = 0_usize;
        let mut failures = Vec::new();
        for stream in bus.list_topics()? {
            let key = StreamKey::from(&stream);
            if self.subscribers.contains_key(&key) || !self.wanted(bus, &key) {
                continue;
            }
            let options = SubscriberOptions::default().keeping(self.depth);
            match bus.subscribe_raw_stream(&key.topic, key.schema_hash, options) {
                Ok(subscriber) => {
                    self.order.push(key.clone());
                    self.subscribers.insert(key, subscriber);
                    added = added.saturating_add(1);
                }
                Err(error) => failures.push(format!("cannot follow '{}': {error}", key.topic)),
            }
        }
        if failures.is_empty() { Ok(added) } else { Err(failures.join("; ")) }
    }

    /// Refreshes when the graph changed since the last call, or after the backstop: one
    /// non-blocking read otherwise.
    ///
    /// # Errors
    /// Returns an error when the graph event cannot be read, or as [`Self::refresh`] does.
    pub fn refresh_if_changed(&mut self, bus: &BusNode) -> Result<usize, String> {
        if !self.graph.changed()? && Instant::now() < self.next_backstop {
            return Ok(0);
        }
        self.refresh(bus)
    }

    /// `stream` under its topic name, with its type and MD5 start when the name has other
    /// followed streams. A schema that cannot be read now is shown as its hash.
    #[must_use]
    pub fn title(&self, bus: &BusNode, stream: &StreamKey) -> String {
        let shared = self.shares_its_name(stream);
        if !shared {
            return stream.topic.clone();
        }
        let schema = bus.resolve_schema_hash_now(&stream.topic, stream.schema_hash).ok().flatten();
        stream_title(&stream.topic, &stream_label(schema.as_ref(), stream.schema_hash), shared)
    }

    /// Takes the next sample from any followed stream, as the transport's loan: the caller
    /// copies only what it keeps. Drop it before the next call; a held loan counts against
    /// its subscription's borrow budget.
    ///
    /// Reads the streams in turn, so one busy stream cannot starve the others. Returns
    /// `None` once every followed stream is empty.
    ///
    /// # Errors
    /// Returns an error as soon as one stream reports it. The next call resumes with the
    /// following stream, so healthy traffic continues.
    pub fn next_sample(&mut self) -> Result<Option<ReceivedSample>, String> {
        if self.order.is_empty() {
            return Ok(None);
        }
        let followed = self.order.len();
        for _step in 0..followed {
            // `followed` is non-zero here, so the remainder is always a valid
            // index and cannot divide by zero.
            let index = self.next_index.checked_rem(followed).unwrap_or(0);
            self.next_index = index.saturating_add(1);
            let Some(key) = self.order.get(index) else { continue };
            let Some(subscriber) = self.subscribers.get_mut(key) else { continue };
            match subscriber.try_receive() {
                Ok(Some(received)) => return Ok(Some(received)),
                Ok(None) => {}
                Err(error) => {
                    return Err(format!("receive from '{}' failed: {error}", key.topic));
                }
            }
        }
        Ok(None)
    }

    /// Followed streams that lost frames before this follower read them, in follow order,
    /// with the cumulative count for each. Empty when none did.
    ///
    /// # Errors
    /// Returns an error when a followed subscription cannot report its count.
    pub fn lost(&self) -> Result<Vec<(StreamKey, u64)>, String> {
        let mut lost = Vec::new();
        for key in &self.order {
            let Some(subscriber) = self.subscribers.get(key) else { continue };
            let missing = subscriber.missing_sequences()?;
            if missing > 0 {
                lost.push((key.clone(), missing));
            }
        }
        Ok(lost)
    }

    /// The names in `topics` of which this follower reads no stream yet.
    #[must_use]
    pub fn missing(&self, topics: &[String]) -> Vec<String> {
        topics
            .iter()
            .filter(|name| !self.order.iter().any(|followed| followed.topic == **name))
            .cloned()
            .collect()
    }

    /// Whether `stream` is one to follow: its name matches a key and, when a selector
    /// names a type or an MD5, its schema is known and matches.
    fn wanted(&self, bus: &BusNode, stream: &StreamKey) -> bool {
        if !self.keys.iter().any(|key| topic_matches_pattern(key, &stream.topic)) {
            return false;
        }
        if self.selector.is_open() {
            return true;
        }
        let schema = bus.resolve_schema_hash_now(&stream.topic, stream.schema_hash).ok().flatten();
        self.selector.selects(schema.as_ref())
    }
}
