//! Opening a service's iceoryx2 services: the stream with its settings and stamp, the
//! two wake-up events, and the name's server lock with the reclaim of a dead holder.

use core::time::Duration;

use hypha_core::port::{PortError, ServerHolder, ServiceError, ServicePolicy, ServiceSpec};
use iceoryx2::node::{NodeState, NodeView as _};
use iceoryx2::port::notifier::{Notifier, NotifierCreateError};
use iceoryx2::port::port_name::PortName;
use iceoryx2::prelude::*;
use iceoryx2::service::attribute::{AttributeKey, AttributeSet, AttributeValue, AttributeVerifier};
use iceoryx2::service::builder::request_response::{
    RequestResponseCreateError as Create, RequestResponseOpenError as Open,
    RequestResponseOpenOrCreateError as OpenOrCreate,
};

use super::key::StreamKey;
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

/// The attributes a stream requires of its existing service and defines on a new one.
fn verifier(spec: &ServiceSpec) -> Result<AttributeVerifier, PortError> {
    let pairs = [
        (POLICY_ATTRIBUTE, stamp_text(&spec.policy)),
        (TYPE_ATTRIBUTE, spec.type_name.clone()),
        (MD5_ATTRIBUTE, spec.md5.clone()),
    ];
    pairs.iter().try_fold(AttributeVerifier::new(), |verifier, (key, value)| {
        let key = AttributeKey::try_from(*key).map_err(|_| crate::qos::corrupted())?;
        let value =
            AttributeValue::try_from(value.as_str()).map_err(|_| crate::qos::corrupted())?;
        verifier.require(&key, &value).map_err(|_| crate::qos::corrupted())
    })
}

