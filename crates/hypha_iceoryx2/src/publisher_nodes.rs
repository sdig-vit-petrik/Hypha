//! Which nodes publish on a subscriber's topics, read from the iceoryx2 registry.
//!
//! Only alive nodes count. The ids can be read under a caller's lock and resolved
//! to names after it is released.

use std::collections::BTreeMap;

use hypha_core::port::PortError;
use iceoryx2::node::{Node, NodeState, NodeView as _};
use iceoryx2::prelude::{CallbackProgression, PortFactory as _};

use crate::subscriber::Iox2Subscriber;
use crate::transport::Ipc;

impl Iox2Subscriber {
    /// Append raw ids of nodes holding publisher ports to `ids`, unsorted. Reads no
    /// node registry.
    pub fn publisher_node_ids(&self, ids: &mut Vec<u128>) {
        for factory in self.port_factories() {
            factory.dynamic_config().list_publishers(|publisher| {
                ids.push(publisher.node_id.value());
                CallbackProgression::Continue
            });
        }
    }
}

/// The name of each alive, named node among `ids`, by node id.
///
/// # Errors
/// [`PortError::Backend`] with code 5 when the node registry cannot be read.
pub fn node_names(ids: &[u128]) -> Result<BTreeMap<u128, String>, PortError> {
    let mut names = BTreeMap::new();
    if ids.is_empty() {
        return Ok(names);
    }
    let config = crate::domain::config()?;
    Node::<Ipc>::list(config, |view| {
        let NodeState::Alive(alive) = &view else {
            return CallbackProgression::Continue;
        };
        let id = alive.id().value();
        if !ids.contains(&id) {
            return CallbackProgression::Continue;
        }
        if let Some(details) = alive.details().as_ref() {
            names.insert(id, details.name().as_str().to_string());
        }
        CallbackProgression::Continue
    })
    .map_err(|_| PortError::Backend(5))?;
    Ok(names)
}
