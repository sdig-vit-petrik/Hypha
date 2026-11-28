//! The streams of a topic: one data service for each schema a topic name carries.
//!
//! A topic name carries one stream per schema hash (topic name overloading, #209). The
//! stream of `/odom` with the schema hash `h` is the service `odom@<h in 16 hex digits>`,
//! and its wake-up event service has the same name. The suffix has a fixed length and is
//! read from the right, so every topic name, one with an `@` too, has one service name,
//! and a service name gives its topic and hash back.

use hypha_core::port::{MAX_RECORDED_TOPIC_NAME_BYTES, MAX_TOPIC_NAME_BYTES, PortError, TopicSpec};

/// The digits of the schema hash in a stream's service name.
const HASH_DIGITS: usize = 16;

/// Refuses, and logs, a topic name of `spec` too long for its stream: for its service
/// name, or for the schema directory of an endpoint that writes a record.
///
/// # Errors
/// [`PortError::InvalidTopic`] for a name longer than [`MAX_TOPIC_NAME_BYTES`] after its
/// `/`, or than [`MAX_RECORDED_TOPIC_NAME_BYTES`] for an endpoint that names an MD5.
pub(crate) fn refuse_overlong(spec: &TopicSpec) -> Result<(), PortError> {
    let bare = spec.name.strip_prefix('/').unwrap_or(&spec.name);
    let (limit, what) = if spec.has_valid_schema_identity() {
        (MAX_RECORDED_TOPIC_NAME_BYTES, "the schema directory of an endpoint with an MD5 leaves")
    } else {
        (MAX_TOPIC_NAME_BYTES, "a stream's service name leaves")
    };
    if bare.len() <= limit {
        return Ok(());
    }
    log::error!(target: "hypha/transport",
        "hypha_iceoryx2: topic '{}' is refused: {} bytes after its '/', more than the {limit} {what}",
        spec.name, bare.len());
    Err(PortError::InvalidTopic)
}

/// One stream: a topic name and the schema hash of the frames it carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Stream {
    /// The public topic name, with its leading `/`.
    pub(crate) topic: String,
    /// The schema hash of every frame of the stream; 0 for the stream of no schema.
    pub(crate) schema_hash: u64,
}

impl Stream {
    /// The stream an endpoint of `spec` publishes on or reads alone.
    ///
    /// # Errors
    /// [`PortError::InvalidTopic`] for an MD5 that is not one.
    pub(crate) fn of(spec: &TopicSpec) -> Result<Self, PortError> {
        let schema_hash = spec.stream_schema_hash().ok_or(PortError::InvalidTopic)?;
        Ok(Self { topic: spec.name.clone(), schema_hash })
    }

    /// The stream that the service `service_name` carries; `None` for a service that is no
    /// stream of this hypha build.
    pub(crate) fn of_service(service_name: &str) -> Option<Self> {
        let (bare, digits) = service_name.rsplit_once('@')?;
        let is_hash = digits.len() == HASH_DIGITS
            && digits.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if bare.is_empty() || !is_hash {
            return None;
        }
        let schema_hash = u64::from_str_radix(digits, 16).ok()?;
        Some(Self { topic: format!("/{bare}"), schema_hash })
    }

    /// The name of the stream's data service and of its event service.
    pub(crate) fn service_name(&self) -> String {
        stream_service_name(&self.topic, self.schema_hash)
    }
}

/// The service name of the stream of `topic` with `schema_hash`: the topic without one
/// leading `/`, then `@` and the hash in 16 hex digits, so `/odom` and `odom` are one
/// topic.
#[must_use]
pub fn stream_service_name(topic: &str, schema_hash: u64) -> String {
    let bare = topic.strip_prefix('/').unwrap_or(topic);
    format!("{bare}@{schema_hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::{Stream, stream_service_name};

    #[test]
    fn a_service_name_gives_back_its_topic_and_hash() {
        let name = stream_service_name("/camera/image", 0x0123_4567_89ab_cdef);
        assert_eq!(name, "camera/image@0123456789abcdef");
        assert_eq!(stream_service_name("camera/image", 0x0123_4567_89ab_cdef), name);
        let stream = Stream::of_service(&name);
        assert_eq!(
            stream,
            Some(Stream { topic: "/camera/image".to_owned(), schema_hash: 0x0123_4567_89ab_cdef })
        );
    }

    #[test]
    fn a_topic_with_an_at_sign_keeps_it() {
        let name = stream_service_name("/mail@0123456789abcdef", 7);
        let stream = Stream::of_service(&name).map(|stream| (stream.topic, stream.schema_hash));
        assert_eq!(stream, Some(("/mail@0123456789abcdef".to_owned(), 7)));
    }

    #[test]
    fn a_service_without_a_hash_suffix_is_no_stream() {
        for name in [
            "odom",
            "odom@",
            "odom@123",
            "@0123456789abcdef",
            "odom@+123456789abcdef",
            "odom@0123456789ABCDEF",
        ] {
            assert_eq!(Stream::of_service(name), None, "{name}");
        }
    }
}
