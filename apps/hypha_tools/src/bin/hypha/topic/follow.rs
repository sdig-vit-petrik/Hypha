//! The streams one `hypha topic` verb reads: every stream of a topic name that a
//! `--type`/`--md5` selector lets through, including one that joins the name after the verb
//! started, and the title that tells them apart when a name has several.

use hypha_node::sdk::{
    BusNode, ReceivedSample, ResolvedSchema, StreamFollower, StreamKey, StreamSelector,
};
use hypha_tools::following::Following;
use hypha_tools::stream::{StreamFlags, follow_error_text, follower_title, stream_label};
use std::collections::BTreeMap;
use std::process::ExitCode;
use std::sync::Arc;

/// The streams of one topic name, followed for the verb `verb`.
pub(super) struct Followed<'bus> {
    bus: &'bus BusNode,
    verb: &'static str,
    follower: Following,
    /// What a refusal or a failed listing last said, so one that lasts is told once.
    told: Option<String>,
    /// The streams already told to have sent samples before they were followed.
    sent_before: BTreeMap<StreamKey, u64>,
}

impl<'bus> Followed<'bus> {
    /// Follows the streams of `topic` that `flags` let through, at the depth of the topic's
    /// service policy. When there is none it says why, once, and the verb goes on: the first
    /// stream that joins is followed from its first sample.
    ///
    /// # Errors
    /// The exit code, after its message, when the bus cannot be followed.
    pub(super) fn open(
        bus: &'bus Arc<BusNode>,
        topic: &str,
        flags: &StreamFlags,
        verb: &'static str,
    ) -> Result<Self, ExitCode> {
        let selector = StreamSelector::from(flags);
        let wanted = StreamFollower::new(vec![topic.to_owned()], selector.clone(), 0);
        let streams = Following::start(bus, wanted).map_err(|error| {
            eprintln!("hypha topic {verb}: {error}");
            ExitCode::FAILURE
        })?;
        let this = Self { bus, verb, follower: streams, told: None, sent_before: BTreeMap::new() };
        if this.follower.followed() == 0 {
            this.say_why_none(topic, &selector);
        }
        Ok(this)
    }

    /// Says that nothing is followed yet: nothing is published, or the flags let the streams
    /// of the name go, and then which streams it has.
    fn say_why_none(&self, topic: &str, selector: &StreamSelector) {
        let skipped = self.follower.skipped();
        if skipped.is_empty() {
            let which = if selector.is_open() {
                String::new()
            } else {
                format!(" (a stream with {selector})")
            };
            eprintln!("hypha topic {}: waiting for '{topic}' to be published{which}", self.verb);
            return;
        }
        let each: Vec<String> =
            skipped.iter().map(|(stream, schema)| describe(stream, *schema)).collect();
        eprintln!(
            "hypha topic {}: '{topic}' has no stream with {selector}; its streams are {}; waiting",
            self.verb,
            each.join(", ")
        );
    }

    /// Follows the streams that joined since the last call, and tells what a stream sent
    /// before it was followed.
    pub(super) fn refresh(&mut self) {
        if let Err(error) = self.follower.pass(self.bus) {
            let text = follow_error_text(self.bus, &error);
            if self.told.as_ref() != Some(&text) {
                eprintln!("hypha topic {}: {text}", self.verb);
                self.told = Some(text);
            }
        }
        for (stream, count) in self.follower.sent_before_following() {
            if self.sent_before.insert(stream.clone(), count).is_none() {
                let title = follower_title(&self.follower, self.bus, &stream);
                eprintln!(
                    "hypha topic {}: {title}: {count} samples were sent before it was followed",
                    self.verb
                );
            }
        }
    }

    /// The next sample of any followed stream; a receive error is printed and counts as no
    /// sample.
    pub(super) fn next(&mut self) -> Option<ReceivedSample> {
        self.follower.next_sample().unwrap_or_else(|error| {
            eprintln!("hypha topic {}: {error}", self.verb);
            None
        })
    }

    /// Whether the topic has more than one followed stream with a publisher, so that each
    /// sample or report must say which stream it is of.
    pub(super) fn is_shared(&self, stream: &StreamKey) -> bool {
        self.follower.shares_its_name(stream)
    }

    /// `stream` under its topic, with its type and MD5 start when the topic is shared.
    pub(super) fn title(&self, stream: &StreamKey) -> String {
        follower_title(&self.follower, self.bus, stream)
    }

    /// The streams followed, each by type and MD5 start, for a refusal that must list them.
    pub(super) fn streams(&self) -> Vec<String> {
        self.follower
            .publisher_counts()
            .keys()
            .map(|stream| {
                let schema = self
                    .bus
                    .resolve_schema_hash_now(&stream.topic, stream.schema_hash)
                    .ok()
                    .flatten();
                describe(stream, schema.as_ref())
            })
            .collect()
    }
}

/// A stream by its type and MD5 start, or by its schema hash when it has no definition.
fn describe(stream: &StreamKey, schema: Option<&ResolvedSchema>) -> String {
    stream_label(schema, stream.schema_hash)
}
