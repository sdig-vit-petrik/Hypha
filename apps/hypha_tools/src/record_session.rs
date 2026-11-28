//! Bus-facing adapter for `hypha record`: manages wildcard subscriptions and schema resolution.
//!
//! Each message is written with the frame's framework send stamp as its MCAP log and
//! publish time.

use crate::recorder::{RecordSample, Recorder};
use crate::stream::{stream_label, stream_title};
use hypha_node::sdk::SchemaIdentityCache;
use hypha_node::sdk::{BusNode, ResolvedSchema, SampleIdentity, StreamKey};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long a strict session waits for a topic's retained schema record.
const SCHEMA_WAIT: Duration = Duration::from_secs(5);

/// Active recording session: resolves each sample's schema and writes it to its `Recorder`.
pub struct RecorderSession<'bus> {
    bus: &'bus BusNode,
    recorder: Recorder,
    /// A frame carries only a schema hash, so the complete type identity comes
    /// from the retained schema service.
    identities: SchemaIdentityCache,
    strict: bool,
    /// The first drop of a strict session, which stops the recording.
    failure: Option<String>,
    /// Streams already reported as dropping samples, so a lenient session warns once each.
    dropping: BTreeSet<StreamKey>,
    /// The schema hashes seen on each topic: a name of several streams is told apart from a
    /// name of one when a drop is reported.
    streams: BTreeMap<String, BTreeSet<u64>>,
}

impl<'bus> RecorderSession<'bus> {
    /// Starts a recording session that writes to `recorder`.
    #[must_use]
    pub fn new(bus: &'bus BusNode, recorder: Recorder, strict: bool) -> Self {
        Self {
            bus,
            recorder,
            identities: SchemaIdentityCache::new(),
            strict,
            failure: None,
            dropping: BTreeSet::new(),
            streams: BTreeMap::new(),
        }
    }

    /// The recorder this session writes to.
    #[must_use]
    pub fn recorder(&self) -> &Recorder {
        &self.recorder
    }

    /// Finalizes the recording and returns its recorder.
    #[must_use]
    pub fn finish(mut self) -> Recorder {
        if let Err(error) = self.recorder.finish() {
            eprintln!("hypha record: {error}");
        }
        self.recorder
    }

    /// Records one received bus sample, written straight from its payload.
    pub fn on_sample(&mut self, sample: &SampleIdentity<'_>, bytes: &[u8]) {
        // A strict session that has failed writes nothing more: the recording is over.
        if self.strict && self.failure.is_some() {
            return;
        }
        self.note_stream(sample);
        if let Err(cause) = self.record(sample, bytes) {
            self.drop_sample(sample, &cause);
        }
    }

    /// The failure of a strict session, once a sample could not be recorded.
    #[must_use]
    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    /// Writes one sample, or says why it cannot be written.
    fn record(&mut self, sample: &SampleIdentity<'_>, bytes: &[u8]) -> Result<(), String> {
        let stamp = sample.framework_sent_nanos;
        if stamp == 0 {
            return Err("the frame carries no framework send time".to_string());
        }
        // A channel holds one stream: a frame of another schema is on a channel of its own.
        let identity = self.resolve_identity(sample)?;
        let record_sample = RecordSample {
            topic: sample.topic,
            data_type: &identity.data_type,
            md5: &identity.md5,
            definition: &identity.definition,
            bytes,
            log_time_nanos: stamp,
            publish_time_nanos: stamp,
        };
        if self.recorder.record(&record_sample) {
            Ok(())
        } else {
            Err("the output could not take the message: disk full, unwritable, or already finished"
                .to_string())
        }
    }

    /// Remembers that a frame of `sample`'s schema came on its topic.
    fn note_stream(&mut self, sample: &SampleIdentity<'_>) {
        match self.streams.get_mut(sample.topic) {
            Some(hashes) => {
                hashes.insert(sample.schema_hash);
            }
            None => {
                self.streams.insert(sample.topic.to_owned(), BTreeSet::from([sample.schema_hash]));
            }
        }
    }

    /// The stream of `sample` as a drop names it: its topic, and its type when the topic
    /// has several streams.
    fn describe(&self, sample: &SampleIdentity<'_>) -> String {
        let shared = self.streams.get(sample.topic).is_some_and(|hashes| hashes.len() > 1);
        let identity = self.identities.cached(sample.topic, sample.schema_hash);
        let label = stream_label(identity.as_deref(), sample.schema_hash);
        stream_title(&format!("'{}'", sample.topic), &label, shared)
    }

    /// Handles a sample that was not recorded. A strict session keeps its first drop as the
    /// failure. A lenient session warns once per stream.
    fn drop_sample(&mut self, sample: &SampleIdentity<'_>, cause: &str) {
        if self.strict {
            if self.failure.is_none() {
                self.failure = Some(format!("cannot record {}: {cause}", self.describe(sample)));
            }
            return;
        }
        if self.dropping.insert(StreamKey::of(sample)) {
            eprintln!(
                "hypha record: cannot record {}: {cause}; its samples that cannot be recorded are dropped (reported once per stream)",
                self.describe(sample)
            );
        }
    }

    /// Resolves the complete type identity behind a frame's schema hash.
    fn resolve_identity(&self, sample: &SampleIdentity<'_>) -> Result<Arc<ResolvedSchema>, String> {
        if sample.schema_hash == 0 {
            return Err("the frame carries no schema hash".to_string());
        }
        let resolved = if self.strict {
            let deadline = Instant::now().checked_add(SCHEMA_WAIT).unwrap_or_else(Instant::now);
            self.identities.resolve_until(self.bus, sample.topic, sample.schema_hash, deadline)
        } else {
            self.identities.resolve(self.bus, sample.topic, sample.schema_hash, Instant::now())
        };
        match resolved {
            Ok(Some(identity)) => Ok(identity),
            Ok(None) if self.strict => {
                Err(format!("no retained schema record arrived within {}s", SCHEMA_WAIT.as_secs()))
            }
            Ok(None) => Err("its retained schema record is not available yet".to_string()),
            Err(cause) => Err(format!("schema hash lookup failed: {cause}")),
        }
    }
}
