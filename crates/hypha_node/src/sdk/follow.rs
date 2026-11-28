//! Following the streams of the bus as they appear (topic name overloading, #209).
//!
//! A subscription that names no schema reads the streams that exist when it is made. A reader
//! that outlives the system it observes, a tool or a bridge, opens one subscription for each
//! stream it is shown, and is shown them by the graph event, the one a node's housekeeping hears
//! too: a publisher or a subscriber that came or went. The watch is opened before the first
//! listing, so no stream falls between the two.
//!
//! Two parts, neither of which creates a thread. [`StreamListings`] waits for the event and
//! lists the streams, which takes tens of milliseconds, so a reader that must not stop reading
//! runs it on a thread of its own; [`StreamFollower`] takes a listing and subscribes to the new
//! streams, which is cheap, on the reader's thread.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use crate::sdk::{
    BusNode, GraphWatch, RawSubscriber, ReceivedSample, ResolvedSchema, SampleIdentity,
    StreamSubscribeError, SubscriberOptions, TopicInfo, topic_matches_pattern,
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

/// The shortest time between two listings, and at least [`IDLE_SHARE`] times the last
/// listing's own duration. A listing reads every service of the domain, which takes tens of
/// milliseconds in a domain of thousands, so a burst of events, a system starting, is one
/// listing; the events a follower's own subscriptions raise wait too.
const SPACING: Duration = Duration::from_millis(100);

/// How many times longer than a listing the next one waits: the thread that lists spends at
/// most one fifth of its time listing, however busy the graph is.
const IDLE_SHARE: u32 = 4;

/// Lists the streams of the bus when its graph changes.
pub struct StreamListings {
    graph: GraphWatch,
    /// An event came since the last listing.
    pending: bool,
    next_listing: Instant,
    next_backstop: Instant,
}

impl StreamListings {
    /// Opens the watch of the graph event. List once before relying on it: an endpoint that
    /// came earlier raised an event nobody heard.
    ///
    /// # Errors
    /// Returns a message when the graph event cannot be watched.
    pub fn new(bus: &BusNode) -> Result<Self, String> {
        let now = Instant::now();
        Ok(Self {
            graph: bus.graph_watch()?,
            pending: false,
            next_listing: now,
            next_backstop: now,
        })
    }

    /// Lists the streams now, whatever the graph did.
    ///
    /// # Errors
    /// Returns a message when the streams cannot be listed. The next listing is due as after
    /// a good one: a listing that fails is not retried at once, nor reported on every pass.
    pub fn list(&mut self, bus: &BusNode) -> Result<Vec<TopicInfo>, String> {
        self.take_listing(|| bus.list_topics())
    }

    /// Lists the streams when the graph changed or the backstop passed, without waiting and
    /// without the spacing: for a reader that must have every stream there is before it goes
    /// on, such as a recorder that answers a replay's chunk.
    ///
    /// # Errors
    /// Returns a message when the graph event cannot be read or the streams cannot be listed.
    pub fn list_if_changed(&mut self, bus: &BusNode) -> Result<Option<Vec<TopicInfo>>, String> {
        self.pending |= self.graph.changed()?;
        if self.pending || Instant::now() >= self.next_backstop {
            return self.list(bus).map(Some);
        }
        Ok(None)
    }

    /// Waits up to `wait` for the graph to change and lists the streams, at most once per
    /// spacing; lists after the backstop when it does not. `None` when `wait` passed with
    /// nothing to list. The thread sleeps in the kernel: this is the form for a thread of
    /// the caller's own.
    ///
    /// # Errors
    /// Returns a message when the graph event cannot be read or the streams cannot be listed.
    pub fn wait_for_listing(
        &mut self,
        bus: &BusNode,
        wait: Duration,
    ) -> Result<Option<Vec<TopicInfo>>, String> {
        let deadline = Instant::now().checked_add(wait).unwrap_or_else(Instant::now);
        loop {
            let now = Instant::now();
            let ready_at = if self.pending { self.next_listing } else { self.next_backstop };
            if now >= ready_at {
                return self.list(bus).map(Some);
            }
            if now >= deadline {
                return Ok(None);
            }
            let until = ready_at.min(deadline).saturating_duration_since(now);
            if self.pending {
                std::thread::sleep(until);
            } else {
                self.pending = self.graph.wait(until)?;
            }
        }
    }

    /// One listing by `list`, and when the next may come: both set before the result is read,
    /// so a failure waits as a success does.
    fn take_listing(
        &mut self,
        list: impl FnOnce() -> Result<Vec<TopicInfo>, String>,
    ) -> Result<Vec<TopicInfo>, String> {
        // The events up to now are covered by the listing that follows.
        let taken = self.graph.changed();
        self.pending = false;
        let started = Instant::now();
        let listed = list();
        let done = Instant::now();
        let spacing = SPACING.max(done.duration_since(started).saturating_mul(IDLE_SHARE));
        self.next_listing = done.checked_add(spacing).unwrap_or(done);
        self.next_backstop = done.checked_add(BACKSTOP).unwrap_or(done);
        taken?;
        listed
    }
}

