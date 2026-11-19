//! Subscriber creation and its error mapping.

use hypha_core::port::PortError;
use iceoryx2::port::port_name::PortName;
use iceoryx2::port::subscriber::{Subscriber, SubscriberCreateError};
use iceoryx2::prelude::CallbackProgression;
use iceoryx2::service::port_factory::PortFactory as _;
use iceoryx2::service::port_factory::publish_subscribe::PortFactory;

use crate::header::Iox2FrameHeader;
use crate::qos::{MAX_PORT_NAME_BYTES, OBSERVER_PORT_PREFIX, SubscriberSlots};
use crate::transport::Ipc;

/// Create a subscriber of `topic` with `buffer_size`, or the policy's depth when
/// `None`. Ports of dead nodes count against the limit. An undeclared subscriber's
/// port carries the observer prefix (see [`admit`]).
///
/// # Errors
/// [`PortError::ResourceLimit`] when `buffer_size` exceeds the depth the
/// service's policy allows, or when the slots of the subscriber's kind are full.
pub(crate) fn create_subscriber(
    service: &PortFactory<Ipc, [u8], Iox2FrameHeader>,
    topic: &str,
    buffer_size: Option<usize>,
    port_name: &PortName,
    slots: SubscriberSlots,
) -> Result<Subscriber<Ipc, [u8], Iox2FrameHeader>, PortError> {
    let max_depth = service.static_config().subscriber_max_buffer_size();
    if let Some(depth) = buffer_size.filter(|depth| *depth > max_depth) {
        log::error!(target: "hypha/transport", "hypha_iceoryx2: topic '{topic}': depth {depth} exceeds the {max_depth} the \
                 service's QoS policy allows; ask for depth 0 to get that depth, or declare in every process that \
                 may create the topic a policy with a deeper max_depth");
        return Err(PortError::ResourceLimit);
    }
    admit(service, topic, slots)?;
    let observer_name;
    let name: &PortName = match slots {
        SubscriberSlots::Observer { .. } => {
            observer_name = observer_port_name(port_name)?;
            &observer_name
        }
        SubscriberSlots::Declared { .. } | SubscriberSlots::Unsplit => port_name,
    };
    let create = || {
        let builder = service.subscriber_builder().name(name);
        match buffer_size {
            Some(depth) => builder.buffer_size(depth.max(1)).create(),
            None => builder.create(),
        }
    };
    create().map_err(subscriber_create_port_error)
}

/// The port name of an undeclared subscriber, cut to iceoryx2's length limit.
fn observer_port_name(node_port: &PortName) -> Result<PortName, PortError> {
    let marked = format!("{OBSERVER_PORT_PREFIX}{}", node_port.as_str());
    let end = marked.len().min(MAX_PORT_NAME_BYTES);
    let cut = marked.get(..end).ok_or(PortError::InvalidTopic)?;
    PortName::new(cut).map_err(|_| PortError::InvalidTopic)
}

/// Refuse a subscriber whose kind has no slot left on this service. Two processes
/// racing for the last slot can both pass; the service's own limit then refuses one.
///
/// # Errors
/// [`PortError::ResourceLimit`], with one log line, when the slots of the kind are full.
fn admit(
    service: &PortFactory<Ipc, [u8], Iox2FrameHeader>,
    topic: &str,
    slots: SubscriberSlots,
) -> Result<(), PortError> {
    let (limit, held, kind) = match slots {
        SubscriberSlots::Unsplit => return Ok(()),
        SubscriberSlots::Declared { limit } => {
            let observers = observers_held(service);
            let all = service.dynamic_config().number_of_subscribers();
            (limit, all.saturating_sub(observers), "declared")
        }
        SubscriberSlots::Observer { limit } => (limit, observers_held(service), "undeclared"),
    };
    let has_room = held < limit;
    if has_room {
        return Ok(());
    }
    log::error!(target: "hypha/transport", "hypha_iceoryx2: topic '{topic}': all {limit} {kind} subscriber slots of the service \
             are in use; this {kind} subscriber is refused. Undeclared subscribers (tools) may hold only their \
             reserved slots, so a declared subscriber is refused only when its own slots are full");
    Err(PortError::ResourceLimit)
}

fn observers_held(service: &PortFactory<Ipc, [u8], Iox2FrameHeader>) -> usize {
    let mut observers = 0_usize;
    service.dynamic_config().list_subscribers(|details| {
        if details.subscriber_name.as_str().starts_with(OBSERVER_PORT_PREFIX) {
            observers = observers.saturating_add(1);
        }
        CallbackProgression::Continue
    });
    observers
}

fn subscriber_create_port_error(cause: SubscriberCreateError) -> PortError {
    use SubscriberCreateError as Error;
    match cause {
        Error::ExceedsMaxSupportedSubscribers => PortError::ResourceLimit,
        Error::BufferSizeExceedsMaxSupportedBufferSizeOfService => {
            log::error!(target: "hypha/transport", "hypha_iceoryx2: {cause}: a stale shared-memory segment or a different hypha \
                     build created this service; run `./clean-shm.sh`");
            PortError::QosMismatch
        }
        Error::HistoryRequestExceedsHistorySizeOfService
        | Error::HistoryRequestExceedsBufferSizeOfSubscriber => PortError::QosMismatch,
        Error::FailedToDeployThreadsafetyPolicy
        | Error::UnableToCreatePortTag
        | Error::UnableToGenerateUniqueSubscriberId => PortError::Backend(4),
    }
}
