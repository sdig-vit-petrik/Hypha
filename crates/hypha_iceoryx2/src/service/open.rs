//! Opening a service's iceoryx2 services: the stream with its settings and stamp, the
//! two wake-up events, and the name's server lock with the reclaim of a dead holder.

use core::time::Duration;

use hypha_core::port::{PortError, ServerHolder, ServiceError, ServicePolicy, ServiceSpec};
use iceoryx2::node::{NodeState, NodeView as _};
use iceoryx2::port::notifier::{Notifier, NotifierCreateError};
use iceoryx2::prelude::*;
use iceoryx2::service::attribute::{AttributeKey, AttributeSet, AttributeValue, AttributeVerifier};
use iceoryx2::service::builder::request_response::{
    RequestResponseCreateError as Create, RequestResponseOpenError as Open,
    RequestResponseOpenOrCreateError as OpenOrCreate,
};

use super::key::{StreamKey, public_name_of_stream};
use crate::header::Iox2FrameHeader;
use crate::notification::EventLimits;
use crate::transport::{Ipc, SharedNode};

/// The request-response service of one stream.
pub(crate) type StreamFactory = iceoryx2::service::port_factory::request_response::PortFactory<
    Ipc,
    [u8],
    Iox2FrameHeader,
    [u8],
    Iox2FrameHeader,
>;

/// The attribute that carries a service's policy.
pub(crate) const POLICY_ATTRIBUTE: &str = "hypha.service_qos";
/// The attribute that carries a service's `.srv` type name.
pub(crate) const TYPE_ATTRIBUTE: &str = "hypha.srv_type";
/// The attribute that carries a service's `.srv` MD5.
pub(crate) const MD5_ATTRIBUTE: &str = "hypha.srv_md5";

/// The node slots of a stream: its clients, the observers', the one server, and one for
/// a tool that lists the stream (it opens the service for an instant).
fn stream_nodes(policy: &ServicePolicy) -> usize {
    policy.admitted_clients().saturating_add(2)
}

/// The stamp text of `policy`: `name/clients/outstanding/request/response/timeout_ns`.
pub(crate) fn stamp_text(policy: &ServicePolicy) -> String {
    format!(
        "{}/{}/{}/{}/{}/{}",
        policy.name,
        policy.max_clients,
        policy.max_outstanding,
        policy.max_request_bytes,
        policy.max_response_bytes,
        policy.timeout.as_nanos()
    )
}

/// The policy a stamp names, or `None` when this build does not write it.
pub(crate) fn parse_stamp(text: &str) -> Option<ServicePolicy> {
    let mut fields = text.split('/');
    let name = fields.next()?.to_owned();
    let mut number = || fields.next()?.parse::<u64>().ok();
    let max_clients = usize::try_from(number()?).ok()?;
    let max_outstanding = usize::try_from(number()?).ok()?;
    let max_request_bytes = usize::try_from(number()?).ok()?;
    let max_response_bytes = usize::try_from(number()?).ok()?;
    let timeout = Duration::from_nanos(number()?);
    if fields.next().is_some() {
        return None;
    }
    let policy = ServicePolicy {
        name,
        max_clients,
        max_outstanding,
        max_request_bytes,
        max_response_bytes,
        timeout,
    };
    policy.validate().ok()?;
    Some(policy)
}

fn corrupted() -> PortError {
    PortError::Backend(crate::service_open::BACKEND_SERVICE_CORRUPTED)
}

/// The attributes a stream requires of its existing service and defines on a new one.
fn verifier(spec: &ServiceSpec) -> Result<AttributeVerifier, PortError> {
    let pairs = [
        (POLICY_ATTRIBUTE, stamp_text(&spec.policy)),
        (TYPE_ATTRIBUTE, spec.type_name.clone()),
        (MD5_ATTRIBUTE, spec.md5.clone()),
    ];
    pairs.iter().try_fold(AttributeVerifier::new(), |verifier, (key, value)| {
        let key = AttributeKey::try_from(*key).map_err(|_| corrupted())?;
        let value = AttributeValue::try_from(value.as_str()).map_err(|_| corrupted())?;
        verifier.require(&key, &value).map_err(|_| corrupted())
    })
}

