//! The streams one `hypha topic` verb reads: every stream of a topic name that a
//! `--type`/`--md5` selector lets through, including one that joins the name after the verb
//! started, and the title that tells them apart when a name has several.

use hypha_node::sdk::{BusNode, ReceivedSample};
use hypha_tools::stream::{StreamKey, StreamSelector};
use hypha_tools::stream_follower::StreamFollower;
use std::process::ExitCode;

/// The streams of one topic name, followed for the verb `verb`.
pub(super) struct Followed<'bus> {
    bus: &'bus BusNode,
    verb: &'static str,
    follower: StreamFollower,
}

impl<'bus> Followed<'bus> {
    /// Follows the streams of `topic` that `selector` lets through, at the depth of the
    /// topic's service policy. Says once that it waits when there is none yet; the verb then
    /// goes on and the first stream that joins is followed from its first sample.
    ///
    /// # Errors
    /// The exit code, after its message, when the bus cannot be followed.
    pub(super) fn open(
        bus: &'bus BusNode,
        topic: &str,
        selector: &StreamSelector,
        verb: &'static str,
    ) -> Result<Self, ExitCode> {
        let mut follower = StreamFollower::new(bus, vec![topic.to_owned()], selector.clone(), 0)
            .map_err(|error| refused(verb, &error))?;
        follower.refresh(bus).map_err(|error| refused(verb, &error))?;
        if follower.followed() == 0 {
            let which = if selector.is_open() {
                String::new()
            } else {
                format!(" (a stream with {selector})")
            };
            eprintln!("hypha topic {verb}: waiting for '{topic}' to be published{which}");
        }
        Ok(Self { bus, verb, follower })
    }

    /// Follows the streams that joined since the last call.
    pub(super) fn refresh(&mut self) {
        if let Err(error) = self.follower.refresh_if_changed(self.bus) {
            eprintln!("hypha topic {}: {error}", self.verb);
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

    /// Whether the topic has more than one followed stream, so that each sample or report
    /// must say which stream it is of.
    pub(super) fn is_shared(&self, stream: &StreamKey) -> bool {
        self.follower.shares_its_name(stream)
    }

    /// `stream` under its topic, with its type and MD5 start when the topic is shared.
    pub(super) fn title(&self, stream: &StreamKey) -> String {
        self.follower.title(self.bus, stream)
    }
}

/// The failure exit code, after the message of the verb.
fn refused(verb: &str, error: &str) -> ExitCode {
    eprintln!("hypha topic {verb}: {error}");
    ExitCode::FAILURE
}
