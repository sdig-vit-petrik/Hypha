//! The streams of the topics a node subscribes to, as its housekeeping last found them.
//!
//! A topic carries one stream per schema. Each typed subscriber of one stream watches its
//! topic here. The node's housekeeping, after a turn of the run and never in a step,
//! refreshes every watched topic in a round: after a graph event (an endpoint came or
//! went), at most once per [`SPACING`], and at least once per [`BACKSTOP`]. A refresh
//! reads the topic's schema records and, per stream, makes one open attempt with no retry,
//! reads the nodes of its publisher ports and the states of the nodes registered at that
//! service, and drops the handle. One pass does at most [`WORK_PER_PASS`] such reads; while
//! a round is unfinished the run comes back after [`PENDING_WAKE`]. A topic keeps its last
//! result until its refresh is complete, and keeps it when a read of it fails.
//!
//! A [`Watch`] holds no transport: reading it opens nothing and calls nothing but a lock.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use hypha_iceoryx2::{Iox2Transport, RecordedStream};

/// The reads (a topic's records, one stream's publishers) one housekeeping pass does at most.
const WORK_PER_PASS: usize = 4;

/// The longest time between two rounds without a graph event, in nanoseconds.
const BACKSTOP: u64 = 1_000_000_000;

/// The shortest time between the starts of two rounds, in nanoseconds; a graph event
/// within it waits.
const SPACING: u64 = 100_000_000;

/// How soon the run comes back to a round that a pass left unfinished, in nanoseconds.
const PENDING_WAKE: u64 = 1_000_000;

/// The streams of one topic that have a live publisher, by schema hash, type and MD5.
type Published = Arc<[RecordedStream]>;

/// One watched topic: its published streams, or `None` before its first refresh.
#[derive(Debug)]
pub(crate) struct TopicStreams {
    topic: String,
    published: Mutex<Option<Published>>,
}

/// What a subscriber of the stream `own` reads about the streams of its topic.
#[derive(Debug, Clone)]
pub(crate) struct Watch {
    streams: Arc<TopicStreams>,
    own: u64,
}

impl Watch {
    /// The other streams of the topic with a live publisher: one entry per live version,
    /// so it grows with them. `None` before the first refresh, which is "not checked".
    pub(crate) fn others(&self) -> Option<Vec<RecordedStream>> {
        let published = crate::locked(&self.streams.published).clone()?;
        Some(published.iter().filter(|stream| stream.schema_hash != self.own).cloned().collect())
    }

    /// True when the last refresh found no live publisher of the own stream; false before
    /// the first.
    pub(crate) fn own_is_unpublished(&self) -> bool {
        crate::locked(&self.streams.published)
            .as_ref()
            .is_some_and(|published| published.iter().all(|stream| stream.schema_hash != self.own))
    }
}

/// The watched topics of a node and the round in progress.
#[derive(Debug, Default)]
pub(crate) struct Registry {
    topics: Mutex<Vec<Weak<TopicStreams>>>,
    graph_changed: AtomicBool,
    round: Mutex<Round>,
}

/// One refresh of every watched topic, a few reads per housekeeping pass.
#[derive(Debug, Default)]
struct Round {
    /// Host monotonic time, in nanoseconds, at which the last round started.
    started: Option<u64>,
    /// The topics of the round not reached yet.
    queue: Vec<Arc<TopicStreams>>,
    scan: Option<Scan>,
}

impl Round {
    fn is_pending(&self) -> bool {
        self.scan.is_some() || !self.queue.is_empty()
    }
}

/// The refresh of one topic.
#[derive(Debug)]
struct Scan {
    topic: Arc<TopicStreams>,
    left: Vec<RecordedStream>,
    found: Vec<RecordedStream>,
}

impl Registry {
    /// The watch of the stream `own` of `topic`, sharing the topic with every other
    /// subscriber of it on this node.
    pub(crate) fn watch(&self, topic: &str, own: u64) -> Watch {
        let mut topics = crate::locked(&self.topics);
        topics.retain(|watched| watched.strong_count() > 0);
        let known = topics.iter().filter_map(Weak::upgrade).find(|watched| watched.topic == topic);
        let streams = known.unwrap_or_else(|| {
            let created =
                Arc::new(TopicStreams { topic: topic.to_owned(), published: Mutex::new(None) });
            topics.push(Arc::downgrade(&created));
            self.graph_changed.store(true, Ordering::Relaxed);
            created
        });
        Watch { streams, own }
    }

    /// An endpoint of the domain came or went: refresh at the next round.
    pub(crate) fn graph_changed(&self) {
        self.graph_changed.store(true, Ordering::Relaxed);
    }

