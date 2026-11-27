//! Discovery and introspection for nodes and services.

use hypha_core::port::PortError;
use iceoryx2::node::{Node, NodeView};
use iceoryx2::prelude::*;
use iceoryx2::service::static_config::messaging_pattern::MessagingPattern as StaticPattern;

use crate::header::FRAME_HEADER_TYPE_NAME;
use crate::stream::Stream;
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

/// Every stream of the domain, with the name of its data service. A data service with the
/// hypha frame header whose name is no stream's was made by a hypha build before the
/// streams: it is left out, and the first listing of the process that meets one warns.
pub(crate) fn topic_streams() -> Result<Vec<(String, Stream)>, PortError> {
    let config = crate::domain::config()?;
    let mut streams = Vec::new();
    let mut foreign = 0_usize;
    Ipc::list(config, |details| {
        if is_topic(details.static_details.messaging_pattern()) {
            let name = details.static_details.name().to_string();
            match Stream::of_service(&name) {
                Some(stream) => streams.push((name, stream)),
                None => foreign = foreign.saturating_add(1),
            }
        }
        CallbackProgression::Continue
    })
    .map_err(|cause| {
        log::error!(target: "hypha/transport", "hypha_iceoryx2: cannot list the services of the domain: {cause:?}");
        PortError::Backend(5)
    })?;
    warn_of_another_build(foreign);
    Ok(streams)
}

/// Warns once per process that `foreign` topics of another hypha build are in the domain.
fn warn_of_another_build(foreign: usize) {
    static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if foreign == 0 || WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    log::warn!(target: "hypha/transport",
        "hypha: {foreign} topics of another hypha build are in this domain: they are not visible \
         to this build; run one hypha build per domain");
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

/// Endpoint counts for one stream: one topic and one schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredService {
    /// The public topic name.
    pub name: String,
    /// The schema hash of the stream's frames; 0 for the stream of no schema.
    pub schema_hash: u64,
    /// Number of publisher ports whose node is alive.
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

/// Lists the streams of the domain and their active endpoint counts, through the caller's
/// node: a topic of two schemas is two entries.
/// # Errors
/// Returns a backend error when the service registry cannot be read.
pub fn list_service_ports(node: &SharedNode) -> Result<Vec<DiscoveredService>, PortError> {
    let streams = topic_streams()?;
    let dead = crate::endpoint::dead_node_ids()?;
    let mut services = Vec::with_capacity(streams.len());
    for (name, stream) in streams {
        let Some(counted) = count_service_ports(node, &name, stream, &dead)? else {
            continue;
        };
        services.push(counted);
    }
    Ok(services)
}

/// Counts the endpoints of the data service `name` of `stream`, excluding the calling
/// node's own ports and the publishers of the `dead` nodes.
///
/// `Ok(None)` when the service disappeared since the registry scan.
fn count_service_ports(
    node: &SharedNode,
    name: &str,
    stream: Stream,
    dead: &[iceoryx2::identifiers::UniqueNodeId],
) -> Result<Option<DiscoveredService>, PortError> {
    let Ok(service_name) = name.try_into() else {
        return Ok(None);
    };
    // The node lock covers the open only.
    let opened = {
        let locked_node = crate::locked(node);
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
        if publisher.node_id != node_id && !dead.contains(&publisher.node_id) {
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
    Ok(Some(DiscoveredService {
        name: stream.topic,
        schema_hash: stream.schema_hash,
        publishers,
        subscribers,
    }))
}
