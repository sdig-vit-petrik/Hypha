//! Content-addressed file store and synchronous resolution for schemas, at
//! `<config.global.root_path()>/<config.global.prefix>_hypha_schemas/<hex(topic)>/<schema_hash:016x>`.
//!
//! Every endpoint that names an MD5 writes the record of its stream, an empty file when it
//! has no definition, so the records of a topic are the list of its streams
//! ([`topic_records`]). A record lives while its stream has a port: the endpoint whose
//! drop leaves the stream without publisher and subscriber removes it, and the topic's
//! directory once it is empty ([`SchemaRelease`]). One lock file
//! per domain orders that removal against writes, and every wait for it is bounded
//! by [`SCHEMA_LOCK_TIMEOUT`] so a frozen holder blocks no node.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration as StdDuration;

use hypha_core::port::{PortError, SchemaSelector, TopicSpec};

use crate::stream::Stream;
use hypha_wire::schema_record::{
    MAX_SCHEMA_RECORD_LEN, ParsedSchemaRecord, SchemaRecordError, parse_half_record,
    parse_schema_record, schema_record_frame_len, write_half_record, write_schema_record,
};
use std::fs::TryLockError;

pub(crate) const SCHEMA_MAX_PAYLOAD: usize = MAX_SCHEMA_RECORD_LEN;

/// How long a schema write or removal waits for the domain's schema lock.
const SCHEMA_LOCK_TIMEOUT: StdDuration = StdDuration::from_millis(500);

const SCHEMA_LOCK_POLL: StdDuration = StdDuration::from_millis(2);

static WRITE_COUNTER: AtomicU64 = AtomicU64::new(0);
static INVALID_RECORDS: AtomicU64 = AtomicU64::new(0);

/// Number of corrupted or invalid schema records encountered.
#[must_use]
pub fn invalid_record_count() -> u64 {
    INVALID_RECORDS.load(Ordering::Relaxed)
}

/// Atomically writes `bytes` to `path` via a temp file in the same directory.
pub(crate) fn atomic_write_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "path has no parent directory")
    })?;
    std::fs::create_dir_all(parent)?;
    let count = WRITE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("tmp");
    let temp_name = format!(".{file_name}.tmp.{pid}.{count}");
    let temp_path = parent.join(temp_name);

    let write_res = (|| -> std::io::Result<()> {
        use std::io::Write;
        let mut file = std::fs::File::create(&temp_path)?;
        file.write_all(bytes)?;
        std::fs::rename(&temp_path, path)?;
        Ok(())
    })();

    if write_res.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    write_res
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().saturating_mul(2));
    for &b in bytes {
        use std::fmt::Write;
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// The schema directory of the domain.
fn schema_dir() -> Result<PathBuf, PortError> {
    let config = crate::domain::config()?;
    let root = Path::new(config.global.root_path().as_str());
    let prefix = config.global.prefix.as_str();
    Ok(root.join(format!("{prefix}_hypha_schemas")))
}

/// The directory that holds every schema of `topic`.
fn topic_dir(topic: &str) -> Result<PathBuf, PortError> {
    Ok(schema_dir()?.join(hex_encode(topic.as_bytes())))
}

/// Computes the content-addressed file path for a topic and schema hash.
///
/// # Errors
/// Returns [`PortError`] when domain configuration fails.
pub fn schema_file_path(topic: &str, schema_hash: u64) -> Result<PathBuf, PortError> {
    Ok(topic_dir(topic)?.join(format!("{schema_hash:016x}")))
}

/// Opens the domain's schema lock. Its name is not a domain directory, so removing
/// a domain's directory never removes a lock that another process holds.
fn schema_lock() -> std::io::Result<std::fs::File> {
    let config =
        crate::domain::config().map_err(|cause| std::io::Error::other(format!("{cause:?}")))?;
    let root = Path::new(config.global.root_path().as_str());
    let prefix = config.global.prefix.as_str();
    std::fs::create_dir_all(root)?;
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join(format!("{prefix}hypha_schemas.lock")))
}

