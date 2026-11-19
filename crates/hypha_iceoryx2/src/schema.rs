//! Content-addressed file store and synchronous resolution for schemas, at
//! `<config.global.root_path()>/<config.global.prefix>_hypha_schemas/<hex(topic)>/<schema_hash:016x>`.
//!
//! A topic's schemas live as long as its data service: the endpoint that drops
//! the last port removes the topic's directory ([`SchemaRelease`]). One lock file
//! per domain orders that removal against writes, and every wait for it is bounded
//! by [`SCHEMA_LOCK_TIMEOUT`] so a frozen holder blocks no node.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration as StdDuration;

use hypha_core::port::{PortError, SchemaSelector, TopicSpec};
use hypha_wire::schema_record::{
    MAX_SCHEMA_RECORD_LEN, parse_schema_record, schema_record_frame_len, write_schema_record,
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

/// True while any port of `topic`'s data service is open in any process of the domain.
fn topic_in_use(topic: &str) -> Result<bool, String> {
    use iceoryx2::service::Service as _;
    let config = crate::domain::config().map_err(|cause| format!("{cause:?}"))?;
    let name = iceoryx2::prelude::ServiceName::new(&crate::transport::service_name_for(topic))
        .map_err(|cause| format!("{cause:?}"))?;
    crate::transport::Ipc::does_exist(
        &name,
        config,
        iceoryx2::prelude::MessagingPattern::PublishSubscribe,
    )
    .map_err(|cause| format!("cannot tell whether the topic is still open: {cause:?}"))
}

/// Removes the schemas of a topic when it drops, if no port of its data service is
/// left in any process. An endpoint holds it as its last field.
#[derive(Debug)]
pub(crate) struct SchemaRelease {
    topic: String,
}

impl SchemaRelease {
    pub(crate) fn new(topic: &str) -> Self {
        Self { topic: topic.to_owned() }
    }
}

impl Drop for SchemaRelease {
    fn drop(&mut self) {
        if let Err(cause) = release_schemas(&self.topic) {
            log::warn!(target: "hypha/schema",
                "hypha_iceoryx2: the schemas of topic '{}' stay on disk: {cause}", self.topic);
        }
    }
}

/// Removes the schema directory of `topic` when its data service is gone.
fn release_schemas(topic: &str) -> Result<(), String> {
    // No lock for the usual case: the last port to drop checks again under the lock.
    if topic_in_use(topic)? {
        return Ok(());
    }
    let _lock = lock_schemas(LockMode::Exclusive)?;
    if topic_in_use(topic)? {
        return Ok(());
    }
    let dir = topic_dir(topic).map_err(|cause| format!("{cause:?}"))?;
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => {}
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
        Err(cause) => return Err(format!("cannot remove {}: {cause}", dir.display())),
    }
    let domain_dir = schema_dir().map_err(|cause| format!("{cause:?}"))?;
    match std::fs::remove_dir(&domain_dir) {
        Ok(()) => Ok(()),
        Err(cause)
            if matches!(
                cause.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
            ) =>
        {
            Ok(())
        }
        Err(cause) => Err(format!("cannot remove {}: {cause}", domain_dir.display())),
    }
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

/// Writes the schema record file for `spec`.
///
///
/// The caller has opened the topic's data service first, so a [`SchemaRelease`]
/// that runs meanwhile keeps the file. An existing file is not rewritten.
pub(crate) fn write_schema_file(spec: &TopicSpec) -> Result<(), PortError> {
    let frame = build_schema_frame(spec)?;
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
    if path.exists() {
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
                Ok(bytes) => match parse_schema_record(&bytes) {
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
