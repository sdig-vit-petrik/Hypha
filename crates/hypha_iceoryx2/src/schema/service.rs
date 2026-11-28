//! The schema records of a service's two halves, which tools read to encode a request.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration as StdDuration;

use hypha_core::port::{PortError, SchemaSelector};
use hypha_wire::schema_record::{parse_half_record, schema_record_frame_len, write_half_record};

use super::{
    INVALID_RECORDS, LockMode, SCHEMA_MAX_PAYLOAD, atomic_write_file, lock_schemas, lookup_record,
    remove_empty_dir, schema_dir, schema_file_path,
};

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