/// How the schema lock is held: shared by writers, alone by a remover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LockMode {
    Shared,
    Exclusive,
}

/// Opens the domain's schema lock and holds it in `mode`, waiting at most
/// [`SCHEMA_LOCK_TIMEOUT`]. The returned file holds the lock until dropped.
///
/// # Errors
/// A message naming the timeout when another process holds the lock for the whole wait,
/// or the cause when the lock file cannot be opened or locked.
fn lock_schemas(mode: LockMode) -> Result<std::fs::File, String> {
    let lock = schema_lock().map_err(|cause| format!("cannot open the schema lock: {cause}"))?;
    let started = std::time::Instant::now();
    loop {
        let attempt = match mode {
            LockMode::Shared => lock.try_lock_shared(),
            LockMode::Exclusive => lock.try_lock(),
        };
        match attempt {
            Ok(()) => return Ok(lock),
            Err(TryLockError::Error(cause)) => {
                return Err(format!("cannot lock the schema lock: {cause}"));
            }
            Err(TryLockError::WouldBlock) => {
                let waited_out = started.elapsed() >= SCHEMA_LOCK_TIMEOUT;
                if waited_out {
                    return Err(format!(
                        "another process held the domain's schema lock for {} ms; it may be frozen",
                        SCHEMA_LOCK_TIMEOUT.as_millis()
                    ));
                }
                std::thread::sleep(SCHEMA_LOCK_POLL);
            }
        }
    }
}

/// True while the data service of `stream` has a publisher or a subscriber, in any process
/// of the domain: one open attempt through `node`, no retry. A service that is being
/// created or removed counts as in use, so its record stays for a later release.
fn stream_has_ports(node: &crate::transport::SharedNode, stream: &Stream) -> Result<bool, String> {
    use iceoryx2::service::builder::publish_subscribe::PublishSubscribeOpenError as Open;
    use iceoryx2::service::port_factory::PortFactory as _;
    let name = iceoryx2::prelude::ServiceName::new(&stream.service_name())
        .map_err(|cause| format!("{cause:?}"))?;
    let locked_node = crate::locked(node);
    let Some(node_ref) = locked_node.as_ref() else {
        return Err("the session is gone".to_owned());
    };
    match crate::transport::data_service_builder(node_ref, &name).open() {
        Ok(service) => {
            let dynamic = service.dynamic_config();
            Ok(dynamic.number_of_publishers() > 0 || dynamic.number_of_subscribers() > 0)
        }
        Err(Open::DoesNotExist) => Ok(false),
        Err(_) => Ok(true),
    }
}

/// Removes the record of a stream when it drops, if the stream has no port left in any
/// process. An endpoint holds it as its last field, so its own port is gone by then.
pub(crate) struct SchemaRelease {
    node: crate::transport::SharedNode,
    stream: Stream,
}

impl SchemaRelease {
    pub(crate) fn new(node: &crate::transport::SharedNode, stream: &Stream) -> Self {
        Self { node: node.clone(), stream: stream.clone() }
    }
}

impl Drop for SchemaRelease {
    fn drop(&mut self) {
        if let Err(cause) = release_schema(&self.node, &self.stream) {
            log::warn!(target: "hypha/schema",
                "hypha_iceoryx2: the schema {:016x} of topic '{}' stays on disk: {cause}",
                self.stream.schema_hash, self.stream.topic);
        }
    }
}

/// Removes the record of `stream` when the stream has no port, then the topic's directory
/// and the domain's when they are empty.
fn release_schema(node: &crate::transport::SharedNode, stream: &Stream) -> Result<(), String> {
    // No lock for the usual case: the last port to drop checks again under the lock.
    if stream_has_ports(node, stream)? {
        return Ok(());
    }
    let _lock = lock_schemas(LockMode::Exclusive)?;
    if stream_has_ports(node, stream)? {
        return Ok(());
    }
    let record = schema_file_path(&stream.topic, stream.schema_hash)
        .map_err(|cause| format!("{cause:?}"))?;
    match std::fs::remove_file(&record) {
        Ok(()) => {}
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
        Err(cause) => return Err(format!("cannot remove {}: {cause}", record.display())),
    }
    remove_empty_dir(&topic_dir(&stream.topic).map_err(|cause| format!("{cause:?}"))?)?;
    remove_empty_dir(&schema_dir().map_err(|cause| format!("{cause:?}"))?)
}

