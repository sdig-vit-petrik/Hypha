//! Retained topic schema resolution and health.
//!
//! A raw frame carries a compact schema hash and no type identity. [`SchemaIdentityCache`]
//! resolves it against the domain's retained schema store and paces retries, so a miss
//! does not stall the caller's receive loop.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use hypha_runtime::ResolvedIdentity;

use crate::sdk::ffi;
use crate::sdk::raw::BusNode;

/// Complete type identity resolved from a received frame's schema hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSchema {
    /// ROS-compatible message type name.
    pub data_type: String,
    /// 32-character hexadecimal MD5 digest.
    pub md5: String,
    /// Full ROS message definition.
    pub definition: String,
}

/// A schema lookup or health operation failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaError {
    /// An input or output pointer contract was invalid.
    InvalidArgument,
    /// The topic or schema identity was invalid.
    InvalidTopic,
    /// The retained record did not match its requested identity.
    InvalidSchema,
    /// A fixed resolver resource limit was reached.
    ResourceLimit,
    /// The definition exceeded the supported size.
    TooLarge,
    /// The transport endpoint disconnected.
    Disconnected,
    /// The backend failed.
    Backend,
    /// The runtime returned another status.
    Unknown(i32),
}

impl fmt::Display for SchemaError {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidArgument => write!(output, "the schema call arguments are invalid"),
            Self::InvalidTopic => write!(output, "the schema topic or identity is invalid"),
            Self::InvalidSchema => write!(output, "the retained schema record is invalid"),
            Self::ResourceLimit => write!(output, "the schema resolver resource limit was reached"),
            Self::TooLarge => write!(output, "the schema definition is too large"),
            Self::Disconnected => write!(output, "the schema endpoint disconnected"),
            Self::Backend => write!(output, "the schema backend failed"),
            Self::Unknown(status) => write!(output, "the schema call returned status {status}"),
        }
    }
}

impl std::error::Error for SchemaError {}

fn error_for_status(status: i32) -> SchemaError {
    match status {
        ffi::HYPHA_STATUS_INVALID_ARGUMENT => SchemaError::InvalidArgument,
        ffi::HYPHA_STATUS_INVALID_TOPIC => SchemaError::InvalidTopic,
        ffi::HYPHA_STATUS_TOO_LARGE => SchemaError::TooLarge,
        ffi::HYPHA_STATUS_DISCONNECTED => SchemaError::Disconnected,
        ffi::HYPHA_STATUS_BACKEND => SchemaError::Backend,
        ffi::HYPHA_STATUS_INVALID_SCHEMA => SchemaError::InvalidSchema,
        ffi::HYPHA_STATUS_RESOURCE_LIMIT => SchemaError::ResourceLimit,
        other => SchemaError::Unknown(other),
    }
}

/// `None` when no record is available, the [`SchemaError`] of any other refusal.
fn found<T>(result: Result<T, ffi::Error>) -> Result<Option<T>, SchemaError> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(ffi::Error::NotAvailable) => Ok(None),
        Err(error) => Err(error_for_status(ffi::status_code(error))),
    }
}

fn resolved(
    result: Result<ResolvedIdentity, ffi::Error>,
) -> Result<Option<ResolvedSchema>, SchemaError> {
    Ok(found(result)?.map(|identity| ResolvedSchema {
        data_type: identity.data_type,
        md5: identity.md5,
        definition: identity.definition,
    }))
}

impl BusNode {
    /// Resolve the definition for `(topic, data_type, md5)`.
    ///
    /// `Ok(None)` means that no retained record is available.
    ///
    /// # Errors
    /// Returns [`SchemaError`] when the identity, retained record, resource
    /// state, or transport is invalid.
    pub fn lookup_definition(
        &self,
        topic: &str,
        data_type: &str,
        md5: &str,
    ) -> Result<Option<String>, SchemaError> {
        found(ffi::schema_lookup(&self.node, topic, data_type, md5))
    }

    /// [`Self::resolve_schema_hash`] without waiting for a record that appears late.
    ///
    /// # Errors
    /// Returns [`SchemaError`] when the resolver or returned schema is invalid.
    pub fn resolve_schema_hash_now(
        &self,
        topic: &str,
        schema_hash: u64,
    ) -> Result<Option<ResolvedSchema>, SchemaError> {
        resolved(ffi::schema_resolve_hash_now(&self.node, topic, schema_hash))
    }

