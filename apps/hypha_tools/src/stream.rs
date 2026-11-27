//! A stream as the tools label it (topic name overloading, #209): by type and the start of
//! its MD5, and the flags that choose among the streams of a name.

use clap::Args;
use hypha_node::sdk::{BusNode, ResolvedSchema, StreamFollower, StreamKey, StreamSelector};

/// The flags that choose among the streams of a topic name: all of them, or those of one
/// message type and/or one MD5. The same on `hypha topic echo`, `hz` and `bw`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
pub struct StreamFlags {
    /// Follow only the streams of this message type.
    #[arg(long = "type", value_name = "type")]
    pub data_type: Option<String>,
    /// Follow only the streams whose MD5 starts with these hex digits.
    #[arg(long, value_name = "prefix")]
    pub md5: Option<String>,
}

impl From<&StreamFlags> for StreamSelector {
    fn from(flags: &StreamFlags) -> Self {
        Self { data_type: flags.data_type.clone(), md5_prefix: flags.md5.clone() }
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
    if md5.is_empty() {
        return data_type.to_owned();
    }
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

/// `stream` under its topic name, with its type and MD5 start when `follower` follows
/// other streams of the name. A schema that cannot be read now is shown as its hash.
#[must_use]
pub fn follower_title(follower: &StreamFollower, bus: &BusNode, stream: &StreamKey) -> String {
    let shared = follower.shares_its_name(stream);
    if !shared {
        return stream.topic.clone();
    }
    let schema = bus.resolve_schema_hash_now(&stream.topic, stream.schema_hash).ok().flatten();
    stream_title(&stream.topic, &stream_label(schema.as_ref(), stream.schema_hash), shared)
}

#[cfg(test)]
mod tests {
    use super::{short_md5, stream_label, type_label};
    use hypha_node::sdk::ResolvedSchema;

    const V1: &str = "b3087778e93fcd34cc8d65bc54e850d1";

    #[test]
    fn a_label_has_the_type_and_the_start_of_the_md5() {
        let schema = ResolvedSchema {
            data_type: "a/B".to_owned(),
            md5: V1.to_owned(),
            definition: String::new(),
        };
        assert_eq!(stream_label(Some(&schema), 7), "a/B (b3087778)");
        assert_eq!(stream_label(None, 7), "? (0000000000000007)");
        assert_eq!(short_md5("abc"), "abc");
        assert_eq!(type_label("a/B", ""), "a/B", "a type of no MD5 is its name");
    }
}
