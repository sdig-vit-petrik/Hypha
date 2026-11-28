//! What a caller asks of a service: request and response between nodes, typed by a
//! ROS 1 `.srv` (`docs/design/native-services.md`).

use alloc::string::{String, ToString as _};
use core::fmt;
use core::time::Duration;

use crate::port::error::PortError;
use crate::port::spec::{TopicSpec, check_policy_count, is_policy_name};

/// The longest service name, in bytes.
///
/// iceoryx2 names hold 255 bytes; the prefix `hypha/srv/`, the stream key `@<16 hex>` and
/// the event suffix `/response` take 36 of them.
pub const MAX_SERVICE_NAME_BYTES: usize = 219;

/// Clients a service admits beyond its policy's, for the tools (`hypha service call`).
pub const SERVICE_OBSERVER_CLIENTS: usize = 3;

/// The largest client count a service policy may name.
pub const MAX_POLICY_CLIENTS: usize = 256;

/// The most calls one client may have in flight under a service policy.
pub const MAX_POLICY_OUTSTANDING: usize = 64;

/// The bound of a request or a response under the `defaultService` policy.
pub const DEFAULT_SERVICE_PAYLOAD: usize = 4096;

/// The largest bound a service policy may give a request or a response: 64 MiB.
pub const MAX_SERVICE_PAYLOAD: usize = 64 << 20;

/// The longest timeout a service policy may name: one hour.
pub const MAX_SERVICE_TIMEOUT: Duration = Duration::from_secs(3600);

/// The limits of a service, stamped on it by its creator. An opener that declares
/// another policy is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServicePolicy {
    /// The policy's name, as the stamp carries it.
    pub name: String,
    /// Clients the service admits, beyond the [`SERVICE_OBSERVER_CLIENTS`].
    pub max_clients: usize,
    /// Calls one client may have in flight.
    pub max_outstanding: usize,
    /// The largest request payload, in bytes.
    pub max_request_bytes: usize,
    /// The largest response payload, in bytes.
    pub max_response_bytes: usize,
    /// The timeout of a call whose client names none.
    pub timeout: Duration,
}

impl ServicePolicy {
    /// The policy of a service whose type maps to no other: 8 clients, 2 calls in flight
    /// each, 4 KiB each way, 1 s.
    #[must_use]
    pub fn default_service() -> Self {
        Self {
            name: "defaultService".to_string(),
            max_clients: 8,
            max_outstanding: 2,
            max_request_bytes: DEFAULT_SERVICE_PAYLOAD,
            max_response_bytes: DEFAULT_SERVICE_PAYLOAD,
            timeout: Duration::from_secs(1),
        }
    }

    /// Clients the transport admits: the policy's and the observers'.
    #[must_use]
    pub const fn admitted_clients(&self) -> usize {
        self.max_clients.saturating_add(SERVICE_OBSERVER_CLIENTS)
    }

    /// Checks the name and every limit against the hard bounds.
    ///
    /// # Errors
    /// A message that names the first field out of bounds.
    pub fn validate(&self) -> Result<(), String> {
        if !is_policy_name(&self.name) {
            return Err(format!("service policy name '{}' is not a policy name", self.name));
        }
        check_policy_count("max_clients", self.max_clients, MAX_POLICY_CLIENTS)?;
        check_policy_count("max_outstanding", self.max_outstanding, MAX_POLICY_OUTSTANDING)?;
        check_policy_count("max_request_bytes", self.max_request_bytes, MAX_SERVICE_PAYLOAD)?;
        check_policy_count("max_response_bytes", self.max_response_bytes, MAX_SERVICE_PAYLOAD)?;
        if self.timeout.is_zero() || self.timeout > MAX_SERVICE_TIMEOUT {
            return Err(format!(
                "service policy timeout {:?} is not above zero and at most {MAX_SERVICE_TIMEOUT:?}",
                self.timeout
            ));
        }
        Ok(())
    }
}

/// One stream of a service: its name, its `.srv` type and identity, and its policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceSpec {
    /// The resolved name, starting with `/`, as a topic's.
    pub name: String,
    /// The `.srv` type, as `std_srvs/SetBool`.
    pub type_name: String,
    /// The ROS 1 service MD5 over both halves: 32 lower-case hexadecimal digits.
    pub md5: String,
    /// The limits.
    pub policy: ServicePolicy,
    /// The definition text of the request half (`<type>Request`), as a `.msg` with its
    /// dependencies; empty for a half of no fields, or when the server gives none. A server
    /// registers it in the schema store so a tool can encode a request it did not compile.
    /// Both halves empty count as none unless `md5` is that of the empty text, the MD5 of
    /// `std_srvs/Empty`.
    pub request_definition: String,
    /// The definition text of the response half (`<type>Response`); empty as the request's.
    pub response_definition: String,
}

