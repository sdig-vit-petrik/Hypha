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

/// One data service, as `list_service_ports` reports it.
#[derive(Debug, Clone, Copy)]
pub struct ListedServicePort<'a> {
    /// Topic service name.
    pub name: &'a CStr,
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

/// Report every data service of the domain and its active endpoint counts to `emit`.
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

/// Endpoint counts for one data service.
#[repr(C)]
pub struct HyphaServicePortInfo {
    /// Topic service name.
    pub name: *const std::os::raw::c_char,
    /// Active publisher count.
    pub publishers: usize,
    /// Active subscriber count.
    pub subscribers: usize,
}

/// Callback used by `hypha_list_service_ports`.
pub type HyphaServicePortCallback =
    Option<extern "C" fn(info: *const HyphaServicePortInfo, user_data: *mut std::os::raw::c_void)>;