/// One attribute of a service, as text.
pub(crate) fn attribute(attributes: &AttributeSet, key: &str) -> Option<String> {
    let key = AttributeKey::try_from(key).ok()?;
    if attributes.number_of_key_values(&key) != 1 {
        return None;
    }
    let value = attributes.key_value(&key, 0)?;
    core::str::from_utf8(value.as_bytes()).ok().map(str::to_owned)
}

/// One attempt to open or create a stream, with every setting hypha fixes
/// (`docs/design/native-services.md` §6). The node must be locked by the caller.
fn try_open_stream(
    node: &Node<Ipc>,
    name: &ServiceName,
    policy: &ServicePolicy,
    verifier: &AttributeVerifier,
) -> Result<StreamFactory, OpenOrCreate> {
    node.service_builder(name)
        .request_response::<[u8], [u8]>()
        .request_user_header::<Iox2FrameHeader>()
        .response_user_header::<Iox2FrameHeader>()
        .max_servers(1)
        .max_clients(policy.admitted_clients())
        .max_nodes(stream_nodes(policy))
        .max_active_requests_per_client(policy.max_outstanding)
        .max_loaned_requests(1)
        .max_response_buffer_size(1)
        .max_borrowed_responses_per_pending_response(1)
        .enable_fire_and_forget_requests(false)
        .enable_safe_overflow_for_requests(false)
        .enable_safe_overflow_for_responses(false)
        .open_or_create_with_attributes(verifier)
}

/// True when a stream open failed because another endpoint creates or removes it.
fn is_transient(cause: OpenOrCreate) -> bool {
    match cause {
        OpenOrCreate::SystemInFlux => true,
        OpenOrCreate::RequestResponseOpenError(open) => matches!(
            open,
            Open::HangsInCreation
                | Open::IsMarkedForDestruction
                | Open::ServiceInCorruptedState
                | Open::DoesNotExist
        ),
        OpenOrCreate::RequestResponseCreateError(create) => matches!(
            create,
            Create::IsBeingCreatedByAnotherInstance
                | Create::AlreadyExists
                | Create::ServiceInCorruptedState
        ),
    }
}

/// Open or create the stream of `key` for `spec`, within the bounded open window.
///
/// # Errors
/// [`ServiceError::QosMismatch`] when the stream exists with another policy, type or
/// settings, [`ServiceError::Port`] otherwise.
pub(crate) fn open_stream(
    node: &SharedNode,
    key: &StreamKey,
    spec: &ServiceSpec,
) -> Result<StreamFactory, ServiceError> {
    let name = ServiceName::new(&key.stream()).map_err(|_| ServiceError::InvalidName)?;
    let verifier = verifier(spec)?;
    let opened = crate::service_open::open_within_window(
        "the service stream",
        node,
        PortError::Backend(1),
        |node_ref| try_open_stream(node_ref, &name, &spec.policy, &verifier),
        is_transient,
    )?;
    opened.map_err(|cause| {
        let mismatch = matches!(
            cause,
            OpenOrCreate::RequestResponseOpenError(
                Open::IncompatibleAttributes
                    | Open::IncompatibleRequestOrResponseType
                    | Open::DoesNotSupportRequestedAmountOfClients
                    | Open::DoesNotSupportRequestedAmountOfActiveRequestsPerClient
                    | Open::DoesNotSupportRequestedAmountOfNodes
                    | Open::DoesNotSupportRequestedAmountOfServers
                    | Open::DoesNotSupportRequestedResponseBufferSize
                    | Open::DoesNotSupportRequestedAmountOfClientRequestLoans
                    | Open::DoesNotSupportRequestedAmountOfBorrowedResponsesPerPendingResponse
                    | Open::IncompatibleOverflowBehaviorForRequests
                    | Open::IncompatibleOverflowBehaviorForResponses
                    | Open::IncompatibleBehaviorForFireAndForgetRequests
            )
        );
        if mismatch {
            log::error!(target: "hypha/transport",
                "hypha: service '{}': it exists with another policy or settings than {} ({cause:?})",
                key.public(), stamp_text(&spec.policy));
            ServiceError::QosMismatch
        } else {
            log::error!(target: "hypha/transport",
                "hypha_iceoryx2: cannot open the service '{}': {cause:?}", key.public());
            ServiceError::Port(PortError::Backend(2))
        }
    })
}

