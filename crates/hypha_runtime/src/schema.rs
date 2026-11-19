//! Backend-independent schema lookup, validation, and bounded identity caches.

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use hypha_core::port::{PortError, SchemaSelector, TopicSpec};
use hypha_wire::schema_record::parse_schema_record;

/// Resolved identities kept per process; an evicted identity is read again from its file.
const MAX_RESOLVED_IDENTITIES: usize = 64;

/// Safe interface implemented by the FFI-selected transport adapter.
pub trait SchemaTransport {
    /// Query a topic and wait at most `timeout` for its matching schema record.
    ///
    /// # Errors
    /// Returns the transport's [`PortError`] when the request cannot complete.
    fn lookup_schema(
        &self,
        topic: &str,
        selector: SchemaSelector,
        timeout: Duration,
    ) -> Result<Option<Vec<u8>>, PortError>;
}

/// Failure of schema lookup or validation policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaFailure {
    /// No publisher currently retains the requested schema.
    NotAvailable,
    /// The response did not contain the requested valid schema identity.
    InvalidSchema,
    /// A bounded transport resource was exhausted.
    ResourceLimit,
    /// A mutex or backend operation failed.
    Backend,
    /// The transport refused a hash lookup for this reason.
    Transport(PortError),
}

/// A validated schema identity resolved from a compact frame hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedIdentity {
    /// ROS message type name.
    pub data_type: String,
    /// ROS message MD5.
    pub md5: String,
    /// Canonical ROS message definition.
    pub definition: String,
}

/// Safe counters for schema lookup policy.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SchemaHealth {
    /// Schema records rejected by parsing, validation, or identity checks.
    pub invalid_records: u64,
    /// Queries that found no currently retained schema.
    pub unavailable_lookups: u64,
    /// Operations refused due to a fixed resource bound.
    pub resource_limit_failures: u64,
}

/// Topic, ROS message type and MD5.
type SchemaKey = (String, String, String);
type IdentityKey = (String, u64);

/// A map that keeps at most `MAX_RESOLVED_IDENTITIES` entries and evicts the
/// oldest inserted key first.
// The bound counts entries, not bytes.
struct BoundedCache<K, V> {
    entries: HashMap<K, V>,
    order: VecDeque<K>,
}

impl<K, V> Default for BoundedCache<K, V> {
    fn default() -> Self {
        Self { entries: HashMap::new(), order: VecDeque::new() }
    }
}

impl<K: Eq + Hash + Clone, V: Clone> BoundedCache<K, V> {
    fn get(&self, key: &K) -> Option<V> {
        self.entries.get(key).cloned()
    }

    fn insert(&mut self, key: K, value: V) {
        if self.entries.insert(key.clone(), value).is_some() {
            return;
        }
        self.order.push_back(key);
        while self.order.len() > MAX_RESOLVED_IDENTITIES {
            let Some(oldest) = self.order.pop_front() else {
                return;
            };
            self.entries.remove(&oldest);
        }
    }
}

/// Lock `mutex`; a poisoned cache or counter still holds valid data, so it is used as is.
fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn is_resource_limit(cause: &PortError) -> bool {
    matches!(cause, PortError::PoolExhausted | PortError::BorrowLimit | PortError::ResourceLimit)
}

/// Synchronized schema cache and lookup health for one node session.
#[derive(Default)]
pub struct SchemaState {
    definitions: Mutex<BoundedCache<SchemaKey, String>>,
    identities: Mutex<BoundedCache<IdentityKey, ResolvedIdentity>>,
    health: Mutex<SchemaHealth>,
}

impl SchemaState {
    /// Read a cached schema definition without calling the transport.
    pub fn cached_definition(&self, topic: &str, data_type: &str, md5: &str) -> Option<String> {
        let key = (topic.to_owned(), data_type.to_owned(), md5.to_owned());
        locked(&self.definitions).get(&key)
    }

