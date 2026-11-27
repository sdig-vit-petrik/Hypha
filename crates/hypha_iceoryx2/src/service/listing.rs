//! The service streams of a domain, for the tools (`hypha service list`).

use hypha_core::port::{PortError, ServicePolicy};
use iceoryx2::prelude::*;
use iceoryx2::service::static_config::messaging_pattern::MessagingPattern;

use super::key::public_name_of_stream;
use super::open::{MD5_ATTRIBUTE, POLICY_ATTRIBUTE, TYPE_ATTRIBUTE, attribute, parse_stamp};
use crate::discovery::DiscoveredNode;
use crate::header::Iox2FrameHeader;
use crate::transport::{Ipc, SharedNode};

/// One service stream, as the registry and the stream itself describe it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredServiceStream {
    /// The public service name.
    pub name: String,
    /// The `.srv` type.
    pub type_name: String,
    /// The `.srv` MD5.
    pub md5: String,
    /// The stamped policy, or `None` for a stamp this build does not read.
    pub policy: Option<ServicePolicy>,
    /// Servers now: 0 or 1. A crashed server counts until its node is reclaimed.
    pub servers: usize,
    /// Clients now, the caller's own excluded.
    pub clients: usize,
    /// The node of the server, alive or dead.
    pub server_node: Option<DiscoveredNode>,
}

struct Listed {
    name: String,
    public: String,
    type_name: String,
    md5: String,
    policy: Option<ServicePolicy>,
}

/// Every stream of the domain.
pub(super) fn list(node: &SharedNode) -> Result<Vec<DiscoveredServiceStream>, PortError> {
    let config = crate::domain::config()?;
    let mut listed = Vec::new();
    Ipc::list(config, |details| {
        let config = &details.static_details;
        let name = config.name().as_str();
        let is_request_response = matches!(config.messaging_pattern(), MessagingPattern::RequestResponse(_));
        if let (true, Some((public, _))) = (is_request_response, public_name_of_stream(name)) {
            let attributes = config.attributes();
            listed.push(Listed {
                name: name.to_owned(),
                public,
                type_name: attribute(attributes, TYPE_ATTRIBUTE).unwrap_or_default(),
                md5: attribute(attributes, MD5_ATTRIBUTE).unwrap_or_default(),
                policy: attribute(attributes, POLICY_ATTRIBUTE).as_deref().and_then(parse_stamp),
            });
        }
        CallbackProgression::Continue
    })
    .map_err(|cause| {
        log::error!(target: "hypha/transport", "hypha_iceoryx2: cannot list the services of the domain: {cause:?}");
        PortError::Backend(5)
    })?;
    let mut streams = Vec::with_capacity(listed.len());
    for entry in listed {
        if let Some(stream) = describe(node, entry)? {
            streams.push(stream);
        }
    }
    streams.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.md5.cmp(&b.md5)));
    Ok(streams)
}

/// The counts of one stream; `None` when it went away since the registry scan.
fn describe(
    node: &SharedNode,
    entry: Listed,
) -> Result<Option<DiscoveredServiceStream>, PortError> {
    let Ok(service_name) = ServiceName::new(&entry.name) else {
        return Ok(None);
    };
    let opened = {
        let locked_node = crate::locked(node);
        let node_ref = locked_node.as_ref().ok_or(PortError::Backend(1))?;
        let own = *node_ref.id();
        // Open only: no setting is required, so any stream of this layout opens.
        node_ref
            .service_builder(&service_name)
            .request_response::<[u8], [u8]>()
            .request_user_header::<Iox2FrameHeader>()
            .response_user_header::<Iox2FrameHeader>()
            .open()
            .ok()
            .map(|service| (service, own))
    };
    let Some((service, own)) = opened else {
        return Ok(None);
    };
    let dynamic = service.dynamic_config();
    let mut server_node_id = None;
    dynamic.list_servers(|server| {
        server_node_id = Some(server.node_id);
        CallbackProgression::Stop
    });
    let mut clients: usize = 0;
    dynamic.list_clients(|client| {
        if client.node_id != own {
            clients = clients.saturating_add(1);
        }
        CallbackProgression::Continue
    });
    let server_node = match server_node_id.map(|id| super::open::peer_node(&service, id)) {
        Some(Ok(super::open::PeerNode::Alive { name, pid })) => Some(DiscoveredNode {
            name,
            executable: String::new(),
            pid: pid.unwrap_or(0),
            is_alive: true,
        }),
        Some(Ok(super::open::PeerNode::Dead(view))) => {
            crate::discovery::discovered_node(&*view, false)
        }
        _ => None,
    };
    Ok(Some(DiscoveredServiceStream {
        name: entry.public,
        type_name: entry.type_name,
        md5: entry.md5,
        policy: entry.policy,
        servers: usize::from(server_node_id.is_some()),
        clients,
        server_node,
    }))
}