impl ServiceSpec {
    /// A spec under the `defaultService` policy.
    #[must_use]
    pub fn new(name: &str, type_name: &str, md5: &str) -> Self {
        Self {
            name: name.to_string(),
            type_name: type_name.to_string(),
            md5: md5.to_string(),
            policy: ServicePolicy::default_service(),
            request_definition: String::new(),
            response_definition: String::new(),
        }
    }

    /// The same spec with the definitions of its two halves.
    #[must_use]
    pub fn with_definitions(mut self, request: &str, response: &str) -> Self {
        self.request_definition = request.to_string();
        self.response_definition = response.to_string();
        self
    }

    /// The same spec under `policy`.
    #[must_use]
    pub fn with_policy(mut self, policy: ServicePolicy) -> Self {
        self.policy = policy;
        self
    }

    /// The 64-bit schema hash the frames of this stream carry: the first eight bytes of the MD5.
    #[must_use]
    pub fn schema_hash(&self) -> Option<u64> {
        hypha_wire::schema_hash_from_md5(&self.md5)
    }

    /// Checks the name, the identity and the policy.
    ///
    /// # Errors
    /// [`ServiceError::InvalidName`], [`ServiceError::NameTooLong`],
    /// [`ServiceError::InvalidIdentity`] or [`ServiceError::InvalidPolicy`].
    pub fn validate(&self) -> Result<(), ServiceError> {
        // `@` separates the stream key in the transport's names, so a name holding it could
        // spell another service's stream or events.
        let reserved = self.name.contains('@');
        if reserved || !TopicSpec::new(&self.name).has_valid_public_name() {
            return Err(ServiceError::InvalidName);
        }
        let bytes = self.name.trim_start_matches('/').len();
        if bytes > MAX_SERVICE_NAME_BYTES {
            return Err(ServiceError::NameTooLong { bytes, limit: MAX_SERVICE_NAME_BYTES });
        }
        if self.type_name.is_empty() || !hypha_wire::is_valid_md5(&self.md5) {
            return Err(ServiceError::InvalidIdentity);
        }
        self.policy.validate().map_err(ServiceError::InvalidPolicy)
    }
}

/// The node that holds a service's server slot, as a refused creation names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerHolder {
    /// The holder's node name, or empty when it cannot be read.
    pub node: String,
    /// The holder's process id, when it can be read.
    pub pid: Option<i32>,
    /// The type it serves and the first 8 hex digits of its MD5, when found.
    pub served: Option<(String, String)>,
}

impl fmt::Display for ServerHolder {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some((type_name, md5)) = &self.served {
            write!(out, "as {type_name} ({md5}) ")?;
        }
        if self.node.is_empty() {
            write!(out, "by a node whose name cannot be read")?;
        } else {
            write!(out, "by '{}'", self.node)?;
        }
        if let Some(pid) = self.pid {
            write!(out, " (pid {pid})")?;
        }
        Ok(())
    }
}

/// Why a server or a client was not created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceError {
    /// The name is not a valid public name.
    InvalidName,
    /// The name is longer than the transport can carry.
    NameTooLong {
        /// Its length, without the leading `/`.
        bytes: usize,
        /// [`MAX_SERVICE_NAME_BYTES`].
        limit: usize,
    },
    /// The type name is empty or the MD5 is not 32 lower-case hexadecimal digits.
    InvalidIdentity,
    /// The policy is out of bounds; the text names the field.
    InvalidPolicy(String),
    /// The name has a live server, of this version or another.
    AlreadyServed(ServerHolder),
    /// The service exists with another policy.
    QosMismatch,
    /// The process has too few file descriptors left for another service.
    OutOfDescriptors {
        /// Descriptors open now.
        open: u64,
        /// The soft limit.
        limit: u64,
    },
    /// The transport refused.
    Port(PortError),
}

impl core::error::Error for ServiceError {}

impl From<PortError> for ServiceError {
    fn from(cause: PortError) -> Self {
        Self::Port(cause)
    }
}

impl fmt::Display for ServiceError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName => write!(out, "the service name is not valid"),
            Self::NameTooLong { bytes, limit } => {
                write!(out, "the service name has {bytes} bytes; at most {limit} are allowed")
            }
            Self::InvalidIdentity => write!(out, "the service type or its MD5 is not valid"),
            Self::InvalidPolicy(why) => write!(out, "{why}"),
            Self::AlreadyServed(holder) => write!(out, "already served {holder}"),
            Self::QosMismatch => write!(out, "the service exists with another policy"),
            Self::OutOfDescriptors { open, limit } => write!(
                out,
                "out of file descriptors: {open} open of this process's limit of {limit}; raise \
                 the limit (ulimit -n)"
            ),
            Self::Port(cause) => write!(out, "{cause}"),
        }
    }
}

