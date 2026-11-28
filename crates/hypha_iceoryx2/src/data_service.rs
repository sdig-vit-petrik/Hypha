//! A topic's data service, opened or created under its policy stamp, and the dead nodes a
//! starting node reclaims.

use std::time::Duration as StdDuration;

use hypha_core::port::{PortError, QosPolicy, QosRequest};
use iceoryx2::prelude::*;

use crate::domain::MAX_BORROWED_SAMPLES;
use crate::header::Iox2FrameHeader;
use crate::qos;
use crate::transport::{DataPortFactory, Ipc, SharedNode};

pub(crate) struct DataService {
    pub(crate) service: DataPortFactory,
    pub(crate) policy: QosPolicy,
}

/// How long a starting node waits for another process reclaiming the same dead node.
pub(crate) const RECLAIM_TIMEOUT: StdDuration = StdDuration::from_secs(1);

/// Remove the resources of the dead nodes a node called `node_name` reclaims
/// when it starts: its own crashed runs and every dead CLI tool node
/// (`_hyphacli_<tool>`). A dead CLI tool node that cannot be reclaimed is logged and the
/// node still starts.
///
/// # Errors
/// [`PortError::Backend`] with [`BACKEND_RECLAIM_FAILED`] when the node's own dead run
/// cannot be reclaimed, or with code 5 when the node registry cannot be read.
pub(crate) fn reclaim_dead_nodes(
    config: &iceoryx2::config::Config,
    node_name: &NodeName,
) -> Result<(), PortError> {
    use iceoryx2::node::{DeadNodeView, NodeState, NodeView as _};
    use iceoryx2::prelude::CallbackProgression;

    let mut dead_nodes: Vec<(DeadNodeView<Ipc>, String, bool)> = Vec::new();
    iceoryx2::node::Node::<Ipc>::list(config, |state| {
        let NodeState::Dead(view) = state else {
            return CallbackProgression::Continue;
        };
        let Some(dead_name) =
            view.details().as_ref().map(|details| details.name().as_str().to_owned())
        else {
            return CallbackProgression::Continue;
        };
        let own_run = dead_name == node_name.as_str();
        let reclaimable = own_run || hypha_core::port::is_cli_node_name(&dead_name);
        if reclaimable {
            dead_nodes.push((view, dead_name, own_run));
        }
        CallbackProgression::Continue
    })
    .map_err(node_list_failed)?;
    for (dead_node, dead_name, own_run) in dead_nodes {
        let pid = match dead_node.id().pid::<Ipc>() {
            Ok(pid) => pid.value().to_string(),
            Err(_) => "unknown".to_string(),
        };
        let dead_node_id = *dead_node.id();
        match dead_node.blocking_remove_stale_resources(RECLAIM_TIMEOUT) {
            Ok(()) => {
                log::info!(target: "hypha/transport", "hypha: node '{}' reclaimed the dead node '{dead_name}' of pid {pid}",
                        node_name.as_str());
            }
            Err(cause) => {
                let still_dead = is_still_dead(config, &dead_node_id)?;
                if !still_dead {
                    log::info!(target: "hypha/transport", "hypha: node '{}' finds the dead node '{dead_name}' of pid {pid} \
                             already reclaimed by another process", node_name.as_str());
                    continue;
                }
                if own_run {
                    log::error!(target: "hypha/transport", "hypha: node '{}' cannot reclaim its own dead run, of pid {pid}: \
                             {cause:?}; the run still holds the name's ports, so the node does not start. Run \
                             `hypha clean` with the domain stopped", node_name.as_str());
                    return Err(PortError::Backend(BACKEND_RECLAIM_FAILED));
                }
                log::error!(target: "hypha/transport", "hypha: node '{}' cannot reclaim the dead node '{dead_name}' of pid \
                         {pid}: {cause:?}; it stays listed as dead and holds its ports until `hypha clean` \
                         removes the stopped domain", node_name.as_str());
            }
        }
    }
    Ok(())
}

/// Log a failed listing of the domain's nodes, and return its error: code 5.
pub(crate) fn node_list_failed(cause: impl core::fmt::Debug) -> PortError {
    log::error!(target: "hypha/transport", "hypha_iceoryx2: cannot list the nodes of the domain: {cause:?}");
    PortError::Backend(5)
}

/// Backend code of a node whose own dead run cannot be reclaimed, so it does not start.
pub(crate) const BACKEND_RECLAIM_FAILED: u32 = 41;

/// True while the registry lists the node `dead_node_id` as dead.
fn is_still_dead(
    config: &iceoryx2::config::Config,
    dead_node_id: &iceoryx2::identifiers::UniqueNodeId,
) -> Result<bool, PortError> {
    use iceoryx2::node::{NodeState, NodeView as _};
    use iceoryx2::prelude::CallbackProgression;

    let mut still_dead = false;
    iceoryx2::node::Node::<Ipc>::list(config, |state| {
        if let NodeState::Dead(view) = &state
            && view.id() == dead_node_id
        {
            still_dead = true;
        }
        CallbackProgression::Continue
    })
    .map_err(node_list_failed)?;
    Ok(still_dead)
}