/// The event that wakes the server of a stream with `policy`: every client may notify it.
pub(crate) fn request_event_limits(policy: &ServicePolicy) -> EventLimits {
    let clients = policy.admitted_clients();
    EventLimits { notifiers: clients, listeners: 2, nodes: clients.saturating_add(2) }
}

/// The event that wakes the clients of a stream with `policy`: the server notifies it.
pub(crate) fn response_event_limits(policy: &ServicePolicy) -> EventLimits {
    let clients = policy.admitted_clients();
    EventLimits { notifiers: 2, listeners: clients, nodes: clients.saturating_add(2) }
}

/// The name lock: one notifier, held by the server. Every node that tries to create a
/// server opens the lock's service first, so its node slots cover the holder, a dead
/// holder, and servers created at the same moment.
const LOCK_LIMITS: EventLimits = EventLimits { notifiers: 1, listeners: 1, nodes: 32 };

/// Take the server lock of `key`'s name. A dead holder is reclaimed first; a live one
/// refuses with its name, pid and the type it serves.
///
/// # Errors
/// [`ServiceError::AlreadyServed`] for a live holder, [`ServiceError::Port`] otherwise.
pub(crate) fn acquire_lock(
    node: &SharedNode,
    key: &StreamKey,
) -> Result<Notifier<Ipc>, ServiceError> {
    let lock_name = ServiceName::new(&key.lock()).map_err(|_| ServiceError::InvalidName)?;
    let mut reclaimed_once = false;
    loop {
        let service = crate::notification::open_event_service(
            node,
            &lock_name,
            Some(LOCK_LIMITS),
            "the service lock",
        )?;
        match service.notifier_builder().create() {
            Ok(notifier) => return Ok(notifier),
            Err(NotifierCreateError::ExceedsMaxSupportedNotifiers) => {}
            Err(cause) => {
                log::error!(target: "hypha/transport",
                    "hypha_iceoryx2: service '{}': cannot take the server lock: {cause:?}", key.public());
                return Err(ServiceError::Port(PortError::Backend(7)));
            }
        }
        let mut holder_id = None;
        service.dynamic_config().list_notifiers(|details| {
            holder_id = Some(details.node_id);
            CallbackProgression::Stop
        });
        let Some(holder_id) = holder_id else {
            // The holder left between the two calls: take the lock again.
            continue;
        };
        let holder = holder_state(&service, holder_id)?;
        match holder {
            Holder::Dead(view) if !reclaimed_once => {
                reclaim(key, *view);
                reclaimed_once = true;
            }
            Holder::Dead(_) => return Err(ServiceError::Port(PortError::Backend(41))),
            Holder::Alive(node_name, pid) | Holder::Other(node_name, pid) => {
                let served = served_by(key, holder_id);
                return Err(ServiceError::AlreadyServed(ServerHolder {
                    node: node_name,
                    pid,
                    served,
                }));
            }
        }
    }
}

enum Holder {
    Alive(String, Option<i32>),
    Dead(Box<iceoryx2::node::DeadNodeView<Ipc>>),
    /// Inaccessible or undefined: never reclaimed.
    Other(String, Option<i32>),
}

