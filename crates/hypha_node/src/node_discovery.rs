//! Discovery of nodes and services, and the topic name check.

use std::ffi::{CStr, CString};

use crate::node::HyphaNode;
use crate::status::Error;

/// One process the bus registry knows, as `list_nodes` reports it.
#[derive(Debug, Clone, Copy)]
pub struct ListedNode<'a> {
    /// The node's registered name.
    pub name: &'a CStr,
    /// The node's executable name or path, empty when unknown.
    pub executable: &'a CStr,
    /// Process ID.
    pub pid: i32,
    /// Whether the process is alive.
    pub alive: bool,
}

/// A watch of the domain's graph: it tells a reader on its own thread that an endpoint came
/// or went since it last asked. See [`graph_watch`].
pub struct GraphWatch(hypha_iceoryx2::GraphWatch);

impl GraphWatch {
    /// Whether an endpoint of the domain came or went since the last call: one
    /// non-blocking read that takes every pending notification.
    ///
    /// # Errors
    /// [`Error::Disconnected`] when the notification service cannot be read.
    pub fn changed(&self) -> Result<bool, Error> {
        self.0.changed().map_err(Error::from)
    }
}

impl GraphWatch {
    /// Waits up to `timeout` for an endpoint of the domain to come or go, and takes the
    /// notifications; true when one came. The calling thread sleeps meanwhile.
    ///
    /// # Errors
    /// [`Error::Disconnected`] when the notification service cannot be read.
    pub fn wait(&self, timeout: std::time::Duration) -> Result<bool, Error> {
        self.0.wait(timeout).map_err(Error::from)
    }
}

/// Open a watch of the domain's graph event, the event a node's own housekeeping hears.
/// A reader that lists the streams once, opens this watch before it lists, and lists again
/// whenever the watch says the graph changed, misses no stream.
///
/// # Errors
/// The transport's error when the event service cannot be opened.
pub fn graph_watch(node: &HyphaNode) -> Result<GraphWatch, Error> {
    node.transport.graph_watch().map(GraphWatch).map_err(Error::from)
}

/// One stream, a topic and a schema, as `list_service_ports` reports it.
#[derive(Debug, Clone, Copy)]
pub struct ListedServicePort<'a> {
    /// The topic name.
    pub name: &'a CStr,
    /// The schema hash of the stream's frames: the first eight bytes of the MD5, 0 for
    /// the stream of no schema.
    pub schema_hash: u64,
    /// Active publisher count.
    pub publishers: usize,
    /// Active subscriber count.
    pub subscribers: usize,
}

/// Report every node registered on the bus to `emit`, in registry order. A node whose
/// name or executable holds a NUL byte is left out.
///
/// # Errors
/// The transport's error when the registry cannot be read; `emit` then ran for none.
pub fn list_nodes(emit: &mut dyn FnMut(&ListedNode<'_>)) -> Result<(), Error> {
    let nodes = hypha_iceoryx2::list_nodes().map_err(Error::from)?;
    for node in nodes {
        let Ok(name) = CString::new(node.name) else {
            continue;
        };
        let Ok(executable) = CString::new(node.executable) else {
            continue;
        };
        emit(&ListedNode {
            name: &name,
            executable: &executable,
            pid: node.pid,
            alive: node.is_alive,
        });
    }
    Ok(())
}

/// Report every stream of the domain and its active endpoint counts to `emit`: a topic of
/// two schemas is reported twice, once with each schema hash.
///
/// # Errors
/// [`Error::Backend`] when the services cannot be read. [`Error::InvalidFrame`] after
/// the listing when a service name could not be reported.
pub fn list_service_ports(
    node: &HyphaNode,
    emit: &mut dyn FnMut(&ListedServicePort<'_>),
) -> Result<(), Error> {
    let services = node.transport.list_service_ports().map_err(|_| Error::Backend)?;
    let mut rejected = false;
    for service in services {
        let Ok(name) = CString::new(service.name) else {
            rejected = true;
            continue;
        };
        emit(&ListedServicePort {
            name: &name,
            schema_hash: service.schema_hash,
            publishers: service.publishers,
            subscribers: service.subscribers,
        });
    }
    if rejected {
        return Err(Error::InvalidFrame);
    }
    Ok(())
}

/// Node information reported by `hypha_list_nodes`.
#[repr(C)]
pub struct HyphaNodeInfo {
    /// The node's registered name.
    pub name: *const std::os::raw::c_char,
    /// The node's executable name/path.
    pub executable: *const std::os::raw::c_char,
    /// Process ID.
    pub pid: i32,
    /// Whether the process is alive (1) or dead (0).
    pub alive: i32,
}

/// Called once per node registered on the bus, by `hypha_list_nodes`.
pub type HyphaNodeCallback =
    Option<extern "C" fn(info: *const HyphaNodeInfo, user_data: *mut std::os::raw::c_void)>;

/// Endpoint counts for one stream: one topic and one schema.
#[repr(C)]
pub struct HyphaServicePortInfo {
    /// The topic name, NUL-terminated, borrowed for the callback only.
    pub name: *const std::os::raw::c_char,
    /// The schema hash of the stream's frames: the first eight bytes of the MD5, as
    /// `HyphaSampleIdentity.schema_hash`; 0 for the stream of no schema.
    pub schema_hash: u64,
    /// Active publisher count.
    pub publishers: usize,
    /// Active subscriber count.
    pub subscribers: usize,
}

/// Callback used by `hypha_list_service_ports`.
pub type HyphaServicePortCallback =
    Option<extern "C" fn(info: *const HyphaServicePortInfo, user_data: *mut std::os::raw::c_void)>;