/// Removes the record of `stream`, whose service a probe found gone: the record of an
/// endpoint whose process died, once a node start reclaimed its ports and the service went
/// with them. Checked again under the exclusive lock, so a stream opened meanwhile keeps it.
pub(crate) fn remove_orphan_record(stream: &Stream) -> Result<(), String> {
    use iceoryx2::service::Service as _;
    let _lock = lock_schemas(LockMode::Exclusive)?;
    let config = crate::domain::config().map_err(|cause| format!("{cause:?}"))?;
    let service = iceoryx2::prelude::ServiceName::new(&stream.service_name())
        .map_err(|cause| format!("{cause:?}"))?;
    let exists = crate::transport::Ipc::does_exist(
        &service,
        config,
        iceoryx2::prelude::MessagingPattern::PublishSubscribe,
    )
    .map_err(|cause| format!("{cause:?}"))?;
    if exists {
        return Ok(());
    }
    let record = schema_file_path(&stream.topic, stream.schema_hash)
        .map_err(|cause| format!("{cause:?}"))?;
    match std::fs::remove_file(&record) {
        Ok(()) => {}
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
        Err(cause) => return Err(format!("cannot remove {}: {cause}", record.display())),
    }
    remove_empty_dir(&topic_dir(&stream.topic).map_err(|cause| format!("{cause:?}"))?)?;
    remove_empty_dir(&schema_dir().map_err(|cause| format!("{cause:?}"))?)
}

/// Removes `dir` if it is empty; a missing or non-empty directory is left as it is.
fn remove_empty_dir(dir: &Path) -> Result<(), String> {
    match std::fs::remove_dir(dir) {
        Ok(()) => Ok(()),
        Err(cause)
            if matches!(
                cause.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
            ) =>
        {
            Ok(())
        }
        Err(cause) => Err(format!("cannot remove {}: {cause}", dir.display())),
    }
}

/// One stream of a topic, as its record names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedStream {
    /// The schema hash of the stream.
    pub schema_hash: u64,
    /// The message type; empty when the stream's endpoints gave no definition.
    pub data_type: String,
    /// The message MD5; empty when the stream's endpoints gave no definition.
    pub md5: String,
}

/// The streams of `topic` that have a record: one read of its directory and of each
/// record. A record that does not parse is counted as an invalid record and left out.
///
/// # Errors
/// [`PortError`] when the domain configuration fails.
pub fn topic_records(topic: &str) -> Result<Vec<RecordedStream>, PortError> {
    let Ok(entries) = std::fs::read_dir(topic_dir(topic)?) else {
        return Ok(Vec::new());
    };
    let mut streams = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(schema_hash) = name.to_str().and_then(|text| u64::from_str_radix(text, 16).ok())
        else {
            continue;
        };
        let Ok(bytes) = std::fs::read(entry.path()) else { continue };
        if bytes.is_empty() {
            streams.push(RecordedStream {
                schema_hash,
                data_type: String::new(),
                md5: String::new(),
            });
            continue;
        }
        let Ok(record) = parse_schema_record(&bytes) else {
            INVALID_RECORDS.fetch_add(1, Ordering::Relaxed);
            continue;
        };
        streams.push(RecordedStream {
            schema_hash,
            data_type: record.data_type().to_owned(),
            md5: record.md5().to_owned(),
        });
    }
    Ok(streams)
}

