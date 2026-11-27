//! What one stream of a topic has now, read without keeping anything open: the probe of
//! the streams that a node's housekeeping runs for the topics it subscribes to.

use hypha_core::port::PortError;
use iceoryx2::identifiers::UniqueNodeId;

use crate::stream::Stream;
use crate::transport::Iox2Transport;

impl Iox2Transport {
    /// The publishers of the stream of `schema_hash` on `topic` whose node is alive: one
    /// open attempt, with no retry and no wait, for the nodes of the publisher ports, and
    /// the states of the nodes registered at that one service (`Service::details`), so the
    /// cost does not grow with the nodes of the domain. Nothing stays open. `Ok(None)` when
    /// the stream does not exist or is being created or removed. The record of a stream that
    /// does not exist is removed: its endpoints died and a node start reclaimed their ports.
    ///
    /// The open takes one node slot of the stream while it lasts. The service keeps
    /// [`crate::qos::PROBE_NODE_SLOTS`] beyond its endpoints for it, so it succeeds on a
    /// stream whose endpoints fill their slots, and no endpoint is refused for it.
    ///
    /// # Errors
    /// [`PortError`] when the session is gone, the open fails for another reason, or the
    /// service's node states cannot be read.
    pub fn live_publishers(
        &self,
        topic: &str,
        schema_hash: u64,
    ) -> Result<Option<usize>, PortError> {
        use iceoryx2::node::{NodeState, NodeView as _};
        use iceoryx2::prelude::CallbackProgression;
        use iceoryx2::service::Service as _;
        use iceoryx2::service::builder::publish_subscribe::PublishSubscribeOpenError as Open;
        use iceoryx2::service::port_factory::PortFactory as _;

        let stream = Stream { topic: topic.to_owned(), schema_hash };
        let name = iceoryx2::prelude::ServiceName::new(&stream.service_name())
            .map_err(|_| PortError::InvalidTopic)?;
        let owners = {
            let locked_node = crate::locked(&self.node);
            let node_ref = locked_node.as_ref().ok_or(PortError::Backend(1))?;
            match crate::transport::data_service_builder(node_ref, &name).open() {
                Ok(service) => {
                    let mut owners: Vec<UniqueNodeId> = Vec::new();
                    service.dynamic_config().list_publishers(|details| {
                        owners.push(details.node_id);
                        CallbackProgression::Continue
                    });
                    owners
                }
                Err(Open::DoesNotExist) => {
                    if let Err(cause) = crate::schema::remove_orphan_record(&stream) {
                        log::warn!(target: "hypha/schema",
                            "hypha: the record of a gone stream of topic '{topic}' stays: {cause}");
                    }
                    return Ok(None);
                }
                Err(cause) if crate::service_open::publish_subscribe_open_is_transient(cause) => {
                    return Ok(None);
                }
                Err(cause) => {
                    return Err(crate::service_open::publish_subscribe_open_port_error(cause));
                }
            }
        };
        if owners.is_empty() {
            return Ok(Some(0));
        }
        let config = crate::domain::config()?;
        let details = crate::transport::Ipc::details(
            &name,
            config,
            iceoryx2::prelude::MessagingPattern::PublishSubscribe,
        )
        .map_err(|_| PortError::Backend(5))?;
        let Some(nodes) = details.and_then(|details| details.dynamic_details) else {
            return Ok(None);
        };
        let dead: Vec<UniqueNodeId> = nodes
            .nodes
            .iter()
            .filter_map(|state| match state {
                NodeState::Dead(view) => Some(*view.id()),
                _ => None,
            })
            .collect();
        Ok(Some(owners.iter().filter(|owner| !dead.contains(owner)).count()))
    }
}
