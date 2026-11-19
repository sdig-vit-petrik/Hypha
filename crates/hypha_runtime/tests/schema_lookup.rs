//! Schema lookup validation, caching, health counters and the identity bound.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Duration;

use hypha_core::port::{PortError, SchemaSelector, TopicSpec};
use hypha_wire::schema_record::write_schema_record;

use hypha_runtime::{ResolvedIdentity, SchemaFailure, SchemaState, SchemaTransport};

const MD5: &str = "fdb28210bfa9d7c91146260178d9a584";
const TYPE: &str = "std_msgs/Float64";

struct FakeTransport {
    answers: Mutex<VecDeque<Result<Option<Vec<u8>>, PortError>>>,
    requests: Mutex<Vec<SchemaSelector>>,
}

impl FakeTransport {
    fn new(answers: Vec<Result<Option<Vec<u8>>, PortError>>) -> Self {
        Self { answers: Mutex::new(answers.into()), requests: Mutex::new(Vec::new()) }
    }

    fn requests(&self) -> usize {
        self.requests.lock().map_or(0, |requests| requests.len())
    }
}

impl SchemaTransport for FakeTransport {
    fn lookup_schema(
        &self,
        _topic: &str,
        selector: SchemaSelector,
        _timeout: Duration,
    ) -> Result<Option<Vec<u8>>, PortError> {
        self.requests.lock().map_err(|_| PortError::Disconnected)?.push(selector);
        self.answers.lock().map_err(|_| PortError::Disconnected)?.pop_front().unwrap_or(Ok(None))
    }
}

#[allow(clippy::expect_used)]
fn record_for(md5: &str) -> Vec<u8> {
    let mut bytes = vec![0; 128];
    let written = write_schema_record(&mut bytes, TYPE, md5, "float64 data").expect("valid record");
    bytes.truncate(written);
    bytes
}

fn record() -> Vec<u8> {
    record_for(MD5)
}

fn spec_for(md5: &str) -> TopicSpec {
    TopicSpec::new("/values").with_type(TYPE).with_md5(md5)
}

fn spec() -> TopicSpec {
    spec_for(MD5)
}

#[test]
fn lookup_validates_and_caches_the_definition() {
    let transport = FakeTransport::new(vec![Ok(Some(record()))]);
    let state = SchemaState::default();
    assert_eq!(
        state.lookup(&transport, &spec(), Duration::from_millis(1)),
        Ok("float64 data".into())
    );
    assert_eq!(
        state.lookup(&transport, &spec(), Duration::from_millis(1)),
        Ok("float64 data".into())
    );
    assert_eq!(transport.requests(), 1);
    assert_eq!(state.cached_definition("/values", TYPE, MD5), Some("float64 data".into()));
}

#[test]
#[allow(clippy::expect_used)]
fn lookup_counts_unavailable_resource_and_invalid_responses() {
    let invalid_record = vec![0, 1, 2];
    let transport =
        FakeTransport::new(vec![Ok(None), Err(PortError::ResourceLimit), Ok(Some(invalid_record))]);
    let state = SchemaState::default();
    assert_eq!(state.lookup(&transport, &spec(), Duration::ZERO), Err(SchemaFailure::NotAvailable));
    assert_eq!(
        state.lookup(&transport, &spec(), Duration::ZERO),
        Err(SchemaFailure::ResourceLimit)
    );
    assert_eq!(
        state.lookup(&transport, &spec(), Duration::ZERO),
        Err(SchemaFailure::InvalidSchema)
    );
    let health = state.health();
    assert_eq!(health.unavailable_lookups, 1);
    assert_eq!(health.resource_limit_failures, 1);
    assert_eq!(health.invalid_records, 1);
}

#[test]
#[allow(clippy::expect_used)]
fn lookup_refuses_a_record_with_a_different_identity() {
    let mut bytes = vec![0; 128];
    let written = write_schema_record(&mut bytes, "std_msgs/String", MD5, "string data")
        .expect("valid record");
    bytes.truncate(written);
    let transport = FakeTransport::new(vec![Ok(Some(bytes))]);
    let state = SchemaState::default();
    assert_eq!(
        state.lookup(&transport, &spec(), Duration::ZERO),
        Err(SchemaFailure::InvalidSchema)
    );
    assert_eq!(state.health().invalid_records, 1);
}