/// Builds the fixed schema record frame for `spec`.
fn build_schema_frame(spec: &TopicSpec) -> Result<Vec<u8>, PortError> {
    if !spec.has_valid_public_name()
        || !spec.has_valid_schema_identity()
        || spec.definition.is_empty()
    {
        return Err(PortError::InvalidTopic);
    }
    let requested =
        schema_record_frame_len(spec.data_type.len(), spec.md5.len(), spec.definition.len())
            .ok_or(PortError::TooLarge { requested: usize::MAX, limit: SCHEMA_MAX_PAYLOAD })?;
    if requested > SCHEMA_MAX_PAYLOAD {
        return Err(PortError::TooLarge { requested, limit: SCHEMA_MAX_PAYLOAD });
    }
    let mut frame = vec![0_u8; requested];
    let frame_size = write_schema_record(&mut frame, &spec.data_type, &spec.md5, &spec.definition)
        .map_err(|cause| match cause {
            hypha_wire::schema_record::SchemaRecordError::OversizedRecord => {
                PortError::TooLarge { requested, limit: SCHEMA_MAX_PAYLOAD }
            }
            _ => PortError::InvalidTopic,
        })?;
    frame.truncate(frame_size);
    Ok(frame)
}

/// Writes the record of the stream of `spec`, an empty file when it has no definition;
/// nothing for an endpoint that names no MD5.
///
/// The caller has created its port first, so a [`SchemaRelease`] that runs meanwhile
/// either sees the port and keeps the file or removed it before this write. An existing
/// record is not rewritten, except an empty one by a record with a definition.
pub(crate) fn write_schema_file(spec: &TopicSpec) -> Result<(), PortError> {
    if !spec.has_valid_schema_identity() {
        return Ok(());
    }
    let frame = if spec.definition.is_empty() { Vec::new() } else { build_schema_frame(spec)? };
    let schema_hash = hypha_wire::schema_hash_from_md5(&spec.md5).ok_or(PortError::InvalidTopic)?;
    let path = schema_file_path(&spec.name, schema_hash)?;
    let fail = |err: std::io::Error| {
        log::error!(
            target: "hypha/schema",
            "failed to write schema file at {}: {err:?}",
            path.display()
        );
        PortError::Backend(3)
    };
    // The existence check comes after the lock: only the lock orders this write
    // against a removal that started before its service open.
    let _lock = lock_schemas(LockMode::Shared).map_err(|cause| {
        log::error!(
            target: "hypha/schema",
            "the schema of topic '{}' was not written to {}: {cause}",
            spec.name,
            path.display()
        );
        PortError::Backend(3)
    })?;
    // A record with a definition replaces one without; nothing else is rewritten.
    let recorded = std::fs::metadata(&path).map(|metadata| metadata.len());
    if recorded.is_ok_and(|length| length > 0 || frame.is_empty()) {
        return Ok(());
    }
    atomic_write_file(&path, &frame).map_err(&fail)
}

/// Synchronously look up a schema record for `topic` and `wanted`.
///
/// Re-checks every 10 ms until `timeout` expires or the record is found. Corrupted
/// or invalid files are counted and return `Ok(None)`.
///
/// # Errors
/// Returns [`PortError::InvalidTopic`] when `wanted` names a malformed MD5,
/// and another [`PortError`] when domain configuration fails.
pub fn lookup_schema(
    topic: &str,
    wanted: SchemaSelector,
    timeout: StdDuration,
) -> Result<Option<Vec<u8>>, PortError> {
    lookup_record(topic, wanted, timeout, parse_schema_record)
}