/// A stream that refused its subscription.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// The stream.
    pub stream: StreamKey,
    /// Why.
    pub cause: StreamSubscribeError,
}

/// Why a listing could not be followed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FollowError {
    /// The streams could not be listed.
    Listing(String),
    /// These streams exist and match, and refused their subscription. The others were followed.
    Refused(Vec<Refusal>),
}

impl std::fmt::Display for FollowError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Listing(message) => write!(out, "cannot list the streams: {message}"),
            Self::Refused(refusals) => {
                let each: Vec<String> = refusals
                    .iter()
                    .map(|refusal| format!("'{}': {}", refusal.stream.topic, refusal.cause))
                    .collect();
                write!(out, "cannot follow {}", each.join("; "))
            }
        }
    }
}

impl std::error::Error for FollowError {}

/// One followed stream.
struct Followed {
    subscriber: RawSubscriber,
    /// The publishers the last listing showed on it.
    publishers: usize,
    /// What the subscriber counted as sent before its first frame when the last listing was
    /// applied.
    counted_at_listing: u64,
    /// From which of those counts a publisher is known to have come after the follower: `None`
    /// while every publisher was there before it.
    counted_from: Option<u64>,
}

/// One subscription for each followed stream, fed with listings.
pub struct StreamFollower {
    keys: Vec<String>,
    selector: StreamSelector,
    depth: usize,
    /// Whether a listing was applied: a stream first listed afterwards came after the follower.
    started: bool,
    followed: BTreeMap<StreamKey, Followed>,
    order: Vec<StreamKey>,
    /// The streams of followed names that the selector let go, with their schema when known.
    skipped: BTreeMap<StreamKey, Option<ResolvedSchema>>,
    next_index: usize,
}

impl StreamFollower {
    /// Follows the streams of topics matching any of `keys` that `selector` selects,
    /// buffering `depth` samples each. Follows none until a listing is applied.
    #[must_use]
    pub fn new(keys: Vec<String>, selector: StreamSelector, depth: usize) -> Self {
        Self {
            keys,
            selector,
            depth,
            started: false,
            followed: BTreeMap::new(),
            order: Vec::new(),
            skipped: BTreeMap::new(),
            next_index: 0,
        }
    }

    /// Count of streams this follower currently reads.
    #[must_use]
    pub fn followed(&self) -> usize {
        self.followed.len()
    }

    /// The streams of the followed names that the selector let go, with their schema when it is
    /// known: what to tell a reader whose flags matched nothing.
    #[must_use]
    pub fn skipped(&self) -> Vec<(&StreamKey, Option<&ResolvedSchema>)> {
        self.skipped.iter().map(|(stream, schema)| (stream, schema.as_ref())).collect()
    }

    /// The publishers the last listing showed on a followed stream.
    #[must_use]
    pub fn publishers(&self, stream: &StreamKey) -> Option<usize> {
        self.followed.get(stream).map(|followed| followed.publishers)
    }

    /// The publishers the last listing showed on each followed stream.
    #[must_use]
    pub fn publisher_counts(&self) -> BTreeMap<StreamKey, usize> {
        self.followed
            .iter()
            .map(|(stream, followed)| (stream.clone(), followed.publishers))
            .collect()
    }

    /// Whether `stream`'s topic name has more than one followed stream with a publisher, so
    /// that its type and MD5 are what tells its samples from another's. A stream only read by
    /// others is not one: nothing is sent on it.
    #[must_use]
    pub fn shares_its_name(&self, stream: &StreamKey) -> bool {
        self.order
            .iter()
            .filter(|other| other.topic == stream.topic)
            .filter(|other| self.publishers(other).is_some_and(|publishers| publishers > 0))
            .take(2)
            .count()
            > 1
    }

    /// Lists the streams and follows the new matches: the one-shot form, on the caller's
    /// thread.
    ///
    /// # Errors
    /// As [`StreamFollower::apply`], or the listing's.
    pub fn refresh(&mut self, bus: &BusNode) -> Result<usize, FollowError> {
        let listing = bus.list_topics().map_err(FollowError::Listing)?;
        self.apply(bus, &listing)
    }

