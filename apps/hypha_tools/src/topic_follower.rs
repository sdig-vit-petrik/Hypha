//! Follows every bus topic that matches a set of subscription keys.
//!
//! A pattern subscription resolves its topics once, when it is created, so a tool that
//! outlives the system it observes lists topics on a cadence it chooses.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use hypha_node::sdk::{
    BusNode, RawSubscriber, ReceivedSample, SubscriberOptions, topic_matches_pattern,
};

/// One subscription per followed topic, refreshed on a caller-chosen cadence.
pub struct TopicFollower {
    keys: Vec<String>,
    depth: usize,
    interval: Duration,
    next_refresh: Instant,
    subscribers: BTreeMap<String, RawSubscriber>,
    order: Vec<String>,
    next_index: usize,
}

impl TopicFollower {
    /// Follows topics matching any of `keys`, buffering `depth` samples each.
    #[must_use]
    pub fn new(keys: Vec<String>, depth: usize, interval: Duration) -> Self {
        Self {
            keys,
            depth,
            interval,
            next_refresh: Instant::now(),
            subscribers: BTreeMap::new(),
            order: Vec::new(),
            next_index: 0,
        }
    }

    /// Count of topics this follower currently reads.
    #[must_use]
    pub fn followed(&self) -> usize {
        self.subscribers.len()
    }

    /// Lists topics and subscribes to every new match. Reports how many were
    /// added.
    ///
    /// # Errors
    ///
    /// Returns an error when the topic registry cannot be listed or a matching
    /// topic refuses its subscription. Other matching topics are still added.
    pub fn refresh(&mut self, bus: &BusNode) -> Result<usize, String> {
        self.next_refresh = Instant::now().checked_add(self.interval).unwrap_or_else(Instant::now);
        let topics = bus.list_topics()?;
        let mut added = 0_usize;
        let mut failures = Vec::new();
        for topic in topics {
            let already_followed = self.subscribers.contains_key(&topic.name);
            if already_followed || !self.wanted(&topic.name) {
                continue;
            }
            let options = SubscriberOptions::default().keeping(self.depth);
            let subscriber = match bus.subscribe_raw_with_options(&topic.name, options) {
                Ok(subscriber) => subscriber,
                Err(error) => {
                    failures.push(format!("cannot follow '{}': {error}", topic.name));
                    continue;
                }
            };
            self.order.push(topic.name.clone());
            self.subscribers.insert(topic.name, subscriber);
            added = added.saturating_add(1);
        }
        if !failures.is_empty() {
            return Err(failures.join("; "));
        }
        Ok(added)
    }

    /// Refreshes only once the configured interval has passed.
    ///
    /// # Errors
    ///
    /// Returns an error when the topic registry cannot be listed.
    pub fn refresh_if_due(&mut self, bus: &BusNode) -> Result<usize, String> {
        if Instant::now() < self.next_refresh {
            return Ok(0);
        }
        self.refresh(bus)
    }

    /// Takes the next sample from any followed topic, as the transport's loan:
    /// the caller copies only what it keeps. Drop it before the next call; a
    /// held loan counts against its subscription's borrow budget.
    ///
    /// Reads the topics in turn, so one busy topic cannot starve the others.
    /// Returns `None` once every followed topic is empty.
    ///
    /// # Errors
    ///
    /// Returns an error as soon as one topic reports it. The next call resumes
    /// with the following topic, so healthy traffic continues.
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
            let Some(name) = self.order.get(index) else { continue };
            let Some(subscriber) = self.subscribers.get_mut(name) else { continue };
            match subscriber.try_receive() {
                Ok(Some(received)) => return Ok(Some(received)),
                Ok(None) => {}
                Err(error) => return Err(format!("receive from '{name}' failed: {error}")),
            }
        }
        Ok(None)
    }

    /// Followed topics that lost frames before this follower read them, in
    /// follow order, with the cumulative count for each. Empty when none did.
    ///
    /// # Errors
    /// Returns an error when a followed subscription cannot report its count.
    pub fn lost(&self) -> Result<Vec<(String, u64)>, String> {
        let mut lost = Vec::new();
        for name in &self.order {
            let Some(subscriber) = self.subscribers.get(name) else { continue };
            let missing = subscriber.missing_sequences()?;
            if missing > 0 {
                lost.push((name.clone(), missing));
            }
        }
        Ok(lost)
    }

    /// The names in `topics` that this follower does not read yet.
    #[must_use]
    pub fn missing(&self, topics: &[String]) -> Vec<String> {
        topics
            .iter()
            .filter(|name| !self.subscribers.contains_key(name.as_str()))
            .cloned()
            .collect()
    }

    fn wanted(&self, topic: &str) -> bool {
        self.keys.iter().any(|key| topic_matches_pattern(key, topic))
    }
}
