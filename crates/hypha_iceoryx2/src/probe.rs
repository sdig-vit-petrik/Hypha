//! What one stream of a topic has now, read without keeping anything open: the probe of
//! the other streams that a node's housekeeping runs for the topics it subscribes to.

use hypha_core::port::PortError;
use iceoryx2::identifiers::UniqueNodeId;

use crate::stream::Stream;
use crate::transport::Iox2Transport;

/// The nodes of the domain that iceoryx2 reports dead, read once for a whole probe.
#[derive(Debug)]
pub struct DeadNodes(Vec<UniqueNodeId>);

impl Iox2Transport {
    /// The dead nodes of the domain now: one read of the node registry.
    ///
    /// # Errors
    /// [`PortError::Backend`] when the registry cannot be read.
    pub fn dead_nodes(&self) -> Result<DeadNodes, PortError> {
        crate::endpoint::dead_node_ids().map(DeadNodes)
    }

    /// The nodes of the publisher ports of the stream of `schema_hash` on `topic`: one open
    /// attempt, no retry and no wait, and the service handle is dropped before it returns.
    /// `Ok(None)` when the stream does not exist or is being created or removed; a later
    /// probe tries again.
    ///
    /// # Errors
    /// [`PortError`] when the session is gone or the open fails for another reason.
    pub fn publisher_ports(
        &self,
        topic: &str,
        schema_hash: u64,
    ) -> Result<Option<PublisherPorts>, PortError> {
        use iceoryx2::prelude::CallbackProgression;
        use iceoryx2::service::builder::publish_subscribe::PublishSubscribeOpenError as Open;
        use iceoryx2::service::port_factory::PortFactory as _;
        let stream = Stream { topic: topic.to_owned(), schema_hash };
        let name = iceoryx2::prelude::ServiceName::new(&stream.service_name())
            .map_err(|_| PortError::InvalidTopic)?;
        let locked_node = crate::locked(&self.node);
        let node_ref = locked_node.as_ref().ok_or(PortError::Backend(1))?;
        match crate::transport::data_service_builder(node_ref, &name).open() {
            Ok(service) => {
                let mut owners = Vec::new();
                service.dynamic_config().list_publishers(|details| {
                    owners.push(details.node_id);
                    CallbackProgression::Continue
                });
                Ok(Some(PublisherPorts(owners)))
            }
            Err(Open::DoesNotExist) => Ok(None),
            Err(cause) if crate::service_open::publish_subscribe_open_is_transient(cause) => {
                Ok(None)
            }
            Err(cause) => Err(crate::service_open::publish_subscribe_open_port_error(cause)),
        }
    }
}

/// The nodes of the publisher ports of one stream, as a probe read them.
#[derive(Debug)]
pub struct PublisherPorts(Vec<UniqueNodeId>);

impl PublisherPorts {
    /// True when the stream has no publisher port at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The ports whose node is not one of `dead`.
    #[must_use]
    pub fn live(&self, dead: &DeadNodes) -> usize {
        self.0.iter().filter(|owner| !dead.0.contains(owner)).count()
    }
}
