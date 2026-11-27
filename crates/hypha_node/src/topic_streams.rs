//! The streams of the topics a node subscribes to, as its housekeeping last found them.
//!
//! A topic carries one stream per schema. Each typed subscriber of the node watches its
//! topic here; the node's housekeeping, after a turn of the run and never in a step,
//! refreshes every watched topic after a graph event (an endpoint came or went) and at
//! least once a second. A refresh reads the topic's schema records and makes one open
//! attempt per stream, with no retry, reads its publisher ports and drops the handle. The
//! one stream that every subscriber of the topic on this node reads counts its ports, as
//! those subscribers do; any other counts only publishers whose node is alive, which takes
//! one read of the domain's nodes a round.
//! One housekeeping pass does at most [`OPENS_PER_PASS`] such reads and opens and goes on
//! at the next; a topic keeps its last result until its refresh is complete. A subscriber
//! reads its topic's streams with one lock and one `Arc` clone: no syscall, no open.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use hypha_iceoryx2::{DeadNodes, Iox2Transport, RecordedStream};

/// The reads of a topic's records and opens of a stream one housekeeping pass does at most.
const OPENS_PER_PASS: usize = 4;

/// The longest time between two refreshes of a watched topic without a graph event.
const BACKSTOP: Duration = Duration::from_secs(1);

/// The streams of one topic that have a live publisher, by schema hash, type and MD5.
pub(crate) type Published = Arc<[RecordedStream]>;

/// One watched topic: its published streams, or `None` before its first refresh.
#[derive(Debug)]
pub(crate) struct TopicStreams {
    topic: String,
    published: Mutex<Option<Published>>,
    /// The streams of the topic that subscribers of this node read.
    watched: Mutex<Vec<u64>>,
}

impl TopicStreams {
    /// The topic's streams with a live publisher at the last refresh; `None` before one.
    pub(crate) fn published(&self) -> Option<Published> {
        crate::locked(&self.published).clone()
    }
}

/// What a subscriber of the stream `own` reads about the other streams of its topic.
#[derive(Debug, Clone)]
pub(crate) struct Watch {
    pub(crate) streams: Arc<TopicStreams>,
    pub(crate) own: u64,
}

impl Watch {
    /// The other streams of the topic with a live publisher; `None` before the first
    /// refresh, which is "not checked".
    pub(crate) fn others(&self) -> Option<Vec<RecordedStream>> {
        let published = self.streams.published()?;
        Some(published.iter().filter(|stream| stream.schema_hash != self.own).cloned().collect())
    }

    /// How many other streams of the topic have a live publisher; `None` before the first
    /// refresh.
    pub(crate) fn count(&self) -> Option<u64> {
        let published = self.streams.published()?;
        let others = published.iter().filter(|stream| stream.schema_hash != self.own).count();
        Some(u64::try_from(others).unwrap_or(u64::MAX))
    }

    /// True when the last refresh found no live publisher of the own stream; false before
    /// the first.
    pub(crate) fn own_is_unpublished(&self) -> bool {
        self.streams
            .published()
            .is_some_and(|published| published.iter().all(|stream| stream.schema_hash != self.own))
    }
}

/// The watched topics of a node and the refresh in progress.
#[derive(Debug, Default)]
pub(crate) struct Registry {
    topics: Mutex<Vec<Weak<TopicStreams>>>,
    graph_changed: AtomicBool,
    round: Mutex<Round>,
}

/// One refresh of every watched topic, a few streams per housekeeping pass.
#[derive(Debug, Default)]
struct Round {
    started: Option<Instant>,
    /// The topics of the round not reached yet.
    queue: Vec<Arc<TopicStreams>>,
    scan: Option<Scan>,
    /// The dead nodes of the domain, read once a round when a stream first has a port.
    dead: Option<DeadNodes>,
}

/// The refresh of one topic.
#[derive(Debug)]
struct Scan {
    topic: Arc<TopicStreams>,
    left: Vec<RecordedStream>,
    found: Vec<RecordedStream>,
}

impl Registry {
    /// The watch of the stream `own` of `topic`, shared with every other subscriber of the
    /// topic on this node.
    pub(crate) fn watch(&self, topic: &str, own: u64) -> Watch {
        let mut topics = crate::locked(&self.topics);
        topics.retain(|watched| watched.strong_count() > 0);
        let known = topics.iter().filter_map(Weak::upgrade).find(|watched| watched.topic == topic);
        let streams = known.unwrap_or_else(|| {
            let created = Arc::new(TopicStreams {
                topic: topic.to_owned(),
                published: Mutex::new(None),
                watched: Mutex::new(Vec::new()),
            });
            topics.push(Arc::downgrade(&created));
            self.graph_changed.store(true, Ordering::Relaxed);
            created
        });
        let mut watched = crate::locked(&streams.watched);
        if !watched.contains(&own) {
            watched.push(own);
        }
        drop(watched);
        Watch { streams, own }
    }

    /// An endpoint of the domain came or went: refresh at the next pass.
    pub(crate) fn graph_changed(&self) {
        self.graph_changed.store(true, Ordering::Relaxed);
    }

