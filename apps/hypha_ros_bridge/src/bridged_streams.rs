//! One hypha stream is bridged for each ROS topic (topic name overloading, #209).
//!
//! ROS 1 carries one message type, one MD5, on a topic, while a hypha topic name may carry
//! several streams, two versions of a message during a migration. The first stream that
//! delivers a sample is the one bridged; a sample of another stream of the name is dropped, and
//! the bridge says so once for that stream, naming both. The choice ends with the stream: when
//! no publisher of it is left, the next sample of another stream is the first.

use std::collections::{BTreeMap, BTreeSet};

use hypha_node::sdk::StreamKey;

use crate::automatic::HyphaTopicState;

/// What the bridge does with a sample of one stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Admission {
    /// The stream is the one bridged for its topic.
    Bridged,
    /// Another stream of the topic is bridged: the sample is dropped. `report` is true the first
    /// time the bridge drops a sample of this stream, so its log line comes once.
    Dropped {
        /// The schema hash of the stream that is bridged.
        bridged: u64,
        /// Whether this is the first sample of the stream dropped since it appeared.
        report: bool,
    },
}

/// The stream bridged for each topic, and the streams already reported as not bridged.
#[derive(Debug, Default)]
pub(crate) struct BridgedStreams {
    chosen: BTreeMap<String, u64>,
    reported: BTreeMap<String, BTreeSet<u64>>,
}

impl BridgedStreams {
    /// Whether a sample of the stream of `schema_hash` on `topic` is bridged. The first stream
    /// seen on a topic is chosen for it.
    pub(crate) fn admit(&mut self, topic: &str, schema_hash: u64) -> Admission {
        let bridged = if let Some(bridged) = self.chosen.get(topic) {
            *bridged
        } else {
            self.chosen.insert(topic.to_owned(), schema_hash);
            schema_hash
        };
        if bridged == schema_hash {
            return Admission::Bridged;
        }
        let report = if let Some(hashes) = self.reported.get_mut(topic) {
            hashes.insert(schema_hash)
        } else {
            self.reported.insert(topic.to_owned(), BTreeSet::from([schema_hash]));
            true
        };
        Admission::Dropped { bridged, report }
    }

    /// Ends the choice of each stream that no publisher is left on, and the report of each
    /// stream that is gone, so a stream that comes back is the first or is reported again.
    pub(crate) fn retain_published(&mut self, streams: &BTreeMap<StreamKey, HyphaTopicState>) {
        let published = |topic: &str, schema_hash: u64| {
            let key = StreamKey { topic: topic.to_owned(), schema_hash };
            streams.get(&key).is_some_and(|state| state.published)
        };
        self.chosen.retain(|topic, schema_hash| published(topic, *schema_hash));
        self.reported.retain(|topic, hashes| {
            hashes.retain(|schema_hash| published(topic, *schema_hash));
            !hashes.is_empty()
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{Admission, BridgedStreams};
    use crate::automatic::HyphaTopicState;
    use hypha_node::sdk::StreamKey;
    use std::collections::BTreeMap;

    fn live(streams: &[(&str, u64)]) -> BTreeMap<StreamKey, HyphaTopicState> {
        streams
            .iter()
            .map(|(topic, schema_hash)| {
                (
                    StreamKey { topic: (*topic).to_owned(), schema_hash: *schema_hash },
                    HyphaTopicState { published: true, subscribed: false },
                )
            })
            .collect()
    }

    #[test]
    fn the_first_stream_of_a_topic_is_bridged_and_the_others_are_dropped_and_reported_once() {
        let mut bridged = BridgedStreams::default();
        assert_eq!(bridged.admit("/odom", 1), Admission::Bridged);
        assert_eq!(bridged.admit("/odom", 2), Admission::Dropped { bridged: 1, report: true });
        assert_eq!(bridged.admit("/odom", 2), Admission::Dropped { bridged: 1, report: false });
        assert_eq!(bridged.admit("/odom", 3), Admission::Dropped { bridged: 1, report: true });
        assert_eq!(bridged.admit("/odom", 1), Admission::Bridged, "the bridged stream goes on");
        assert_eq!(bridged.admit("/scan", 2), Admission::Bridged, "another topic has its own");
    }

    #[test]
    fn the_choice_ends_with_its_stream_and_a_stream_that_comes_back_is_reported_again() {
        let mut bridged = BridgedStreams::default();
        assert_eq!(bridged.admit("/odom", 1), Admission::Bridged);
        assert_eq!(bridged.admit("/odom", 2), Admission::Dropped { bridged: 1, report: true });

        // The first version's publishers left: the second is the first sample seen next.
        bridged.retain_published(&live(&[("/odom", 2)]));
        assert_eq!(bridged.admit("/odom", 2), Admission::Bridged);
        assert_eq!(bridged.admit("/odom", 1), Admission::Dropped { bridged: 2, report: true });

        // Both stay published: nothing changes, and nothing is reported twice.
        bridged.retain_published(&live(&[("/odom", 1), ("/odom", 2)]));
        assert_eq!(bridged.admit("/odom", 1), Admission::Dropped { bridged: 2, report: false });
    }
}