/// Why a client did not send a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallSendError {
    /// The client has its limit of calls in flight: `expired` of them timed out and are
    /// still held by the server.
    Busy {
        /// The client's limit.
        outstanding: usize,
        /// Calls that timed out and are still held by the server.
        expired: usize,
    },
    /// The request is above the policy's bound.
    TooLarge {
        /// Bytes of the request.
        requested: usize,
        /// The bound.
        limit: usize,
    },
    /// No server of the stream received it; nothing is pending.
    NoServer,
    /// The transport refused.
    Port(PortError),
}

impl core::error::Error for CallSendError {}

impl fmt::Display for CallSendError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy { outstanding, expired: 0 } => {
                write!(out, "the client has {outstanding} calls in flight, its limit")
            }
            Self::Busy { outstanding, expired } => write!(
                out,
                "the client has {outstanding} calls in flight, its limit; the server still holds \
                 {expired} expired calls of this client"
            ),
            Self::TooLarge { requested, limit } => {
                write!(out, "the request has {requested} bytes, above the bound of {limit}")
            }
            Self::NoServer => write!(out, "no server"),
            Self::Port(cause) => write!(out, "{cause}"),
        }
    }
}

/// Why a server did not answer a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyError {
    /// The handle names no request the server holds.
    UnknownRequest,
    /// The response is above the policy's bound; the client was refused with the sizes.
    TooLarge {
        /// Bytes of the response.
        requested: usize,
        /// The bound.
        limit: usize,
    },
    /// The client is gone; the request is dropped.
    Disconnected,
    /// The transport refused.
    Port(PortError),
}

impl core::error::Error for ReplyError {}

impl fmt::Display for ReplyError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownRequest => write!(out, "no such request"),
            Self::TooLarge { requested, limit } => {
                write!(out, "the response has {requested} bytes, above the bound of {limit}")
            }
            Self::Disconnected => write!(out, "the client is gone"),
            Self::Port(cause) => write!(out, "{cause}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_SERVICE_NAME_BYTES, ServiceError, ServicePolicy, ServiceSpec};

    const MD5: &str = "09fb03525b03e7ea1fd3992bafd87e16";

    #[test]
    fn a_name_of_219_bytes_is_accepted_and_one_of_220_is_refused_with_the_limit() {
        let at_limit = format!("/{}", "a".repeat(MAX_SERVICE_NAME_BYTES));
        assert_eq!(ServiceSpec::new(&at_limit, "std_srvs/SetBool", MD5).validate(), Ok(()));
        let over = format!("/{}", "a".repeat(MAX_SERVICE_NAME_BYTES + 1));
        assert_eq!(
            ServiceSpec::new(&over, "std_srvs/SetBool", MD5).validate(),
            Err(ServiceError::NameTooLong { bytes: 220, limit: 219 })
        );
        assert!(
            ServiceError::NameTooLong { bytes: 220, limit: 219 }.to_string().contains("219"),
            "the message names the limit"
        );
    }

    #[test]
    fn an_invalid_name_identity_or_policy_is_refused() {
        assert_eq!(
            ServiceSpec::new("no_slash", "std_srvs/SetBool", MD5).validate(),
            Err(ServiceError::InvalidName)
        );
        assert_eq!(ServiceSpec::new("/a#b", "std_srvs/SetBool", MD5).validate(), Ok(()));
        for reserved in ["/a@b", "/hypha/srv/a", "/hypha"] {
            assert_eq!(
                ServiceSpec::new(reserved, "std_srvs/SetBool", MD5).validate(),
                Err(ServiceError::InvalidName),
                "{reserved}"
            );
        }
        assert_eq!(
            ServiceSpec::new("/a", "std_srvs/SetBool", "XYZ").validate(),
            Err(ServiceError::InvalidIdentity)
        );
        let zero_timeout = ServicePolicy {
            timeout: core::time::Duration::ZERO,
            ..ServicePolicy::default_service()
        };
        assert!(matches!(
            ServiceSpec::new("/a", "std_srvs/SetBool", MD5).with_policy(zero_timeout).validate(),
            Err(ServiceError::InvalidPolicy(_))
        ));
    }

    #[test]
    fn the_default_policy_admits_three_observer_clients_more() {
        let policy = ServicePolicy::default_service();
        assert_eq!(policy.validate(), Ok(()));
        assert_eq!(policy.admitted_clients(), 11);
    }
}