fn holder_state(
    service: &impl iceoryx2::service::port_factory::PortFactory<Service = Ipc>,
    id: iceoryx2::identifiers::UniqueNodeId,
) -> Result<Holder, ServiceError> {
    let mut found = None;
    service
        .nodes(|state| {
            let this = match &state {
                NodeState::Alive(v) => *v.id(),
                NodeState::Dead(v) => *v.id(),
                NodeState::Inaccessible(v) | NodeState::Undefined(v) => *v,
            };
            if this != id {
                return CallbackProgression::Continue;
            }
            found = Some(match state {
                NodeState::Alive(view) => Holder::Alive(node_name(&view), pid_of(&view)),
                NodeState::Dead(view) => Holder::Dead(Box::new(view)),
                NodeState::Inaccessible(_) | NodeState::Undefined(_) => {
                    Holder::Other(String::new(), None)
                }
            });
            CallbackProgression::Stop
        })
        .map_err(|cause| ServiceError::Port(crate::transport::node_list_failed(cause)))?;
    Ok(found.unwrap_or_else(|| Holder::Other(String::new(), None)))
}

pub(crate) fn node_name(view: &impl iceoryx2::node::NodeView) -> String {
    view.details().as_ref().map_or_default(|details| details.name().as_str().to_owned())
}

pub(crate) fn pid_of(view: &impl iceoryx2::node::NodeView) -> Option<i32> {
    view.id().pid::<Ipc>().ok().map(|pid| pid.value())
}

/// Reclaim a dead node that holds a service's server slot, as its own restart would.
fn reclaim(key: &StreamKey, view: iceoryx2::node::DeadNodeView<Ipc>) {
    let name = node_name(&view);
    let pid = pid_of(&view).map_or_else(|| "unknown".to_owned(), |pid| pid.to_string());
    match view.blocking_remove_stale_resources(crate::transport::RECLAIM_TIMEOUT) {
        Ok(()) => log::info!(target: "hypha/transport",
            "hypha: service '{}': reclaimed dead node '{name}' (pid {pid}) that held its server", key.public()),
        // Another process reclaims it at the same moment, or it is gone: try the lock again.
        Err(cause) => log::info!(target: "hypha/transport",
            "hypha: service '{}': dead node '{name}' (pid {pid}) not reclaimed here: {cause:?}", key.public()),
    }
}

/// The type and MD5 prefix of the stream of `key`'s name that node `holder` is
/// registered at, for the refusal's message.
fn served_by(
    key: &StreamKey,
    holder: iceoryx2::identifiers::UniqueNodeId,
) -> Option<(String, String)> {
    let config = crate::domain::config().ok()?;
    let prefix = key.sibling_prefix();
    let mut served = None;
    let listed = Ipc::list(config, |details| {
        let config = &details.static_details;
        let name = config.name().as_str();
        let is_stream = name.starts_with(&prefix) && public_name_of_stream(name).is_some();
        let is_request_response = matches!(
            config.messaging_pattern(),
            iceoryx2::service::static_config::messaging_pattern::MessagingPattern::RequestResponse(
                _
            )
        );
        let holder_is_there = details.dynamic_details.as_ref().is_some_and(|dynamic| {
            dynamic.nodes.iter().any(|state| match state {
                NodeState::Alive(view) => *view.id() == holder,
                NodeState::Dead(view) => *view.id() == holder,
                NodeState::Inaccessible(id) | NodeState::Undefined(id) => *id == holder,
            })
        });
        if !(is_stream && is_request_response && holder_is_there) {
            return CallbackProgression::Continue;
        }
        let type_name = attribute(config.attributes(), TYPE_ATTRIBUTE).unwrap_or_default();
        let md5 = attribute(config.attributes(), MD5_ATTRIBUTE).unwrap_or_default();
        served = Some((type_name, md5.get(..8).unwrap_or_default().to_owned()));
        CallbackProgression::Stop
    });
    listed.ok()?;
    served
}
