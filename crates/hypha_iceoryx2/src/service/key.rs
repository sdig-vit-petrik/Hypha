//! The stream key of a service and every name and code derived from it.
//!
//! A stream is a service name and the schema hash of its `.srv` MD5, as a topic stream
//! is a topic name and the hash of its message MD5 (PR #266). This is the one place that
//! spells the key, so it can be unified with the topic streams when #266 lands.

use hypha_core::port::{ServiceError, ServiceSpec};
use hypha_wire::frame::TopicCode;

/// The separator of the roles in a frame's code: a control character, never part of a name.
const ROLE: char = '\u{1f}';

/// The framework namespace every service's iceoryx2 services live under.
pub(crate) const SERVICE_PREFIX: &str = "hypha/srv/";

/// One stream of one service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StreamKey {
    /// The public name, `/a/b`.
    public: String,
    /// The name without its leading `/`.
    bare: String,
    /// The schema hash of the `.srv` MD5.
    hash: u64,
}

impl StreamKey {
    /// The key of `spec`, once the spec is valid.
    ///
    /// # Errors
    /// The error of [`ServiceSpec::validate`].
    pub(crate) fn of(spec: &ServiceSpec) -> Result<Self, ServiceError> {
        spec.validate()?;
        let hash = spec.schema_hash().ok_or(ServiceError::InvalidIdentity)?;
        let bare = spec.name.trim_start_matches('/').to_owned();
        Ok(Self { public: spec.name.clone(), bare, hash })
    }

    /// The public name.
    pub(crate) fn public(&self) -> &str {
        &self.public
    }

    /// The schema hash every frame of the stream carries.
    pub(crate) const fn hash(&self) -> u64 {
        self.hash
    }

    /// The request-response service of the stream.
    pub(crate) fn stream(&self) -> String {
        format!("{SERVICE_PREFIX}{}@{:016x}", self.bare, self.hash)
    }

    /// The event that wakes the stream's server: clients notify it.
    pub(crate) fn request_event(&self) -> String {
        format!("{}/request", self.stream())
    }

    /// The event that wakes the stream's clients: the server notifies it.
    pub(crate) fn response_event(&self) -> String {
        format!("{}/response", self.stream())
    }

    /// The name-level event whose single notifier slot is the name's server lock, across
    /// every version of the service.
    pub(crate) fn lock(&self) -> String {
        format!("{SERVICE_PREFIX}{}", self.bare)
    }

    /// The code a request of this service is sealed for. The separator is a control
    /// character, which no topic or service name may hold, so no topic frame and no frame
    /// of another service is sealed with it.
    pub(crate) fn request_code(&self) -> TopicCode {
        TopicCode::of(&format!("{}{ROLE}request", self.public))
    }

    /// The code the response to request `sequence` of client `client` is sealed for, so it
    /// verifies against that request only.
    pub(crate) fn response_code(&self, client: u64, sequence: u64) -> TopicCode {
        TopicCode::of(&format!(
            "{}{ROLE}response{ROLE}{client:016x}{ROLE}{sequence:016x}",
            self.public
        ))
    }
}

/// The public name of a stream's iceoryx2 service name, or `None` for another service.
pub(crate) fn public_name_of_stream(stream: &str) -> Option<(String, u64)> {
    let rest = stream.strip_prefix(SERVICE_PREFIX)?;
    let (bare, hex) = rest.rsplit_once('@')?;
    let is_key = hex.len() == 16 && hex.bytes().all(|b| b.is_ascii_hexdigit());
    if !is_key {
        return None;
    }
    let hash = u64::from_str_radix(hex, 16).ok()?;
    Some((format!("/{bare}"), hash))
}

#[cfg(test)]
mod tests {
    #![expect(clippy::unwrap_used, reason = "a test of known-valid specs")]

    use super::{StreamKey, public_name_of_stream};
    use hypha_core::port::{MAX_SERVICE_NAME_BYTES, ServiceSpec};

    const MD5: &str = "09fb03525b03e7ea1fd3992bafd87e16";

    #[test]
    fn the_longest_name_fits_iceoryx2_in_every_derived_service_name() {
        let name = format!("/{}", "a".repeat(MAX_SERVICE_NAME_BYTES));
        let key = StreamKey::of(&ServiceSpec::new(&name, "std_srvs/SetBool", MD5)).unwrap();
        for derived in [key.stream(), key.request_event(), key.response_event(), key.lock()] {
            assert!(
                derived.len() <= iceoryx2::constants::MAX_SERVICE_NAME_LENGTH,
                "{}",
                derived.len()
            );
            assert!(iceoryx2::prelude::ServiceName::new(&derived).is_ok());
        }
        assert_eq!(key.response_event().len(), iceoryx2::constants::MAX_SERVICE_NAME_LENGTH);
    }

    #[test]
    fn a_stream_name_gives_back_its_public_name_and_hash() {
        let key =
            StreamKey::of(&ServiceSpec::new("/uav1/arming", "std_srvs/SetBool", MD5)).unwrap();
        assert_eq!(
            public_name_of_stream(&key.stream()),
            Some(("/uav1/arming".to_owned(), key.hash()))
        );
        assert_eq!(public_name_of_stream(&key.request_event()), None);
        assert_eq!(public_name_of_stream("odom"), None);
    }

    #[test]
    fn no_topic_frame_seals_like_a_request_or_a_response() {
        use hypha_core::port::TopicSpec;
        let key = StreamKey::of(&ServiceSpec::new("/a", "std_srvs/SetBool", MD5)).unwrap();
        // Topic names that spell the old separators are still valid topics, and their codes
        // are not the service's.
        for topic in ["/a#request", "/a#response/0000000000000001/0000000000000001", "/a"] {
            assert!(TopicSpec::new(topic).has_valid_public_name(), "{topic}");
            let code = hypha_wire::frame::TopicCode::of(topic);
            assert_ne!(code, key.request_code(), "{topic}");
            assert_ne!(code, key.response_code(1, 1), "{topic}");
        }
        let spelled = format!("/a{}request", super::ROLE);
        assert!(
            !TopicSpec::new(&spelled).has_valid_public_name(),
            "no topic can spell the separator"
        );
    }

    #[test]
    fn the_role_codes_differ_by_direction_and_by_request() {
        let key = StreamKey::of(&ServiceSpec::new("/a", "std_srvs/SetBool", MD5)).unwrap();
        assert_ne!(key.request_code(), key.response_code(1, 1));
        assert_ne!(key.response_code(1, 1), key.response_code(1, 2));
        assert_ne!(key.response_code(1, 1), key.response_code(2, 1));
    }
}
