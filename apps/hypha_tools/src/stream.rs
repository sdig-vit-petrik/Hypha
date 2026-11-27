//! A stream as the tools name it: a topic name and the hash of one schema (topic name
//! overloading, #209). Every tool that tells the streams of a name apart uses these.

use clap::Args;
use hypha_node::sdk::{ResolvedSchema, SampleIdentity, TopicInfo};

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

/// Which streams of a topic name a tool follows: all of them, or those of one message type
/// and/or one MD5. The same flags on `hypha topic echo`, `hz` and `bw`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
pub struct StreamSelector {
    /// Follow only the streams of this message type.
    #[arg(long = "type", value_name = "type")]
    pub data_type: Option<String>,
    /// Follow only the streams whose MD5 starts with these hex digits.
    #[arg(long, value_name = "prefix")]
    pub md5: Option<String>,
}

impl StreamSelector {
    /// Whether the selector names nothing, so it needs no schema to decide.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.data_type.is_none() && self.md5.is_none()
    }

    /// Whether a stream of `schema` is selected. A stream whose schema is unknown is
    /// selected by no flag: it cannot be told to match.
    #[must_use]
    pub fn selects(&self, schema: Option<&ResolvedSchema>) -> bool {
        if self.is_open() {
            return true;
        }
        let Some(schema) = schema else { return false };
        let type_matches = self.data_type.as_ref().is_none_or(|wanted| *wanted == schema.data_type);
        let md5_matches = self.md5.as_ref().is_none_or(|prefix| {
            schema.md5.to_ascii_lowercase().starts_with(&prefix.to_ascii_lowercase())
        });
        type_matches && md5_matches
    }
}

impl std::fmt::Display for StreamSelector {
    /// The flags as they were given: `--type <type> --md5 <prefix>`.
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let flags: Vec<String> = [
            self.data_type.as_ref().map(|data_type| format!("--type {data_type}")),
            self.md5.as_ref().map(|prefix| format!("--md5 {prefix}")),
        ]
        .into_iter()
        .flatten()
        .collect();
        write!(out, "{}", flags.join(" "))
    }
}

/// The first eight hex digits of an MD5: enough to tell two versions of one type apart.
#[must_use]
pub fn short_md5(md5: &str) -> &str {
    md5.get(..8).unwrap_or(md5)
}

/// A message type as a line names it: `type (md5 start)`.
#[must_use]
pub fn type_label(data_type: &str, md5: &str) -> String {
    format!("{data_type} ({})", short_md5(md5))
}

/// A stream as a line names it: its type and MD5 start, or `? (schema hash)` when its
/// schema is not known.
#[must_use]
pub fn stream_label(schema: Option<&ResolvedSchema>, schema_hash: u64) -> String {
    schema.map_or_else(
        || format!("? ({schema_hash:016x})"),
        |schema| type_label(&schema.data_type, &schema.md5),
    )
}

/// A stream under its topic name: the name alone, so that a topic of one stream reads as it
/// always did, and `name [label]` when the name has `shared` it with other streams.
#[must_use]
pub fn stream_title(topic: &str, label: &str, shared: bool) -> String {
    if shared { format!("{topic} [{label}]") } else { topic.to_owned() }
}

#[cfg(test)]
mod tests {
    use super::{StreamSelector, short_md5, stream_label};
    use hypha_node::sdk::ResolvedSchema;

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
        let by_type = StreamSelector { data_type: Some("a/B".into()), md5: None };
        let by_md5 = StreamSelector { data_type: None, md5: Some("B308".into()) };
        let both = StreamSelector { data_type: Some("a/C".into()), md5: Some("b308".into()) };
        let this = schema("a/B", V1);
        assert!(by_type.selects(Some(&this)));
        assert!(by_md5.selects(Some(&this)), "a prefix is not case sensitive");
        assert!(!both.selects(Some(&this)), "the type differs");
        assert!(!by_type.selects(Some(&schema("a/C", V1))));
        assert!(!by_md5.selects(Some(&schema("a/B", "f79835def461350458ead76f07016dd7"))));
    }

    #[test]
    fn a_flag_selects_no_stream_whose_schema_is_unknown() {
        let by_md5 = StreamSelector { data_type: None, md5: Some("b308".into()) };
        assert!(!by_md5.selects(None));
    }

    #[test]
    fn a_label_has_the_type_and_the_start_of_the_md5() {
        assert_eq!(stream_label(Some(&schema("a/B", V1)), 7), "a/B (b3087778)");
        assert_eq!(stream_label(None, 7), "? (0000000000000007)");
        assert_eq!(short_md5("abc"), "abc");
    }
}