/// [`lookup_schema`] with the parser that verifies the record: the one of a service half
/// accepts an empty definition.
fn lookup_record(
    topic: &str,
    wanted: SchemaSelector,
    timeout: StdDuration,
    parse: fn(&[u8]) -> Result<ParsedSchemaRecord<'_>, SchemaRecordError>,
) -> Result<Option<Vec<u8>>, PortError> {
    let schema_hash = match &wanted {
        SchemaSelector::Hash(hash) => *hash,
        SchemaSelector::Md5(md5) => {
            hypha_wire::schema_hash_from_md5(md5).ok_or(PortError::InvalidTopic)?
        }
    };
    let path = schema_file_path(topic, schema_hash)?;
    let start = std::time::Instant::now();
    let poll_interval = StdDuration::from_millis(10);

    loop {
        if path.exists() {
            match std::fs::read(&path) {
                // A stream whose endpoints gave no definition: nothing to resolve yet.
                Ok(bytes) if bytes.is_empty() => {}
                Ok(bytes) => match parse(&bytes) {
                    Ok(record) => {
                        let hash_matches =
                            hypha_wire::schema_hash_from_md5(record.md5()) == Some(schema_hash);
                        let md5_matches = match &wanted {
                            SchemaSelector::Md5(expected_md5) => record.md5() == expected_md5,
                            SchemaSelector::Hash(_) => true,
                        };
                        if hash_matches && md5_matches {
                            return Ok(Some(bytes));
                        }
                        log::error!(
                            target: "hypha/schema",
                            "schema file at {} failed identity verification",
                            path.display()
                        );
                        INVALID_RECORDS.fetch_add(1, Ordering::Relaxed);
                        return Ok(None);
                    }
                    Err(err) => {
                        log::error!(
                            target: "hypha/schema",
                            "schema file at {} is corrupted: {err:?}",
                            path.display()
                        );
                        INVALID_RECORDS.fetch_add(1, Ordering::Relaxed);
                        return Ok(None);
                    }
                },
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    // File might have been concurrently renamed
                }
                Err(err) => {
                    log::error!(
                        target: "hypha/schema",
                        "failed to read schema file at {}: {err:?}",
                        path.display()
                    );
                    INVALID_RECORDS.fetch_add(1, Ordering::Relaxed);
                    return Ok(None);
                }
            }
        }

        if start.elapsed() >= timeout {
            return Ok(None);
        }
        let remaining = timeout.saturating_sub(start.elapsed());
        std::thread::sleep(remaining.min(poll_interval));
    }
}

/// Which half of a service a record describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceHalf {
    /// The request, `<type>Request`.
    Request,
    /// The response, `<type>Response`.
    Response,
}

impl ServiceHalf {
    const fn word(self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::Response => "response",
        }
    }
}

/// The store key of one half of `service`: `<service>␟request` or `<service>␟response`, the
/// role codes the service's frames are sealed with. `␟` (U+001F) is in no topic name, so no
/// topic's records share the directory.
#[must_use]
pub fn service_schema_key(service: &str, half: ServiceHalf) -> String {
    format!("{service}\u{1f}{}", half.word())
}

/// The definitions a server registered for its stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceSchema {
    /// The request type, `std_srvs/SetBoolRequest`.
    pub request_type: String,
    /// The request definition: the `.msg` text of the request half with its dependencies.
    pub request_definition: String,
    /// The response type, `std_srvs/SetBoolResponse`.
    pub response_type: String,
    /// The response definition.
    pub response_definition: String,
}

/// The records of a server's stream, removed when the server drops. A server holds it
/// before its name lock, so the next server of the name writes only after the removal.
pub(crate) struct ServiceSchemaRelease {
    paths: [PathBuf; 2],
}

impl Drop for ServiceSchemaRelease {
    fn drop(&mut self) {
        let removed = lock_schemas(LockMode::Exclusive).and_then(|_lock| {
            for path in &self.paths {
                match std::fs::remove_file(path) {
                    Ok(()) => {}
                    Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
                    Err(cause) => return Err(format!("cannot remove {}: {cause}", path.display())),
                }
                if let Some(dir) = path.parent() {
                    remove_empty_dir(dir)?;
                }
            }
            remove_empty_dir(&schema_dir().map_err(|cause| format!("{cause:?}"))?)
        });
        if let Err(cause) = removed {
            log::warn!(target: "hypha/schema", "hypha_iceoryx2: a service's schema stays on disk: {cause}");
        }
    }
}

