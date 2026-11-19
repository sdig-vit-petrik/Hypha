//! Schema housekeeping and resolution C ABI.

use std::time::Duration;

use hypha_core::port::{PortError, SchemaSelector, TopicSpec};
use hypha_runtime::{ResolvedIdentity, SchemaFailure, SchemaTransport};

use crate::node::HyphaNode;
use crate::status::Error;

pub(crate) use hypha_runtime::SchemaState;

/// Bounded wait for one schema RPC round trip issued from the C ABI.
const SCHEMA_LOOKUP_TIMEOUT: Duration = Duration::from_millis(200);

struct SchemaBus<'a>(&'a hypha_iceoryx2::Iox2Transport);

impl SchemaTransport for SchemaBus<'_> {
    fn lookup_schema(
        &self,
        topic: &str,
        selector: SchemaSelector,
        timeout: Duration,
    ) -> Result<Option<Vec<u8>>, PortError> {
        self.0.schema_lookup(topic, selector, timeout)
    }
}

fn failure_error(failure: SchemaFailure) -> Error {
    match failure {
        SchemaFailure::NotAvailable => Error::NotAvailable,
        SchemaFailure::InvalidSchema => Error::InvalidSchema,
        SchemaFailure::ResourceLimit => Error::ResourceLimit,
        SchemaFailure::Backend => Error::Backend,
        SchemaFailure::Transport(cause) => Error::from(cause),
    }
}

/// Resolve a compact frame schema hash on `topic` to its retained type identity.
///
/// # Errors
/// `NotAvailable` when no retained record is available, `InvalidState` inside a
/// `WaitSet` run, and the resolver's error otherwise.
pub fn schema_resolve_hash(
    node: &HyphaNode,
    topic: &str,
    schema_hash: u64,
) -> Result<ResolvedIdentity, Error> {
    resolve_hash_within(node, topic, schema_hash, SCHEMA_LOOKUP_TIMEOUT)
}

/// [`schema_resolve_hash`] without the wait for a record that appears late.
///
/// # Errors
/// `NotAvailable` when no retained record is there now, `InvalidState` inside a
/// `WaitSet` run, and the resolver's error otherwise.
pub fn schema_resolve_hash_now(
    node: &HyphaNode,
    topic: &str,
    schema_hash: u64,
) -> Result<ResolvedIdentity, Error> {
    resolve_hash_within(node, topic, schema_hash, Duration::ZERO)
}

fn resolve_hash_within(
    node: &HyphaNode,
    topic: &str,
    schema_hash: u64,
    wait: Duration,
) -> Result<ResolvedIdentity, Error> {
    if crate::waitset::in_dispatch() {
        return Err(Error::InvalidState);
    }
    node.schema
        .resolve_hash(&SchemaBus(&node.transport), topic, schema_hash, wait)
        .map_err(failure_error)?
        .ok_or(Error::NotAvailable)
}

/// Resolve the message definition for `(topic, data_type, md5)`.
///
/// # Errors
/// `InvalidTopic` for an invalid name or identity, `InvalidState` for a lookup that is
/// not cached inside a `WaitSet` run, `NotAvailable` when no retained record is
/// available, and the resolver's error otherwise.
pub fn schema_lookup(
    node: &HyphaNode,
    topic: &str,
    data_type: &str,
    md5: &str,
) -> Result<String, Error> {
    let spec = TopicSpec::new(topic).with_type(data_type).with_md5(md5);
    let valid_identity = spec.has_valid_public_name() && spec.has_valid_schema_identity();
    if !valid_identity {
        return Err(Error::InvalidTopic);
    }
    // A synchronous lookup waits for an RPC round trip, which would stall a dispatcher
    // callback, so it is refused there.
    let is_cached = node.schema.cached_definition(topic, data_type, md5).is_some();
    if !is_cached && crate::waitset::in_dispatch() {
        return Err(Error::InvalidState);
    }
    node.schema
        .lookup(&SchemaBus(&node.transport), &spec, SCHEMA_LOOKUP_TIMEOUT)
        .map_err(failure_error)
}

/// Emit the `schema.*` metrics of `node`.
pub(crate) fn emit_schema_metrics(node: &HyphaNode, emit: &mut dyn FnMut(&str, u64)) {
    let health = node.schema.health();
    let file_invalid_records = hypha_iceoryx2::invalid_record_count();
    emit("schema.invalid_records", health.invalid_records.saturating_add(file_invalid_records));
    emit("schema.unavailable_lookups", health.unavailable_lookups);
    emit("schema.resource_limit_failures", health.resource_limit_failures);
}