/// One attribute of a service, as text.
pub(crate) fn attribute(attributes: &AttributeSet, key: &str) -> Option<String> {
    crate::qos::attribute_text(attributes, key)
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

/// The attribute that makes an event the server lock of a service: its public name.
const LOCK_ATTRIBUTE: &str = "hypha.service_lock";

/// Open or create the server lock of `key`'s name. The lock carries [`LOCK_ATTRIBUTE`]
/// with the service's name, and opening requires it: an event of the lock's name without
/// it was made by something other than a hypha server, and locks nothing.
///
/// # Errors
/// [`ServiceError::Port`] with [`super::BACKEND_LOCK_INVALID`] for an event that is not
/// this name's lock, and with code 7 when the event cannot be opened.
fn open_lock(
    node: &SharedNode,
    key: &StreamKey,
) -> Result<iceoryx2::service::port_factory::event::PortFactory<Ipc>, ServiceError> {
    use iceoryx2::service::builder::event::{EventOpenError, EventOpenOrCreateError};
    let lock_name = ServiceName::new(&key.lock()).map_err(|_| ServiceError::InvalidName)?;
    let attribute = AttributeKey::try_from(LOCK_ATTRIBUTE).map_err(|_| crate::qos::corrupted())?;
    let name = AttributeValue::try_from(key.public()).map_err(|_| ServiceError::InvalidName)?;
    let verifier =
        AttributeVerifier::new().require(&attribute, &name).map_err(|_| crate::qos::corrupted())?;
    let opened = crate::service_open::open_within_window(
        "the service lock",
        node,
        PortError::Disconnected,
        |node| {
            node.service_builder(&lock_name)
                .event()
                .max_notifiers(LOCK_LIMITS.notifiers)
                .max_listeners(LOCK_LIMITS.listeners)
                .max_nodes(LOCK_LIMITS.nodes)
                .open_or_create_with_attributes(&verifier)
        },
        crate::service_open::event_is_transient,
    )?;
    opened.map_err(|cause| {
        if cause == EventOpenOrCreateError::EventOpenError(EventOpenError::IncompatibleAttributes) {
            log::error!(target: "hypha/transport",
                "hypha: service '{}': the event '{}' is not its server lock (something other \
                 than a hypha server created it); refusing to serve", key.public(), key.lock());
            ServiceError::Port(PortError::Backend(super::BACKEND_LOCK_INVALID))
        } else {
            log::error!(target: "hypha/transport",
                "hypha_iceoryx2: service '{}': cannot open its server lock: {cause:?}", key.public());
            ServiceError::Port(PortError::Backend(7))
        }
    })
}

/// The name the lock's notifier carries: the MD5 and type it serves, within iceoryx2's
/// 64 bytes of a port name, so a refusal names them without a listing of the domain.
fn served_label(spec: &ServiceSpec) -> Result<PortName, ServiceError> {
    let full = format!("{}:{}", spec.md5, spec.type_name);
    let limit = iceoryx2::constants::MAX_PORT_NAME_LENGTH;
    let end =
        (0..=full.len().min(limit)).rev().find(|end| full.is_char_boundary(*end)).unwrap_or(0);
    PortName::new(full.get(..end).unwrap_or_default()).map_err(|_| ServiceError::InvalidIdentity)
}

/// The type and the first 8 hex digits of the MD5 that a lock's notifier name carries.
fn served_of_label(label: &str) -> Option<(String, String)> {
    let (md5, type_name) = label.split_once(':')?;
    Some((type_name.to_owned(), md5.get(..8)?.to_owned()))
}

/// Take the server lock of `key`'s name for `spec`. A dead holder is reclaimed first; a
/// live one refuses with its name, pid and the type it serves.
///
/// # Errors
/// [`ServiceError::AlreadyServed`] for a live holder; [`ServiceError::Port`] with
/// [`super::BACKEND_LOCK_INVALID`] when the event of the lock's name is not this name's
/// lock, or with [`super::BACKEND_RECLAIM`] when a dead holder cannot be reclaimed.
pub(crate) fn acquire_lock(
    node: &SharedNode,
    key: &StreamKey,
    spec: &ServiceSpec,
) -> Result<Notifier<Ipc>, ServiceError> {
    let label = served_label(spec)?;
    let mut reclaimed_once = false;
    loop {
        let service = open_lock(node, key)?;
        match service.notifier_builder().name(&label).create() {
            Ok(notifier) => return Ok(notifier),
            Err(NotifierCreateError::ExceedsMaxSupportedNotifiers) => {}
            Err(cause) => {
                log::error!(target: "hypha/transport",
                    "hypha_iceoryx2: service '{}': cannot take the server lock: {cause:?}", key.public());
                return Err(ServiceError::Port(PortError::Backend(7)));
            }
        }
        let mut holder = None;
        service.dynamic_config().list_notifiers(|details| {
            holder = Some((details.node_id, details.notifier_name.as_str().to_owned()));
            CallbackProgression::Stop
        });
        let Some((holder_id, holder_label)) = holder else {
            // The holder left between the two calls: take the lock again.
            continue;
        };
        match peer_node(&service, holder_id)? {
            PeerNode::Dead(view) if !reclaimed_once => {
                reclaim_dead(key, "its server", *view);
                reclaimed_once = true;
            }
            PeerNode::Dead(_) => {
                return Err(ServiceError::Port(PortError::Backend(super::BACKEND_RECLAIM)));
            }
            PeerNode::Alive { name, pid } => {
                return Err(ServiceError::AlreadyServed(ServerHolder {
                    node: name,
                    pid,
                    served: served_of_label(&holder_label),
                }));
            }
            // A holder whose state cannot be read is never taken over.
            PeerNode::Unknown => {
                return Err(ServiceError::AlreadyServed(ServerHolder {
                    node: String::new(),
                    pid: None,
                    served: served_of_label(&holder_label),
                }));
            }
        }
    }
}

/// A node registered at a service, as its monitor reports it.
pub(crate) enum PeerNode {
    Alive {
        name: String,
        pid: Option<i32>,
    },
    Dead(Box<iceoryx2::node::DeadNodeView<Ipc>>),
    /// Not registered, inaccessible or undefined: never reclaimed.
    Unknown,
}

/// The state of node `id` among the nodes registered at `service`.
///
/// # Errors
/// [`ServiceError::Port`] when the node list cannot be read.
pub(crate) fn peer_node(
    service: &impl iceoryx2::service::port_factory::PortFactory<Service = Ipc>,
    id: iceoryx2::identifiers::UniqueNodeId,
) -> Result<PeerNode, ServiceError> {
    let mut found = PeerNode::Unknown;
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
            found = match state {
                NodeState::Alive(view) => {
                    PeerNode::Alive { name: node_name(&view), pid: pid_of(&view) }
                }
                NodeState::Dead(view) => PeerNode::Dead(Box::new(view)),
                NodeState::Inaccessible(_) | NodeState::Undefined(_) => PeerNode::Unknown,
            };
            CallbackProgression::Stop
        })
        .map_err(|cause| ServiceError::Port(crate::transport::node_list_failed(cause)))?;
    Ok(found)
}

pub(crate) fn node_name(view: &impl iceoryx2::node::NodeView) -> String {
    view.details().as_ref().map(|details| details.name().as_str().to_owned()).unwrap_or_default()
}

pub(crate) fn pid_of(view: &impl iceoryx2::node::NodeView) -> Option<i32> {
    view.id().pid::<Ipc>().ok().map(|pid| pid.value())
}

/// Reclaim a dead node that holds `what` of a service, as its own restart would. Only a
/// node that the monitor reports dead reaches here.
pub(crate) fn reclaim_dead(key: &StreamKey, what: &str, view: iceoryx2::node::DeadNodeView<Ipc>) {
    let name = node_name(&view);
    let pid = pid_of(&view).map_or_else(|| "unknown".to_owned(), |pid| pid.to_string());
    match view.blocking_remove_stale_resources(crate::transport::RECLAIM_TIMEOUT) {
        Ok(()) => log::info!(target: "hypha/transport",
            "hypha: service '{}': reclaimed dead node '{name}' (pid {pid}) that held {what}", key.public()),
        // Another process reclaims it at the same moment, or it is gone: the caller tries again.
        Err(cause) => log::info!(target: "hypha/transport",
            "hypha: service '{}': dead node '{name}' (pid {pid}) not reclaimed here: {cause:?}", key.public()),
    }
}