/// The record frame of one half: its type, the service MD5 (which names the stream, so the
/// file name is that stream's schema hash) and its definition.
fn service_record_frame(data_type: &str, md5: &str, definition: &str) -> Result<Vec<u8>, String> {
    let requested = schema_record_frame_len(data_type.len(), md5.len(), definition.len())
        .ok_or_else(|| "the definition is too long".to_owned())?;
    if requested > SCHEMA_MAX_PAYLOAD {
        return Err(format!(
            "the definition of {data_type} takes {requested} B, above the record bound of {SCHEMA_MAX_PAYLOAD} B"
        ));
    }
    let mut frame = vec![0_u8; requested];
    let size = write_half_record(&mut frame, data_type, md5, definition)
        .map_err(|cause| format!("{cause:?}"))?;
    frame.truncate(size);
    Ok(frame)
}

/// The MD5 of a service whose two halves have no fields (`std_srvs/Empty`): that of the
/// empty text, since the service MD5 is taken over the texts of both halves.
const EMPTY_SERVICE_MD5: &str = "d41d8cd98f00b204e9800998ecf8427e";

/// Writes the records of both halves of `spec`'s stream, for a server that just took the
/// name. A half of no fields is an empty definition and has its record. `Ok(None)` for a
/// server that gave no definitions: both halves empty under the MD5 of a service that has
/// fields, since the empty service alone has the MD5 of the empty text.
///
/// # Errors
/// The reason when a record cannot be built or written; the server stands without one.
pub(crate) fn write_service_schema(
    spec: &hypha_core::port::ServiceSpec,
) -> Result<Option<ServiceSchemaRelease>, String> {
    let none_given = spec.request_definition.is_empty() && spec.response_definition.is_empty();
    if none_given && spec.md5 != EMPTY_SERVICE_MD5 {
        return Ok(None);
    }
    let schema_hash =
        hypha_wire::schema_hash_from_md5(&spec.md5).ok_or_else(|| "invalid MD5".to_owned())?;
    let halves = [
        (ServiceHalf::Request, "Request", &spec.request_definition),
        (ServiceHalf::Response, "Response", &spec.response_definition),
    ];
    let mut records = Vec::with_capacity(2);
    for (half, suffix, definition) in halves {
        let key = service_schema_key(&spec.name, half);
        let path = schema_file_path(&key, schema_hash).map_err(|cause| format!("{cause:?}"))?;
        let data_type = format!("{}{suffix}", spec.type_name);
        records.push((path, service_record_frame(&data_type, &spec.md5, definition)?));
    }
    let _lock = lock_schemas(LockMode::Shared)?;
    for (path, frame) in &records {
        atomic_write_file(path, frame)
            .map_err(|cause| format!("cannot write {}: {cause}", path.display()))?;
    }
    let [(request, _), (response, _)] =
        <[_; 2]>::try_from(records).map_err(|_| "two records were built".to_owned())?;
    Ok(Some(ServiceSchemaRelease { paths: [request, response] }))
}

/// The definitions registered for the stream of `service` and `md5`, waiting up to
/// `timeout` for a server that is writing them. `Ok(None)` when none is registered: no
/// server of the stream, or a server that gave none.
///
/// # Errors
/// [`PortError::InvalidTopic`] for a malformed MD5, and another [`PortError`] when the
/// domain configuration fails. A record that does not verify is counted and is `None`.
pub fn lookup_service_schema(
    service: &str,
    md5: &str,
    timeout: StdDuration,
) -> Result<Option<ServiceSchema>, PortError> {
    let wanted = || SchemaSelector::Md5(md5.to_owned());
    let request_key = service_schema_key(service, ServiceHalf::Request);
    let Some(request) = lookup_record(&request_key, wanted(), timeout, parse_half_record)? else {
        return Ok(None);
    };
    let response_key = service_schema_key(service, ServiceHalf::Response);
    let Some(response) = lookup_record(&response_key, wanted(), timeout, parse_half_record)? else {
        return Ok(None);
    };
    let (Ok(request), Ok(response)) = (parse_half_record(&request), parse_half_record(&response))
    else {
        INVALID_RECORDS.fetch_add(1, Ordering::Relaxed);
        return Ok(None);
    };
    Ok(Some(ServiceSchema {
        request_type: request.data_type().to_owned(),
        request_definition: request.definition().to_owned(),
        response_type: response.data_type().to_owned(),
        response_definition: response.definition().to_owned(),
    }))
}
