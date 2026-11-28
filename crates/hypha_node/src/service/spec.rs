//! What a server or a client is made of: the declaration code gives, resolved against the
//! node's ports tables (`docs/design/native-services.md` §8.6).
//!
//! A declaration names a port or a literal service. A port's table is
//! `[ports.servers.<port>]` or `[ports.clients.<port>]`, with `service` (required), for a
//! server `per_step`, and for a client `timeout_ms` and `outstanding`. A missing table is refused with the stanza to
//! paste, so the application exits 78.

use std::time::Duration;

use hypha_core::introspection::ConfigSource;
use hypha_core::parameters::ParamValue;
use hypha_core::port::{ServiceError, ServicePolicy, ServiceSpec};

use crate::config::HyphaConfig;
use crate::node::HyphaNode;
use crate::status::Error;

/// What code declares for a server or a client.
#[derive(Debug, Clone)]
pub struct ServiceDecl<'a> {
    /// The port, whose table names the service, or the service itself (`literal`).
    pub name: &'a str,
    /// True when `name` is the service: absolute, relative or `~/`.
    pub literal: bool,
    /// The `.srv` type, as `std_srvs/SetBool`.
    pub type_name: &'a str,
    /// The `.srv` MD5 over both halves.
    pub md5: &'a str,
    /// The definition of the request half with its dependencies; empty for a half of no
    /// fields, or for none (both halves empty, unless the MD5 is that of the empty text). A
    /// server registers both definitions so the tools can encode a request; a client's are
    /// unused.
    pub request_definition: &'a str,
    /// The definition of the response half; empty as the request's.
    pub response_definition: &'a str,
    /// The policy, or `None` for `defaultService`.
    pub policy: Option<ServicePolicy>,
    /// A client's timeout that code sets, or `None` for the policy's. The client's table
    /// (`timeout_ms`) overrides both.
    pub timeout: Option<Duration>,
}

/// Which end a declaration makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    Server,
    Client,
}

impl Role {
    const fn table(self) -> &'static str {
        match self {
            Self::Server => "servers",
            Self::Client => "clients",
        }
    }

    const fn noun(self) -> &'static str {
        match self {
            Self::Server => "server",
            Self::Client => "client",
        }
    }
}

/// A declaration resolved: the stream, a client's timeout and where it came from, and its
/// limit of calls in flight.
#[derive(Debug, Clone)]
pub(crate) struct Resolved {
    pub(crate) spec: ServiceSpec,
    pub(crate) timeout: Duration,
    /// Where the timeout came from, as `describe()` names it: `policy defaultService`.
    pub(crate) timeout_source: String,
    pub(crate) outstanding: Option<usize>,
    /// A server's `per_step`: the requests one cycle step takes.
    pub(crate) per_step: Option<usize>,
}

/// Resolve `decl` for `role` on `node`: the name from the port table or the literal, `~/`
/// and relative names against the node, the policy and a client's overrides.
///
/// # Errors
/// `InvalidArgument` for a missing or malformed table, an invalid identity or policy;
/// `InvalidTopic` for an invalid name. Each is logged and recorded for `last_error`.
pub(crate) fn resolve(
    node: &HyphaNode,
    decl: &ServiceDecl<'_>,
    role: Role,
) -> Result<Resolved, Error> {
    let table = PortTable { config: &node.parameters, node: Some(node), role, port: decl.name };
    let (written, overrides) = if decl.literal {
        (decl.name.to_owned(), Overrides::of(decl))
    } else {
        (table.service()?, table.overrides(decl)?)
    };
    let name = node.resolve_name(&written).map_err(|error| {
        crate::last_error::refuse(
            error,
            &format!("service '{written}' is not a valid service name"),
        )
    })?;
    let spec = ServiceSpec::new(&name, decl.type_name, decl.md5)
        .with_policy(overrides.policy.clone())
        .with_definitions(decl.request_definition, decl.response_definition);
    spec.validate().map_err(|cause| refuse_spec(&name, &cause))?;
    Ok(overrides.with(spec))
}

/// Resolve `decl` for `role` with no node, for an end a step test feeds: the table of the
/// port in `config`, when it has one, read and checked as [`resolve`] reads it. A missing
/// configuration or table leaves the defaults, as a step test enables a port without a
/// check of its table; the service keeps the name written, since no node resolves it.
///
/// # Errors
/// `InvalidArgument` for a malformed table, an invalid identity or policy.
pub(crate) fn resolve_fed(
    config: Option<&HyphaConfig>,
    decl: &ServiceDecl<'_>,
    role: Role,
) -> Result<Resolved, Error> {
    let table = config
        .filter(|_| !decl.literal)
        .map(|config| PortTable { config, node: None, role, port: decl.name })
        .filter(PortTable::exists);
    let (name, overrides) = match table {
        Some(table) => (table.service()?, table.overrides(decl)?),
        None => (decl.name.to_owned(), Overrides::of(decl)),
    };
    let spec = ServiceSpec::new(&name, decl.type_name, decl.md5)
        .with_policy(overrides.policy.clone())
        .with_definitions(decl.request_definition, decl.response_definition);
    overrides
        .policy
        .validate()
        .map_err(|cause| refuse_spec(&name, &ServiceError::InvalidPolicy(cause)))?;
    Ok(overrides.with(spec))
}