#[test]
#[allow(clippy::expect_used)]
fn hash_resolution_caches_identity() {
    let transport = FakeTransport::new(vec![Ok(Some(record()))]);
    let state = SchemaState::default();
    let hash = hypha_wire::schema_hash_from_md5(MD5).expect("a valid MD5");
    let expected = ResolvedIdentity {
        data_type: TYPE.into(),
        md5: MD5.into(),
        definition: "float64 data".into(),
    };
    assert_eq!(
        state.resolve_hash(&transport, "/values", hash, Duration::ZERO),
        Ok(Some(expected.clone()))
    );
    assert_eq!(state.resolve_hash(&transport, "/values", hash, Duration::ZERO), Ok(Some(expected)));
    assert_eq!(transport.requests(), 1);
}

#[test]
fn hash_cache_evicts_oldest_identity_after_its_fixed_bound() {
    let state = SchemaState::default();
    let transport = FakeTransport::new((0_u64..514).map(|_| Ok(Some(record()))).collect());
    for hash in 1_u64..=513 {
        assert!(state.resolve_hash(&transport, "/values", hash, Duration::ZERO).is_ok());
    }
    assert_eq!(transport.requests(), 513);
    assert!(state.resolve_hash(&transport, "/values", 1, Duration::ZERO).is_ok());
    assert_eq!(transport.requests(), 514);
    assert!(state.resolve_hash(&transport, "/values", 513, Duration::ZERO).is_ok());
    assert_eq!(transport.requests(), 514);
}

#[test]
#[allow(clippy::expect_used)]
fn hash_resolution_counts_unavailable_resource_and_invalid_responses() {
    let invalid_record = vec![0, 1, 2];
    let transport =
        FakeTransport::new(vec![Ok(None), Err(PortError::ResourceLimit), Ok(Some(invalid_record))]);
    let state = SchemaState::default();
    let hash = hypha_wire::schema_hash_from_md5(MD5).expect("a valid MD5");
    assert_eq!(state.resolve_hash(&transport, "/values", hash, Duration::ZERO), Ok(None));
    assert_eq!(
        state.resolve_hash(&transport, "/values", hash, Duration::ZERO),
        Err(SchemaFailure::Transport(PortError::ResourceLimit))
    );
    assert_eq!(
        state.resolve_hash(&transport, "/values", hash, Duration::ZERO),
        Err(SchemaFailure::InvalidSchema)
    );
    let health = state.health();
    assert_eq!(health.unavailable_lookups, 1);
    assert_eq!(health.resource_limit_failures, 1);
    assert_eq!(health.invalid_records, 1);
}

#[test]
#[allow(clippy::expect_used)]
fn definition_cache_evicts_oldest_definition_after_its_fixed_bound() {
    let md5s: Vec<String> = (0_u128..513).map(|n| format!("{n:032x}")).collect();
    let oldest = md5s.first().expect("513 digests").clone();
    let mut answers: Vec<Result<Option<Vec<u8>>, PortError>> =
        md5s.iter().map(|md5| Ok(Some(record_for(md5)))).collect();
    answers.push(Ok(Some(record_for(&oldest))));
    let transport = FakeTransport::new(answers);
    let state = SchemaState::default();
    for md5 in &md5s {
        assert!(state.lookup(&transport, &spec_for(md5), Duration::ZERO).is_ok());
    }
    assert_eq!(transport.requests(), 513);
    assert!(state.lookup(&transport, &spec_for(&oldest), Duration::ZERO).is_ok());
    assert_eq!(transport.requests(), 514);
}

#[test]
#[allow(clippy::expect_used)]
fn invalid_answer_is_not_cached_so_a_later_valid_answer_is_accepted() {
    let transport = FakeTransport::new(vec![Ok(Some(vec![0, 1, 2])), Ok(Some(record()))]);
    let state = SchemaState::default();
    assert_eq!(
        state.lookup(&transport, &spec(), Duration::ZERO),
        Err(SchemaFailure::InvalidSchema)
    );
    assert_eq!(state.lookup(&transport, &spec(), Duration::ZERO), Ok("float64 data".into()));
    assert_eq!(transport.requests(), 2);
}

#[test]
#[allow(clippy::expect_used)]
fn cache_hit_with_a_different_type_asks_the_transport_again() {
    let mut string_bytes = vec![0; 128];
    let written = write_schema_record(&mut string_bytes, "std_msgs/String", MD5, "string data")
        .expect("valid record");
    string_bytes.truncate(written);
    let transport = FakeTransport::new(vec![Ok(Some(record())), Ok(Some(string_bytes))]);
    let state = SchemaState::default();
    assert_eq!(state.lookup(&transport, &spec(), Duration::ZERO), Ok("float64 data".into()));
    let string_spec = TopicSpec::new("/values").with_type("std_msgs/String").with_md5(MD5);
    assert_eq!(state.lookup(&transport, &string_spec, Duration::ZERO), Ok("string data".into()));
    assert_eq!(transport.requests(), 2);
}