    /// One housekeeping pass at `now`: start a round after a graph event or the backstop,
    /// and open at most [`OPENS_PER_PASS`] streams of it. A failed read leaves the topic's
    /// last result and is tried again at the next round.
    pub(crate) fn housekeep(&self, transport: &Iox2Transport, now: Instant) {
        let Ok(mut round) = self.round.try_lock() else { return };
        let idle = round.scan.is_none() && round.queue.is_empty();
        let backstop = round.started.is_none_or(|started| now.duration_since(started) >= BACKSTOP);
        if idle && (self.graph_changed.swap(false, Ordering::Relaxed) || backstop) {
            round.started = Some(now);
            round.queue = crate::locked(&self.topics).iter().filter_map(Weak::upgrade).collect();
            round.dead = None;
        }
        // A topic's read, a stream's open and the read of the dead nodes are one unit each.
        let mut work = 0_usize;
        while work < OPENS_PER_PASS {
            let scan = round.scan.take().or_else(|| {
                work = work.saturating_add(1);
                next_scan(&mut round)
            });
            let Some(mut scan) = scan else { return };
            while work < OPENS_PER_PASS {
                let Some(stream) = scan.left.pop() else { break };
                work = work.saturating_add(1);
                let ports = transport.publisher_ports(&scan.topic.topic, stream.schema_hash);
                let Ok(Some(ports)) = ports else { continue };
                if ports.is_empty() {
                    continue;
                }
                // The stream of every subscriber of the topic on this node counts its ports,
                // as those subscribers do; one that is another stream to any of them must
                // have a live one, which takes the dead nodes of the domain.
                let only_own =
                    crate::locked(&scan.topic.watched).iter().all(|own| *own == stream.schema_hash);
                if only_own {
                    scan.found.push(stream);
                    continue;
                }
                if round.dead.is_none() {
                    work = work.saturating_add(1);
                    round.dead = transport.dead_nodes().ok();
                }
                if round.dead.as_ref().is_some_and(|dead| ports.live(dead) > 0) {
                    scan.found.push(stream);
                }
            }
            if scan.left.is_empty() {
                *crate::locked(&scan.topic.published) = Some(Arc::from(scan.found));
            } else {
                round.scan = Some(scan);
            }
        }
    }
}

/// The scan of the next topic of the round whose records can be read.
fn next_scan(round: &mut Round) -> Option<Scan> {
    while let Some(topic) = round.queue.pop() {
        match hypha_iceoryx2::topic_records(&topic.topic) {
            Ok(left) => return Some(Scan { topic, left, found: Vec::new() }),
            Err(cause) => log::warn!(target: "hypha/subscriber",
                "hypha: cannot read the streams of topic '{}': {cause:?}", topic.topic),
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
        let streams = Arc::new(TopicStreams {
            topic: "/t".to_owned(),
            published: Mutex::new(None),
            watched: Mutex::new(vec![1]),
        });
        let watch = Watch { streams: Arc::clone(&streams), own: 1 };
        assert_eq!(watch.count(), None, "not checked before the first refresh");
        assert!(!watch.own_is_unpublished());
        *crate::locked(&streams.published) = Some(Arc::from(vec![stream(1), stream(2)]));
        assert_eq!(watch.count(), Some(1));
        assert_eq!(watch.others().map(|others| others.len()), Some(1));
        assert!(!watch.own_is_unpublished());
        *crate::locked(&streams.published) = Some(Arc::from(vec![stream(2)]));
        assert!(watch.own_is_unpublished());
    }

    /// The read path holds no transport, so it cannot open anything; it costs a lock and
    /// an `Arc` clone.
    #[test]
    fn a_read_is_a_lock_and_a_clone() {
        let streams = Arc::new(TopicStreams {
            topic: "/t".to_owned(),
            published: Mutex::new(Some(Arc::from(vec![stream(1), stream(2), stream(3)]))),
            watched: Mutex::new(vec![1]),
        });
        let watch = Watch { streams, own: 1 };
        let rounds = 100_000_u32;
        let start = std::time::Instant::now();
        for _ in 0..rounds {
            assert_eq!(std::hint::black_box(&watch).count(), Some(2));
        }
        println!("one read: {:?}", start.elapsed() / rounds);
    }

    /// No step path refreshes: the cycle's capture and the subscriber's checks only read.
    #[test]
    fn no_step_path_refreshes_or_opens() {
        let step_paths = [
            ("cycle/source.rs", include_str!("cycle/source.rs")),
            ("cycle/tick.rs", include_str!("cycle/tick.rs")),
            ("subscriber.rs", include_str!("subscriber.rs")),
            ("node_state.rs", include_str!("node_state.rs")),
        ];
        for (file, text) in step_paths {
            for call in [".housekeep(", ".publisher_ports(", "topic_records(", ".dead_nodes("] {
                assert!(!text.contains(call), "{file} calls {call}");
            }
        }
        let dispatch = include_str!("waitset/dispatch.rs");
        assert_eq!(dispatch.matches(".housekeep(").count(), 1, "one call, after the turn");
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
}
