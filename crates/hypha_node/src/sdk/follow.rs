//! Following the streams of the bus as they appear (topic name overloading, #209).
//!
//! A subscription that names no schema reads the streams that exist when it is made. A reader
//! that outlives the system it observes, a tool or a bridge, opens one subscription for each
//! stream it is shown, and is shown them by the graph event, the one a node's housekeeping hears
//! too: a publisher or a subscriber that came or went. The watch is opened before the first
//! listing, so no stream falls between the two.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::sdk::{
    BusNode, GraphWatch, RawSubscriber, ReceivedSample, ResolvedSchema, SampleIdentity,
    SubscriberOptions, TopicInfo, topic_matches_pattern,
};

/// One stream of the bus: the frames of one schema on one topic name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StreamKey {
    /// The topic name.
    pub topic: String,
    /// The schema hash of the stream's frames, 0 for the stream of no schema.
    pub schema_hash: u64,
}

impl StreamKey {
    /// The stream a received frame arrived on.
    #[must_use]
    pub fn of(identity: &SampleIdentity<'_>) -> Self {
        Self { topic: identity.topic.to_owned(), schema_hash: identity.schema_hash }
    }
}

impl From<&TopicInfo> for StreamKey {
    /// The stream a bus listing shows.
    fn from(info: &TopicInfo) -> Self {
        Self { topic: info.name.clone(), schema_hash: info.schema_hash }
    }
}

/// Which streams of a topic name a follower follows: all of them, or those of one message type
/// and/or one MD5.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StreamSelector {
    /// Only the streams of this message type.
    pub data_type: Option<String>,
    /// Only the streams whose MD5 starts with these hex digits, not case sensitive.
    pub md5_prefix: Option<String>,
}

impl StreamSelector {
    /// Whether the selector names nothing, so it needs no schema to decide.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.data_type.is_none() && self.md5_prefix.is_none()
    }

    /// Whether a stream of `schema` is selected. A stream whose schema is unknown is selected
    /// by no flag: it cannot be told to match.
    #[must_use]
    pub fn selects(&self, schema: Option<&ResolvedSchema>) -> bool {
        if self.is_open() {
            return true;
        }
        let Some(schema) = schema else { return false };
        let type_matches = self.data_type.as_ref().is_none_or(|wanted| *wanted == schema.data_type);
        let md5_matches = self.md5_prefix.as_ref().is_none_or(|prefix| {
            schema.md5.to_ascii_lowercase().starts_with(&prefix.to_ascii_lowercase())
        });
        type_matches && md5_matches
    }
}

impl std::fmt::Display for StreamSelector {
    /// The selection as the flags that name it: `--type <type> --md5 <prefix>`.
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let flags: Vec<String> = [
            self.data_type.as_ref().map(|data_type| format!("--type {data_type}")),
            self.md5_prefix.as_ref().map(|prefix| format!("--md5 {prefix}")),
        ]
        .into_iter()
        .flatten()
        .collect();
        write!(out, "{}", flags.join(" "))
    }
}

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

#[cfg(test)]
mod tests {
    use super::StreamSelector;
    use crate::sdk::ResolvedSchema;

    fn schema(data_type: &str, md5: &str) -> ResolvedSchema {
        ResolvedSchema {
            data_type: data_type.to_owned(),
            md5: md5.to_owned(),
            definition: String::new(),
        }
    }

    const V1: &str = "b3087778e93fcd34cc8d65bc54e850d1";

    #[test]
    fn an_open_selector_selects_every_stream_even_one_of_no_known_schema() {
        assert!(StreamSelector::default().selects(None));
        assert!(StreamSelector::default().selects(Some(&schema("a/B", V1))));
    }

    #[test]
    fn a_type_and_an_md5_prefix_must_both_match() {
        let by_type = StreamSelector { data_type: Some("a/B".into()), md5_prefix: None };
        let by_md5 = StreamSelector { data_type: None, md5_prefix: Some("B308".into()) };
        let both =
            StreamSelector { data_type: Some("a/C".into()), md5_prefix: Some("b308".into()) };
        let this = schema("a/B", V1);
        assert!(by_type.selects(Some(&this)));
        assert!(by_md5.selects(Some(&this)), "a prefix is not case sensitive");
        assert!(!both.selects(Some(&this)), "the type differs");
        assert!(!by_type.selects(Some(&schema("a/C", V1))));
        assert!(!by_md5.selects(Some(&schema("a/B", "f79835def461350458ead76f07016dd7"))));
    }

    #[test]
    fn a_flag_selects_no_stream_whose_schema_is_unknown() {
        let by_md5 = StreamSelector { data_type: None, md5_prefix: Some("b308".into()) };
        assert!(!by_md5.selects(None));
    }

    #[test]
    fn a_selector_reads_back_as_its_flags() {
        let both = StreamSelector { data_type: Some("a/B".into()), md5_prefix: Some("b3".into()) };
        assert_eq!(both.to_string(), "--type a/B --md5 b3");
    }
}