    /// The host monotonic time, in nanoseconds, by which the run must come back for the
    /// housekeeping, at `now`: soon while a round is unfinished, else when the next round is
    /// due; `None` with no watched topic.
    pub(crate) fn next_deadline(&self, now: u64) -> Option<i64> {
        let round = self.round.try_lock().ok()?;
        let deadline = if round.is_pending() {
            now.saturating_add(PENDING_WAKE)
        } else {
            let watched = crate::locked(&self.topics).iter().any(|topic| topic.strong_count() > 0);
            if !watched {
                return None;
            }
            let wait = if self.graph_changed.load(Ordering::Relaxed) { SPACING } else { BACKSTOP };
            round.started.map_or(now, |started| started.saturating_add(wait))
        };
        i64::try_from(deadline).ok()
    }

    /// One housekeeping pass at `now`: start a round after a graph event (at most once per
    /// [`SPACING`]) or at the backstop, and do at most [`WORK_PER_PASS`] reads of it. A
    /// round ends when its queue is empty, so it never continues past the topics it began
    /// with.
    pub(crate) fn housekeep(&self, transport: &Iox2Transport, now: u64) {
        let Ok(mut round) = self.round.try_lock() else { return };
        if !round.is_pending() {
            let since = round.started.map(|started| now.saturating_sub(started));
            let backstop = since.is_none_or(|since| since >= BACKSTOP);
            let spaced = since.is_none_or(|since| since >= SPACING);
            let changed = spaced && self.graph_changed.swap(false, Ordering::Relaxed);
            if !(changed || backstop) {
                return;
            }
            round.started = Some(now);
            round.queue = crate::locked(&self.topics).iter().filter_map(Weak::upgrade).collect();
        }
        let mut work = 0_usize;
        while work < WORK_PER_PASS {
            let scan = round.scan.take().or_else(|| {
                work = work.saturating_add(1);
                next_scan(&mut round)
            });
            let Some(mut scan) = scan else { return };
            match probe(transport, &mut scan, &mut work) {
                Ok(true) => *crate::locked(&scan.topic.published) = Some(Arc::from(scan.found)),
                Ok(false) => round.scan = Some(scan),
                Err(cause) => log::warn!(target: "hypha/subscriber",
                    "hypha: cannot read the streams of topic '{}', its last result stays: {cause:?}",
                    scan.topic.topic),
            }
        }
    }
}

/// Probe the streams left in `scan` until `work` reaches [`WORK_PER_PASS`]. `Ok(true)`
/// when the scan is complete.
fn probe(
    transport: &Iox2Transport,
    scan: &mut Scan,
    work: &mut usize,
) -> Result<bool, hypha_core::port::PortError> {
    while *work < WORK_PER_PASS {
        let Some(stream) = scan.left.pop() else { return Ok(true) };
        *work = work.saturating_add(1);
        if transport
            .live_publishers(&scan.topic.topic, stream.schema_hash)?
            .is_some_and(|live| live > 0)
        {
            scan.found.push(stream);
        }
    }
    Ok(scan.left.is_empty())
}

/// The scan of the next topic of the round whose records can be read.
fn next_scan(round: &mut Round) -> Option<Scan> {
    while let Some(topic) = round.queue.pop() {
        match hypha_iceoryx2::topic_records(&topic.topic) {
            Ok(left) => return Some(Scan { topic, left, found: Vec::new() }),
            Err(cause) => log::warn!(target: "hypha/subscriber",
                "hypha: cannot read the streams of topic '{}', its last result stays: {cause:?}",
                topic.topic),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{Registry, TopicStreams, Watch};
    use hypha_iceoryx2::RecordedStream;
    use std::sync::{Arc, Mutex};

    fn stream(schema_hash: u64) -> RecordedStream {
        RecordedStream { schema_hash, data_type: "pkg/T".to_owned(), md5: String::new() }
    }

    #[test]
    fn a_watch_reads_not_checked_then_the_other_streams() {
        let streams =
            Arc::new(TopicStreams { topic: "/t".to_owned(), published: Mutex::new(None) });
        let watch = Watch { streams: Arc::clone(&streams), own: 1 };
        assert_eq!(watch.others(), None, "not checked before the first refresh");
        assert!(!watch.own_is_unpublished());
        *crate::locked(&streams.published) = Some(Arc::from(vec![stream(1), stream(2)]));
        assert_eq!(watch.others(), Some(vec![stream(2)]));
        assert!(!watch.own_is_unpublished());
        *crate::locked(&streams.published) = Some(Arc::from(vec![stream(2)]));
        assert!(watch.own_is_unpublished());
    }

    #[test]
    fn subscribers_of_one_topic_share_its_streams() {
        let registry = Registry::default();
        let first = registry.watch("/t", 1);
        let second = registry.watch("/t", 2);
        let other = registry.watch("/u", 1);
        assert!(Arc::ptr_eq(&first.streams, &second.streams));
        assert!(!Arc::ptr_eq(&first.streams, &other.streams));
    }

    #[test]
    fn the_run_comes_back_for_a_watched_topic_and_not_without_one() {
        let registry = Registry::default();
        let now = 5_000_000_000;
        assert_eq!(registry.next_deadline(now), None, "nothing watched: no wake");
        let _watch = registry.watch("/t", 1);
        assert_eq!(registry.next_deadline(now), Some(5_000_000_000), "a first round is due now");
    }
}
