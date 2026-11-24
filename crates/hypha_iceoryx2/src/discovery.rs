//! Discovery and introspection for nodes and services.

use hypha_core::port::PortError;
use iceoryx2::node::{Node, NodeView};
use iceoryx2::prelude::*;
use iceoryx2::service::static_config::messaging_pattern::MessagingPattern as StaticPattern;

use crate::header::FRAME_HEADER_TYPE_NAME;
use crate::transport::{Ipc, SharedNode};

/// True for a data topic: a publish-subscribe service whose user header is the
/// hypha frame header.
fn is_topic(pattern: &StaticPattern) -> bool {
    match pattern {
        StaticPattern::PublishSubscribe(config) => {
            *config.message_type_details().user_header.type_name() == FRAME_HEADER_TYPE_NAME
        }
        _ => false,
    }
}

/// The bus names of every data topic.
pub(crate) fn topic_bus_names() -> Result<Vec<String>, PortError> {
    let config = crate::domain::config()?;
    let mut names = Vec::new();
    Ipc::list(config, |details| {
        if is_topic(details.static_details.messaging_pattern()) {
            names.push(details.static_details.name().to_string());
        }
        CallbackProgression::Continue
    })
    .map_err(|cause| {
        log::error!(target: "hypha/transport", "hypha_iceoryx2: cannot list the services of the domain: {cause:?}");
        PortError::Backend(5)
    })?;
    Ok(names)
}

/// Discovered node metadata from iceoryx2 registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredNode {
    /// Node registration name.
    pub name: String,
    /// Executable name if known.
    pub executable: String,
    /// Process ID.
    pub pid: i32,
    /// Active / alive status.
    pub is_alive: bool,
}

/// Endpoint counts for one data service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredService {
    /// Public topic service name.
    pub name: String,
    /// Number of active publisher ports.
    pub publishers: usize,
    /// Number of active subscriber ports.
    pub subscribers: usize,
}

/// Lists all active nodes registered in the iceoryx2 registry.
///
/// # Errors
///
/// Returns [`PortError::Backend`] when the transport cannot inspect the
/// iceoryx2 node registry, or cannot read the process id of a listed node.
pub fn list_nodes() -> Result<Vec<DiscoveredNode>, PortError> {
    let mut out = Vec::new();
    let mut pid_unreadable = false;
    let config = crate::domain::config()?;
    Node::<Ipc>::list(config, |view| {
        let node = match &view {
            iceoryx2::node::NodeState::Alive(node_view) => discovered_node(node_view, true),
            iceoryx2::node::NodeState::Dead(node_view) => discovered_node(node_view, false),
            _ => return CallbackProgression::Continue,
        };
        // A node left out would make the list silently wrong.
        let Some(node) = node else {
            pid_unreadable = true;
            return CallbackProgression::Stop;
        };
        out.push(node);
        CallbackProgression::Continue
    })
    .map_err(crate::transport::node_list_failed)?;
    if pid_unreadable {
        log::error!(target: "hypha/transport", "hypha_iceoryx2: cannot read the process id of a listed node");
        return Err(PortError::Backend(5));
    }
    Ok(out)
}

/// One registry entry, or `None` when its process id cannot be read.
fn discovered_node(view: &impl NodeView, is_alive: bool) -> Option<DiscoveredNode> {
    let pid = view.id().pid::<Ipc>().ok()?.value();
    let name = view.details().as_ref().map_or_default(|d| d.name().as_str().to_string());
    let executable = view
        .details()
        .as_ref()
        .map_or_default(|d| String::from_utf8_lossy(d.executable().as_bytes()).into_owned());
    Some(DiscoveredNode { name, executable, pid, is_alive })
}

/// Lists data topics and their active endpoint counts, through the caller's node.
/// # Errors
/// Returns a backend error when the service registry cannot be read.
pub fn list_service_ports(node: &SharedNode) -> Result<Vec<DiscoveredService>, PortError> {
    let names = topic_bus_names()?;
    let mut services = Vec::with_capacity(names.len());
    for name in names {
        let Some(counted) = count_service_ports(node, &name)? else {
            continue;
        };
        services.push(counted);
    }
    Ok(services)
}

/// Counts the endpoints of one service, excluding the calling node's own ports.
///
/// `Ok(None)` when the service disappeared since the registry scan.
fn count_service_ports(
    node: &SharedNode,
    name: &str,
) -> Result<Option<DiscoveredService>, PortError> {
    let Ok(service_name) = name.try_into() else {
        return Ok(None);
    };
    // The node lock covers the open only.
    let opened = {
        let locked_node = node.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let node_ref = locked_node.as_ref().ok_or(PortError::Backend(1))?;
        let node_id = *node_ref.id();
        let service = crate::transport::data_service_builder(node_ref, &service_name).open();
        service.ok().map(|service| (service, node_id))
    };
    let Some((service, node_id)) = opened else {
        return Ok(None);
    };

    let dynamic = service.dynamic_config();
    let mut publishers: usize = 0;
    dynamic.list_publishers(|publisher| {
        if publisher.node_id != node_id {
            publishers = publishers.saturating_add(1);
        }
        CallbackProgression::Continue
    });
    let mut subscribers: usize = 0;
    dynamic.list_subscribers(|subscriber| {
        if subscriber.node_id != node_id {
            subscribers = subscribers.saturating_add(1);
        }
        CallbackProgression::Continue
    });
    Ok(Some(DiscoveredService { name: public_topic_name(name), publishers, subscribers }))
}

/// Converts a data-service name into its public topic name.
pub(crate) fn public_topic_name(service_name: &str) -> String {
    if service_name.starts_with('/') {
        return service_name.to_owned();
    }
    format!("/{service_name}")
}