    /// Follows the streams of `listing` that match and are not followed yet. Reports how many
    /// were added. A stream first listed by a later listing than the first came after the
    /// follower, and so does a publisher that joins a followed stream: what they sent before
    /// the follower read them is told by [`StreamFollower::sent_before_following`].
    ///
    /// # Errors
    /// [`FollowError::Refused`] when a matching stream refuses its subscription; the other
    /// matching streams are still added, and a refused stream is tried again by the next
    /// listing.
    pub fn apply(&mut self, bus: &BusNode, listing: &[TopicInfo]) -> Result<usize, FollowError> {
        let came_after = self.started;
        self.started = true;
        let mut added = 0_usize;
        let mut refusals = Vec::new();
        let mut present = BTreeSet::new();
        for info in listing {
            let key = StreamKey::from(info);
            if !self.keys.iter().any(|pattern| topic_matches_pattern(pattern, &key.topic)) {
                continue;
            }
            present.insert(key.clone());
            if let Some(followed) = self.followed.get_mut(&key) {
                if info.publishers > followed.publishers && followed.counted_from.is_none() {
                    // A publisher joined: the frames it sent before the last listing are
                    // counted from there, in case its first frame was read since.
                    followed.counted_from = Some(followed.counted_at_listing);
                }
                followed.publishers = info.publishers;
                followed.counted_at_listing = followed.subscriber.sent_before_first();
                continue;
            }
            let schema = (!self.selector.is_open())
                .then(|| bus.resolve_schema_hash_now(&key.topic, key.schema_hash).ok().flatten())
                .flatten();
            if !self.selector.selects(schema.as_ref()) {
                self.skipped.insert(key, schema);
                continue;
            }
            self.skipped.remove(&key);
            let options = SubscriberOptions::default().keeping(self.depth);
            match bus.subscribe_raw_stream(&key.topic, key.schema_hash, options) {
                Ok(subscriber) => {
                    self.order.push(key.clone());
                    self.followed.insert(
                        key,
                        Followed {
                            subscriber,
                            publishers: info.publishers,
                            counted_at_listing: 0,
                            counted_from: came_after.then_some(0),
                        },
                    );
                    added = added.saturating_add(1);
                }
                Err(cause) => refusals.push(Refusal { stream: key, cause }),
            }
        }
        self.skipped.retain(|stream, _| present.contains(stream));
        if refusals.is_empty() { Ok(added) } else { Err(FollowError::Refused(refusals)) }
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
            let Some(stream) = self.followed.get_mut(key) else { continue };
            match stream.subscriber.try_receive() {
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
            let Some(followed) = self.followed.get(key) else { continue };
            let missing = followed.subscriber.missing_sequences()?;
            if missing > 0 {
                lost.push((key.clone(), missing));
            }
        }
        Ok(lost)
    }

    /// Followed streams of which frames were sent before the follower followed them, in follow
    /// order, with the cumulative count for each. A publisher's first frame has sequence 1, so the
    /// first sequence the follower accepts from one, less one, is the number of its frames it
    /// never saw.
    ///
    /// The count is the evidence of a recording that begins late, not a loss to the transport,
    /// and it is made only where the publisher is known to have come after the follower: every
    /// publisher of a stream that a later listing than the first showed, and the publishers
    /// that joined a followed stream since a listing showed one more (a publisher that joins a
    /// stream already published is told from a quiet one by that count alone). A joined
    /// publisher whose port registered after the stream's subscription owed it every frame, so
    /// the frames before its first accepted one are lost ([`StreamFollower::lost`]), not
    /// counted here. A publisher that existed when the follower started is not counted: its
    /// history is not the recording's. A latched publisher's replayed sample is not counted
    /// either.
    #[must_use]
    pub fn sent_before_following(&self) -> Vec<(StreamKey, u64)> {
        self.order
            .iter()
            .filter_map(|key| {
                let followed = self.followed.get(key)?;
                let from = followed.counted_from?;
                let sent = followed.subscriber.sent_before_first().saturating_sub(from);
                (sent > 0).then(|| (key.clone(), sent))
            })
            .collect()
    }

    /// The names in `topics` of which this follower reads no stream with a publisher yet.
    #[must_use]
    pub fn missing(&self, topics: &[String]) -> Vec<String> {
        topics
            .iter()
            .filter(|name| {
                !self.order.iter().any(|stream| {
                    stream.topic == **name
                        && self.publishers(stream).is_some_and(|publishers| publishers > 0)
                })
            })
            .cloned()
            .collect()
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
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

    /// A listing that fails is not retried on every pass, nor reported on each (M3 of the fourth
    /// review of #268): its next time and the backstop are set before its result is read.
    #[test]
    fn a_failed_listing_is_not_due_again_at_once() {
        let bus = crate::sdk::BusNode::new(&format!("follow_failed_{}", std::process::id()))
            .expect("a bus node");
        let mut listings = super::StreamListings::new(&bus).expect("listings");
        let failed = listings.take_listing(|| Err("the registry is unreadable".to_owned()));
        assert_eq!(failed, Err("the registry is unreadable".to_owned()));
        assert!(!listings.pending);
        assert!(listings.next_listing > std::time::Instant::now());
        assert!(listings.next_backstop > std::time::Instant::now());
        assert!(
            listings.list_if_changed(&bus).expect("a read").is_none(),
            "nothing is due right after a failure"
        );
    }
}
