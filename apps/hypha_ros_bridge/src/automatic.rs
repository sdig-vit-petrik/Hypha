//! Automatic same-name route discovery and reconciliation primitives.

use std::collections::{BTreeMap, BTreeSet};

use hypha_node::sdk::StreamKey;

/// The two cross-side data paths that automatic mode can create.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoFlow {
    /// A ROS publisher feeds an hypha subscriber.
    RosToHypha,
    /// An hypha publisher feeds a ROS subscriber.
    HyphaToRos,
}

/// The ROS graph state for one topic, excluding the bridge's own endpoints.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RosTopicState {
    /// Names of the external ROS nodes that publish the topic.
    pub publishers: BTreeSet<String>,
    /// At least one external ROS subscriber exists.
    pub subscribed: bool,
    /// The ROS type reported by the graph, when known.
    pub type_name: String,
}

impl RosTopicState {
    /// At least one external ROS publisher exists.
    #[must_use]
    pub fn published(&self) -> bool {
        !self.publishers.is_empty()
    }
}

/// The hypha graph state for one topic, excluding bridge-owned ports.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HyphaTopicState {
    /// At least one external hypha publisher exists.
    pub published: bool,
    /// At least one external hypha subscriber exists.
    pub subscribed: bool,
}

/// The hypha graph state of each topic name, from the state of each of its streams: a name is
/// published, or subscribed, when any of its streams is. A stream's flags are its own: a
/// stream nobody subscribes to does not hide the subscriber of another.
#[must_use]
pub fn topic_states(
    streams: &BTreeMap<StreamKey, HyphaTopicState>,
) -> BTreeMap<String, HyphaTopicState> {
    let mut topics: BTreeMap<String, HyphaTopicState> = BTreeMap::new();
    for (stream, state) in streams {
        let topic = topics.entry(stream.topic.clone()).or_default();
        topic.published |= state.published;
        topic.subscribed |= state.subscribed;
    }
    topics
}

/// Returns the cross-side pipes required by two graph snapshots.
#[must_use]
pub fn desired_flows(ros: &RosTopicState, hypha: &HyphaTopicState) -> Vec<AutoFlow> {
    let mut flows = Vec::with_capacity(2);
    if ros.published() && hypha.subscribed {
        flows.push(AutoFlow::RosToHypha);
    }
    if hypha.published && ros.subscribed {
        flows.push(AutoFlow::HyphaToRos);
    }
    flows
}

/// One sample the bridge itself put on the hypha bus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentSample {
    /// Topic the bridge published on.
    pub topic: std::sync::Arc<str>,
    /// Publish timestamp written into the frame.
    pub stamp_nanos: u64,
    /// Frame payload, without the hypha header.
    pub bytes: Vec<u8>,
}

/// Remembers samples the bridge published, so it never sends them back.
///
/// The bridge subscribes by name to topics that it also publishes.
/// Without this filter the two sides amplify each other.
///
/// A received frame reports a topic, a publish timestamp and a payload. It does
/// not repeat the type identity, so the md5 must not take part in the match.
#[derive(Debug)]
pub struct RecentSamples {
    entries: std::collections::VecDeque<RecentSample>,
    limit: usize,
}

impl RecentSamples {
    /// Creates a filter that remembers at most `limit` samples.
    #[must_use]
    pub fn new(limit: usize) -> Self {
        Self { entries: std::collections::VecDeque::new(), limit }
    }

    /// Number of remembered samples.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is remembered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Records one sample the bridge just published.
    pub fn remember(&mut self, sample: RecentSample) {
        if self.limit == 0 {
            return;
        }
        while self.entries.len() >= self.limit {
            let _ = self.entries.pop_front();
        }
        self.entries.push_back(sample);
    }

    /// Removes and reports a match for a received frame.
    ///
    /// Returns `true` when the bridge published this frame itself.
    pub fn take(&mut self, topic: &str, stamp_nanos: u64, bytes: &[u8]) -> bool {
        let Some(index) = self.entries.iter().position(|sample| {
            &*sample.topic == topic && sample.stamp_nanos == stamp_nanos && sample.bytes == bytes
        }) else {
            return false;
        };
        let _ = self.entries.remove(index);
        true
    }
}
