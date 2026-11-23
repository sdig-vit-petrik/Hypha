//! Bus-facing adapter for `hypha record`: manages wildcard subscriptions and schema resolution.
//!
//! Each message is written with the frame's framework send stamp as its MCAP log and
//! publish time.

use crate::recorder::{RecordSample, Recorder};
use hypha_node::sdk::SchemaIdentityCache;
use hypha_node::sdk::{BusNode, ResolvedSchema, SampleIdentity};
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
    /// Topics already reported as dropping samples, so a lenient session warns once each.
    dropping_topics: BTreeSet<String>,
    /// The type identity the first recorded frame gave each topic: `(data_type, md5)`.
    recorded_types: BTreeMap<String, (String, String)>,
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
            dropping_topics: BTreeSet::new(),
            recorded_types: BTreeMap::new(),
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
        if let Err(cause) = self.record(sample, bytes) {
            self.drop_sample(sample.topic, &cause);
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
        let identity = self.resolve_identity(sample)?;
        // A channel holds one type: the first recorded frame fixes it for the topic.
        if let Some((recorded_type, recorded_md5)) = self.recorded_types.get(sample.topic) {
            let other_type = !recorded_md5.eq_ignore_ascii_case(&identity.md5);
            if other_type {
                return Err(format!(
                    "its type {} differs from {recorded_type}, the type recorded for the topic",
                    identity.data_type
                ));
            }
        }
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
            self.recorded_types
                .entry(sample.topic.to_owned())
                .or_insert_with(|| (identity.data_type.clone(), identity.md5.clone()));
            Ok(())
        } else {
            Err("the output could not take the message: disk full, unwritable, or already finished"
                .to_string())
        }
    }

    /// Handles a sample that was not recorded. A strict session keeps its first drop as the
    /// failure. A lenient session warns once per topic.
    fn drop_sample(&mut self, topic: &str, cause: &str) {
        if self.strict {
            if self.failure.is_none() {
                self.failure = Some(format!("cannot record '{topic}': {cause}"));
            }
            return;
        }
        if self.dropping_topics.insert(topic.to_owned()) {
            eprintln!(
                "hypha record: cannot record '{topic}': {cause}; its samples that cannot be recorded are dropped (reported once per topic)"
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
