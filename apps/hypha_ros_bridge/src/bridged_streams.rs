//! One hypha stream is bridged for each ROS topic (topic name overloading, #209).
//!
//! ROS 1 carries one message type, one MD5, on a topic, while a hypha topic name may carry
//! several streams, two versions of a message during a migration. The first stream that
//! delivers a sample is the one bridged; a sample of another stream of the name is dropped, and
//! the bridge says so once for that stream, naming both. The choice ends with the stream: when
//! no publisher of it is left, the next sample of another stream is the first.

use std::collections::{BTreeMap, BTreeSet};

use hypha_node::sdk::{ResolvedSchema, StreamKey};

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

/// What to say, once, when a topic already advertised to ROS as `first` is offered `other`: a
/// ROS 1 topic keeps its type under its subscribers, so the first type stays until the topic
/// is unadvertised and `other` is not bridged. `None` when this pair was said before.
pub(crate) fn not_bridged_warning(
    topic: &str,
    first: &ResolvedSchema,
    other: &ResolvedSchema,
    said: &mut BTreeSet<(String, String)>,
) -> Option<String> {
    if !said.insert((topic.to_owned(), other.md5.clone())) {
        return None;
    }
    let label = |schema: &ResolvedSchema| {
        format!("{} ({})", schema.data_type, schema.md5.get(..8).unwrap_or(&schema.md5))
    };
    Some(format!(
        "{topic} is bridged to ROS 1 as {}, and ROS 1 keeps a topic's type under its subscribers: \
         {} is not bridged",
        label(first),
        label(other)
    ))
}

#[cfg(test)]
mod tests {
    use super::{Admission, BridgedStreams, not_bridged_warning};
    use crate::automatic::HyphaTopicState;
    use hypha_node::sdk::ResolvedSchema;
    use hypha_node::sdk::StreamKey;
    use std::collections::BTreeMap;
    use std::collections::BTreeSet;

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

    fn schema(data_type: &str, md5: &str) -> ResolvedSchema {
        ResolvedSchema {
            data_type: data_type.to_owned(),
            md5: md5.to_owned(),
            definition: String::new(),
        }
    }

    #[test]
    fn a_second_type_on_an_advertised_ros_topic_is_warned_of_once_with_both_types() {
        let first = schema("t/R", "b3087778e93fcd34cc8d65bc54e850d1");
        let other = schema("t/R", "f79835def461350458ead76f07016dd7");
        let mut said = BTreeSet::new();
        let warning = not_bridged_warning("/odom", &first, &other, &mut said).unwrap_or_default();
        assert!(warning.contains("t/R (b3087778)") && warning.contains("t/R (f79835de)"));
        assert!(warning.contains("not bridged"), "{warning}");
        assert_eq!(not_bridged_warning("/odom", &first, &other, &mut said), None, "once");
        let third = schema("t/R", "00000000000000000000000000000000");
        assert!(not_bridged_warning("/odom", &first, &third, &mut said).is_some());
    }
}