/// Open the data service `service_name` of `topic`, or create it when `may_create`.
///
///
/// An endpoint that declares a policy must find it in the service's stamp, or is
/// refused; one that declares nothing adopts it. Only a publisher, or a subscriber
/// that declares a policy or reads a framework topic, creates it.
pub(crate) fn open_data_service(
    node: &SharedNode,
    service_name: &ServiceName,
    topic: &str,
    request: &QosRequest,
    may_create: bool,
) -> Result<DataService, PortError> {
    use iceoryx2::service::builder::publish_subscribe::{
        PublishSubscribeOpenError as Open, PublishSubscribeOpenOrCreateError as OpenOrCreate,
    };
    use iceoryx2::service::port_factory::PortFactory as _;

    let creation = qos::creation_policy(topic, request);
    let stamp = qos::stamp_specifier(&creation)?;
    let subscribers = qos::subscriber_capacity(topic, &creation);
    let opened = crate::service_open::open_within_window(
        "the data service",
        node,
        PortError::Backend(1),
        |node_ref| {
            open_or_create_data_service(
                node_ref,
                service_name,
                &creation,
                &stamp,
                may_create,
                subscribers,
            )
        },
        crate::service_open::publish_subscribe_is_transient,
    )?;
    let service = match opened {
        Ok(service) => service,
        Err(cause) => {
            let absent_and_not_created =
                cause == OpenOrCreate::PublishSubscribeOpenError(Open::DoesNotExist);
            if !absent_and_not_created {
                log::error!(target: "hypha/transport", "hypha_iceoryx2: cannot open data service '{}': {cause:?}{}",
                        service_name.as_str(),
                        crate::service_open::half_created_advice(crate::service_open::hangs_in_creation(cause)));
            }
            return Err(crate::service_open::publish_subscribe_port_error(cause));
        }
    };
    let Ok(stamped) = qos::policy_of_service(service.attributes()) else {
        log::error!(target: "hypha/transport", "hypha_iceoryx2: data service '{}' names no QoS policy of this hypha build \
                 in its attribute {}: another hypha build or a stale segment made it (run `hypha clean` \
                 with the domain stopped)", service_name.as_str(), qos::QOS_ATTRIBUTE);
        return Err(PortError::QosMismatch);
    };
    qos::check_stamped(topic, request, &stamped)?;
    Ok(DataService { service, policy: stamped })
}

/// The builder of a topic's data service: raw bytes behind a hypha frame header.
pub(crate) fn data_service_builder(
    node_ref: &iceoryx2::node::Node<Ipc>,
    service_name: &ServiceName,
) -> iceoryx2::service::builder::publish_subscribe::Builder<[u8], Iox2FrameHeader, Ipc> {
    let service = node_ref.service_builder(service_name);
    let raw = service.publish_subscribe::<[u8]>();
    raw.user_header::<Iox2FrameHeader>()
}

/// One attempt of [`open_data_service`]. A service another endpoint creates in
/// between is `AlreadyExists`, which the caller's retry turns into an open.
fn open_or_create_data_service(
    node_ref: &iceoryx2::node::Node<Ipc>,
    service_name: &ServiceName,
    policy: &QosPolicy,
    stamp: &iceoryx2::service::attribute::AttributeSpecifier,
    may_create: bool,
    subscribers: usize,
) -> Result<
    DataPortFactory,
    iceoryx2::service::builder::publish_subscribe::PublishSubscribeOpenOrCreateError,
> {
    use iceoryx2::service::builder::publish_subscribe::{
        PublishSubscribeOpenError as Open, PublishSubscribeOpenOrCreateError as OpenOrCreate,
    };

    let opened = data_service_builder(node_ref, service_name).open();
    let cause = match opened {
        Ok(service) => return Ok(service),
        Err(cause) => cause,
    };
    let creating = may_create && cause == Open::DoesNotExist;
    if !creating {
        return Err(OpenOrCreate::PublishSubscribeOpenError(cause));
    }
    let builder = data_service_builder(node_ref, service_name);
    let builder = builder.max_publishers(policy.max_publishers);
    let builder = builder.max_subscribers(subscribers);
    let builder = builder.max_nodes(qos::max_nodes(policy, subscribers));
    let builder = builder.subscriber_max_buffer_size(policy.max_depth);
    let builder = builder.subscriber_max_borrowed_samples(MAX_BORROWED_SAMPLES);
    let builder = builder.history_size(qos::HISTORY_SIZE);
    builder.create_with_attributes(stamp).map_err(OpenOrCreate::PublishSubscribeCreateError)
}