    /// Read a cached identity without calling the transport.
    pub fn cached_identity(&self, topic: &str, schema_hash: u64) -> Option<ResolvedIdentity> {
        let key = (topic.to_owned(), schema_hash);
        locked(&self.identities).get(&key)
    }

    /// Resolve and validate a schema definition, caching only a matching record.
    ///
    /// # Errors
    /// Returns a [`SchemaFailure`] when the schema is unavailable, invalid,
    /// or the transport hit a resource limit or failed.
    pub fn lookup<T: SchemaTransport>(
        &self,
        transport: &T,
        spec: &TopicSpec,
        timeout: Duration,
    ) -> Result<String, SchemaFailure> {
        let key = (spec.name.clone(), spec.data_type.clone(), spec.md5.clone());
        if let Some(definition) = self.cached_definition(&key.0, &key.1, &key.2) {
            return Ok(definition);
        }
        let response = match transport.lookup_schema(
            &spec.name,
            SchemaSelector::Md5(spec.md5.clone()),
            timeout,
        ) {
            Ok(Some(response)) => response,
            Ok(None) => {
                self.record(|health| &mut health.unavailable_lookups);
                return Err(SchemaFailure::NotAvailable);
            }
            Err(cause) if is_resource_limit(&cause) => {
                self.record(|health| &mut health.resource_limit_failures);
                return Err(SchemaFailure::ResourceLimit);
            }
            Err(_) => return Err(SchemaFailure::Backend),
        };
        let Ok(parsed) = parse_schema_record(&response) else {
            self.record(|health| &mut health.invalid_records);
            return Err(SchemaFailure::InvalidSchema);
        };
        if parsed.md5() != spec.md5 || parsed.data_type() != spec.data_type {
            self.record(|health| &mut health.invalid_records);
            return Err(SchemaFailure::InvalidSchema);
        }
        let definition = parsed.definition().to_owned();
        locked(&self.definitions).insert(key, definition.clone());
        Ok(definition)
    }

    /// Resolve a compact schema hash, using the bounded identity cache first.
    ///
    /// # Errors
    /// Returns [`SchemaFailure::Transport`] with the transport's cause, or
    /// [`SchemaFailure::InvalidSchema`] for an unparseable record.
    pub fn resolve_hash<T: SchemaTransport>(
        &self,
        transport: &T,
        topic: &str,
        schema_hash: u64,
        timeout: Duration,
    ) -> Result<Option<ResolvedIdentity>, SchemaFailure> {
        if let Some(identity) = self.cached_identity(topic, schema_hash) {
            return Ok(Some(identity));
        }
        let bytes = match transport.lookup_schema(topic, SchemaSelector::Hash(schema_hash), timeout)
        {
            Ok(Some(bytes)) => bytes,
            Ok(None) => {
                self.record(|health| &mut health.unavailable_lookups);
                return Ok(None);
            }
            Err(cause) => {
                if is_resource_limit(&cause) {
                    self.record(|health| &mut health.resource_limit_failures);
                }
                return Err(SchemaFailure::Transport(cause));
            }
        };
        let Ok(schema) = parse_schema_record(&bytes) else {
            self.record(|health| &mut health.invalid_records);
            return Err(SchemaFailure::InvalidSchema);
        };
        let identity = ResolvedIdentity {
            data_type: schema.data_type().to_owned(),
            md5: schema.md5().to_owned(),
            definition: schema.definition().to_owned(),
        };
        locked(&self.identities).insert((topic.to_owned(), schema_hash), identity.clone());
        Ok(Some(identity))
    }

    /// Copy schema resolution counters for the C ABI to combine with backend health.
    pub fn health(&self) -> SchemaHealth {
        *locked(&self.health)
    }

    /// Count one event in the health counter that `counter` selects.
    fn record(&self, counter: fn(&mut SchemaHealth) -> &mut u64) {
        let mut health = locked(&self.health);
        let slot = counter(&mut health);
        *slot = slot.wrapping_add(1);
    }
}
