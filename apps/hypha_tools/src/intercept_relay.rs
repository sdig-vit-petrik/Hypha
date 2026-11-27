//! What one interception does with each frame, without the bus.

use std::collections::BTreeMap;

use hypha_node::sdk::ResolvedSchema;

use crate::intercept_doc::{InterceptionSpec, InterceptionState, RelayState};
use crate::msg_patch::{Override, apply};
use crate::msg_reflect::MessageSchema;

/// What becomes of one frame.
#[derive(Debug, PartialEq, Eq)]
pub enum Forward<'a> {
    /// Publish these bytes on the output.
    Publish(&'a [u8]),
    /// Publish nothing.
    Drop,
}

/// The relay of one interception.
pub struct Relay {
    armed: bool,
    overrides: BTreeMap<String, Override>,
    /// The hash of the newest frame seen while `Resolving`.
    pending: Option<u64>,
    /// The schema the relay is live on.
    live: Option<(u64, MessageSchema, ResolvedSchema)>,
    counters: InterceptionState,
}

fn convert(spec: &InterceptionSpec) -> Result<BTreeMap<String, Override>, String> {
    spec.overrides
        .iter()
        .map(|(path, entry)| {
            entry
                .to_override()
                .map(|value| (path.clone(), value))
                .map_err(|why| format!("interception '{}', override '{path}': {why}", spec.name))
        })
        .collect()
}

fn bump(counter: &mut u64) {
    *counter = counter.saturating_add(1);
}

impl Relay {
    /// A relay for `spec`, waiting for its schema.
    ///
    /// # Errors
    /// Returns the reason when an override in `spec` is not valid.
    pub fn new(spec: &InterceptionSpec) -> Result<Self, String> {
        Ok(Self {
            armed: spec.armed,
            overrides: convert(spec)?,
            pending: None,
            live: None,
            counters: InterceptionState { name: spec.name.clone(), ..InterceptionState::default() },
        })
    }

    /// Takes the arm switch and the overrides of `spec`; keeps the counters and the
    /// schema. Only for a spec with the same input and output.
    ///
    /// # Errors
    /// Returns the reason when an override is not valid; the relay is then unchanged.
    pub fn update(&mut self, spec: &InterceptionSpec) -> Result<(), String> {
        self.overrides = convert(spec)?;
        self.armed = spec.armed;
        Ok(())
    }

    /// The schema hash to resolve, while the relay waits for one.
    #[must_use]
    pub fn pending_schema(&self) -> Option<u64> {
        if self.counters.state == RelayState::Resolving { self.pending } else { None }
    }

    /// True when the relay forwards nothing until it is rebuilt: its input's schema
    /// changed, or it failed.
    #[must_use]
    pub fn stopped(&self) -> bool {
        matches!(self.counters.state, RelayState::SchemaChanged | RelayState::Error)
    }

    /// The schema of `schema_hash` was resolved: the relay goes live on it.
    ///
    /// # Errors
    /// Returns the reason when the definition does not parse; the relay is then in
    /// the error state.
    pub fn resolved(&mut self, schema_hash: u64, schema: &ResolvedSchema) -> Result<(), String> {
        match MessageSchema::from_definition(&schema.data_type, &schema.definition) {
            Ok(parsed) => {
                self.counters.state = RelayState::Live;
                self.counters.data_type.clone_from(&schema.data_type);
                self.live = Some((schema_hash, parsed, schema.clone()));
                Ok(())
            }
            Err(why) => {
                let reason =
                    format!("the definition of {} does not parse: {why}", schema.data_type);
                self.fail(reason.clone());
                Err(reason)
            }
        }
    }

    /// The schema hashes of the input's streams that have a publisher. Each schema of a
    /// topic is its own stream, so a producer that restarts on another definition leaves
    /// the relay's stream: when it has no publisher and another stream has, the input's
    /// schema changed.
    pub fn input_published(&mut self, published: &[u64]) {
        let Some((live_hash, _, _)) = self.live.as_ref() else { return };
        if self.counters.state != RelayState::Live || published.contains(live_hash) {
            return;
        }
        let Some(other) = published.first() else { return };
        self.counters.state = RelayState::SchemaChanged;
        self.counters.last_error = format!("schema changed: {live_hash:016x} -> {other:016x}");
    }

    /// The relay cannot run: it forwards nothing until it is rebuilt.
    pub fn fail(&mut self, reason: String) {
        self.counters.state = RelayState::Error;
        self.counters.last_error = reason;
    }

    /// Records a failure that does not stop the relay, such as a refused publish.
    pub fn note_error(&mut self, reason: String) {
        self.counters.last_error = reason;
    }

    /// Decides what one frame becomes. `scratch` holds a patched frame.
    pub fn on_frame<'a>(
        &mut self,
        schema_hash: u64,
        payload: &'a [u8],
        scratch: &'a mut Vec<u8>,
    ) -> Forward<'a> {
        bump(&mut self.counters.received);
        match self.counters.state {
            RelayState::Resolving => {
                self.pending = Some(schema_hash);
                bump(&mut self.counters.unresolved_dropped);
                return Forward::Drop;
            }
            RelayState::SchemaChanged | RelayState::Error => return Forward::Drop,
            RelayState::Live => {}
        }
        let Some((live_hash, schema, identity)) = self.live.as_mut() else {
            return Forward::Drop;
        };
        if *live_hash != schema_hash {
            self.counters.state = RelayState::SchemaChanged;
            self.counters.last_error =
                format!("schema changed: {live_hash:016x} -> {schema_hash:016x}");
            return Forward::Drop;
        }
        if !self.armed || self.overrides.is_empty() {
            return Forward::Publish(payload);
        }
        match apply(schema, &identity.data_type, payload, &self.overrides, scratch) {
            Err(why) => {
                bump(&mut self.counters.patch_failed);
                self.counters.last_error = why;
                Forward::Publish(payload)
            }
            Ok(report) => {
                if let Some((path, why)) = report.failed.first() {
                    bump(&mut self.counters.patch_failed);
                    self.counters.last_error = format!("{path}: {why}");
                }
                if report.applied > 0 {
                    bump(&mut self.counters.modified);
                }
                Forward::Publish(scratch.as_slice())
            }
        }
    }

    /// One frame was published.
    pub fn count_published(&mut self) {
        bump(&mut self.counters.published);
    }

    /// The state and counters, for the state document.
    #[must_use]
    pub fn snapshot(&self) -> InterceptionState {
        self.counters.clone()
    }
}
