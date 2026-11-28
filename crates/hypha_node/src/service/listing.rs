//! The services of the domain and their registered definitions, for the tools
//! (`hypha service list|info|call`).

use std::time::Duration;

use hypha_core::port::ServicePolicy;

use crate::node::HyphaNode;
use crate::status::Error;

/// How long a read of a stream's definitions waits for a server that is writing them.
const SCHEMA_WAIT: Duration = Duration::from_millis(200);

/// One service stream of the domain: a name and a `.srv` MD5.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedService {
    /// The resolved service name.
    pub name: String,
    /// The `.srv` type.
    pub type_name: String,
    /// The `.srv` MD5.
    pub md5: String,
    /// The policy the stream was created with; `None` for a stamp this build does not read.
    pub policy: Option<ServicePolicy>,
    /// The server's node name, pid and whether it is alive; `None` without a server. A
    /// crashed server is listed, dead, until a node start reclaims it.
    pub server: Option<(String, i32, bool)>,
    /// Clients now, the caller's own excluded.
    pub clients: usize,
}

/// The definitions a server registered for its stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceDefinitions {
    /// The request type, `std_srvs/SetBoolRequest`.
    pub request_type: String,
    /// The request definition with its dependencies.
    pub request_definition: String,
    /// The response type, `std_srvs/SetBoolResponse`.
    pub response_type: String,
    /// The response definition with its dependencies.
    pub response_definition: String,
}

/// Every service stream of the domain, by name and MD5.
///
/// # Errors
/// `Backend` when the registry cannot be read.
pub fn list_services(node: &HyphaNode) -> Result<Vec<ListedService>, Error> {
    let streams = node.transport.list_services().map_err(Error::from)?;
    Ok(streams
        .into_iter()
        .map(|stream| ListedService {
            server: stream
                .server_node
                .map(|server| (server.name, server.pid, server.is_alive))
                .or_else(|| (stream.servers > 0).then(|| (String::new(), 0, true))),
            name: stream.name,
            type_name: stream.type_name,
            md5: stream.md5,
            policy: stream.policy,
            clients: stream.clients,
        })
        .collect())
}

/// The definitions the server of `name`'s stream of `md5` registered, or `None` when it
/// registered none (no server, or a server that gave none).
///
/// # Errors
/// `InvalidTopic` for a malformed MD5; the transport's error otherwise.
pub fn service_definitions(
    node: &HyphaNode,
    name: &str,
    md5: &str,
) -> Result<Option<ServiceDefinitions>, Error> {
    let schema = node.transport.service_schema(name, md5, SCHEMA_WAIT).map_err(Error::from)?;
    Ok(schema.map(|schema| ServiceDefinitions {
        request_type: schema.request_type,
        request_definition: schema.request_definition,
        response_type: schema.response_type,
        response_definition: schema.response_definition,
    }))
}