/// What a declaration and its table set besides the name.
struct Overrides {
    policy: ServicePolicy,
    timeout: Duration,
    timeout_source: String,
    outstanding: Option<usize>,
    per_step: Option<usize>,
}

impl Overrides {
    /// What code declares, before any table.
    fn of(decl: &ServiceDecl<'_>) -> Self {
        let policy = decl.policy.clone().unwrap_or_else(ServicePolicy::default_service);
        let timeout = decl.timeout.unwrap_or(policy.timeout);
        let timeout_source = if decl.timeout.is_some() {
            "set in code".to_owned()
        } else {
            format!("policy {}", policy.name)
        };
        Self { policy, timeout, timeout_source, outstanding: None, per_step: None }
    }

    fn with(self, spec: ServiceSpec) -> Resolved {
        let Self { timeout, timeout_source, outstanding, per_step, .. } = self;
        Resolved { spec, timeout, timeout_source, outstanding, per_step }
    }
}

/// The error a refused spec or creation stands for, logged with its message.
pub(crate) fn refuse_spec(name: &str, cause: &ServiceError) -> Error {
    let error = match cause {
        ServiceError::InvalidName | ServiceError::NameTooLong { .. } => Error::InvalidTopic,
        ServiceError::InvalidIdentity | ServiceError::InvalidPolicy(_) => Error::InvalidArgument,
        ServiceError::AlreadyServed(_) | ServiceError::OutOfDescriptors { .. } => {
            Error::ResourceLimit
        }
        ServiceError::QosMismatch => Error::QosMismatch,
        ServiceError::Port(port) => Error::from(*port),
    };
    crate::last_error::refuse(error, &format!("service '{name}': {cause}"))
}

/// The table of one port.
struct PortTable<'a> {
    config: &'a HyphaConfig,
    /// The node whose rows record what the table set; `None` for a step test's end.
    node: Option<&'a HyphaNode>,
    role: Role,
    port: &'a str,
}

impl PortTable<'_> {
    fn entry(&self) -> String {
        format!("ports.{}.{}", self.role.table(), self.port)
    }

    /// Whether the configuration has the table's `service` key.
    fn exists(&self) -> bool {
        self.config.value(&format!("{}.service", self.entry())).is_some()
    }

    /// A client's `timeout_ms` and `outstanding`, or a server's `per_step`, over `decl`.
    fn overrides(&self, decl: &ServiceDecl<'_>) -> Result<Overrides, Error> {
        let mut overrides = Overrides::of(decl);
        if self.role == Role::Client {
            if let Some(millis) = self.positive("timeout_ms")? {
                overrides.timeout = Duration::from_millis(millis);
                overrides.timeout_source = format!("timeout_ms of [ports.clients.{}]", decl.name);
            }
            overrides.outstanding = self
                .positive("outstanding")?
                .map(|count| usize::try_from(count).unwrap_or(usize::MAX));
        } else {
            overrides.per_step = self
                .positive("per_step")?
                .map(|count| usize::try_from(count).unwrap_or(usize::MAX));
        }
        Ok(overrides)
    }

    fn record(&self, key: String, value: String) {
        if let Some(node) = self.node {
            crate::param_topic::record_key_row(node, key, value, None, ConfigSource::Parameter);
        }
    }

    /// The `service` key, recorded as a configuration row.
    fn service(&self) -> Result<String, Error> {
        let entry = self.entry();
        let key = format!("{entry}.service");
        let service = match self.config.value(&key) {
            Some(ParamValue::Text(text)) if !text.is_empty() => text.clone(),
            Some(_) => {
                return Err(crate::last_error::refuse(
                    Error::InvalidArgument,
                    &format!("the parameter {key} must be a service name in text"),
                ));
            }
            None => {
                return Err(crate::last_error::refuse(
                    Error::InvalidArgument,
                    &format!(
                        "{} port '{}' has no table; add to the parameter file:\n[{entry}]\nservice = \"~/{}\"",
                        self.role.noun(),
                        self.port,
                        self.port
                    ),
                ));
            }
        };
        self.record(key, service.clone());
        Ok(service)
    }

    /// A positive whole number of the table, or `None` when it is not set.
    fn positive(&self, name: &str) -> Result<Option<u64>, Error> {
        let key = format!("{}.{name}", self.entry());
        match self.config.value(&key) {
            None => Ok(None),
            Some(ParamValue::Int(value)) if *value > 0 => {
                let value = value.unsigned_abs();
                self.record(key, value.to_string());
                Ok(Some(value))
            }
            Some(_) => Err(crate::last_error::refuse(
                Error::InvalidArgument,
                &format!("the parameter {key} must be a whole number above zero"),
            )),
        }
    }
}