    /// Resolve a received frame's compact schema hash to its complete identity.
    ///
    /// `Ok(None)` means that no retained schema is currently available.
    ///
    /// # Errors
    /// Returns [`SchemaError`] when the resolver or returned schema is invalid.
    pub fn resolve_schema_hash(
        &self,
        topic: &str,
        schema_hash: u64,
    ) -> Result<Option<ResolvedSchema>, SchemaError> {
        resolved(ffi::schema_resolve_hash(&self.node, topic, schema_hash))
    }
}

/// Delay before a [`SchemaIdentityCache`] asks the bus again for a hash it could not resolve.
pub const SCHEMA_RETRY_INTERVAL: Duration = Duration::from_millis(500);

/// Caches resolved frame identities and paces retries for unresolved ones.
#[derive(Debug, Default)]
pub struct SchemaIdentityCache {
    /// By topic, then by schema hash, so a lookup borrows the topic.
    entries: Mutex<HashMap<String, HashMap<u64, Entry>>>,
}

#[derive(Debug)]
enum Entry {
    Resolved(Arc<ResolvedSchema>),
    /// Not resolved; the bus is not asked again before this instant.
    RetryAfter(Instant),
}

impl SchemaIdentityCache {
    /// Creates an empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns an already-resolved identity without asking the bus.
    #[must_use]
    pub fn cached(&self, topic: &str, schema_hash: u64) -> Option<Arc<ResolvedSchema>> {
        let entries = self.entries.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        match entries.get(topic)?.get(&schema_hash)? {
            Entry::Resolved(identity) => Some(Arc::clone(identity)),
            Entry::RetryAfter(_) => None,
        }
    }

    /// Resolves `schema_hash` for `topic`, using the cache when it can.
    ///
    /// Returns `Ok(None)` when no retained record is available yet.
    ///
    /// # Errors
    /// Returns [`SchemaError`] when the resolver itself failed.
    pub fn resolve(
        &self,
        bus: &BusNode,
        topic: &str,
        schema_hash: u64,
        now: Instant,
    ) -> Result<Option<Arc<ResolvedSchema>>, SchemaError> {
        if schema_hash == 0 {
            return Ok(None);
        }
        {
            let entries = self.entries.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            match entries.get(topic).and_then(|by_hash| by_hash.get(&schema_hash)) {
                Some(Entry::Resolved(identity)) => return Ok(Some(Arc::clone(identity))),
                Some(Entry::RetryAfter(deadline)) if *deadline > now => return Ok(None),
                _ => {}
            }
        }

        let outcome = bus.resolve_schema_hash(topic, schema_hash);
        let (entry, result) = match outcome {
            Ok(Some(identity)) => {
                let identity = Arc::new(identity);
                (Entry::Resolved(Arc::clone(&identity)), Ok(Some(identity)))
            }
            Ok(None) => (Entry::RetryAfter(retry_deadline(now)), Ok(None)),
            Err(cause) => (Entry::RetryAfter(retry_deadline(now)), Err(cause)),
        };
        let mut entries = self.entries.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        entries.entry(topic.to_owned()).or_default().insert(schema_hash, entry);
        result
    }

    /// Resolves `schema_hash`, waiting until `deadline` for a retained record.
    ///
    /// A publisher's schema record reaches a new reader a short time after its first data frame.
    ///
    /// # Errors
    /// Returns [`SchemaError`] when the resolver itself failed.
    pub fn resolve_until(
        &self,
        bus: &BusNode,
        topic: &str,
        schema_hash: u64,
        deadline: Instant,
    ) -> Result<Option<Arc<ResolvedSchema>>, SchemaError> {
        loop {
            let now = Instant::now();
            // The paced retry protects a per-sample hot path, not a caller that waits.
            self.clear_retry(topic, schema_hash);
            let identity = self.resolve(bus, topic, schema_hash, now)?;
            if identity.is_some() {
                return Ok(identity);
            }
            if now >= deadline {
                return Ok(None);
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    fn clear_retry(&self, topic: &str, schema_hash: u64) {
        let mut entries = self.entries.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(by_hash) = entries.get_mut(topic)
            && matches!(by_hash.get(&schema_hash), Some(Entry::RetryAfter(_)))
        {
            by_hash.remove(&schema_hash);
        }
    }
}

/// Delay between two attempts while waiting for a retained record to appear.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The instant at which an unresolved hash may be asked for again.
fn retry_deadline(now: Instant) -> Instant {
    now.checked_add(SCHEMA_RETRY_INTERVAL).unwrap_or(now)
}
