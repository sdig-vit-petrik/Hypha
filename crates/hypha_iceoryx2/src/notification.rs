//! Opening the event services that carry wake-ups.

use hypha_core::port::PortError;
use iceoryx2::port::listener::Listener;
use iceoryx2::port::notifier::Notifier;
use iceoryx2::prelude::*;

use crate::transport::{DataPortFactory, Ipc, SharedNode};

use core::sync::atomic::{AtomicU64, Ordering};

/// Sends of this process whose wake notification failed. Nothing polls a listener, so
/// such a sample waits for the next notification of its topic.
static FAILED_NOTIFICATIONS: AtomicU64 = AtomicU64::new(0);

/// How many sends of this process were sent but not notified. Wraps.
#[must_use]
pub fn failed_notifications() -> u64 {
    FAILED_NOTIFICATIONS.load(Ordering::Relaxed)
}

/// True when the wake notification of a send went out; counts it when it did not.
pub(crate) fn notified<T, E>(result: &Result<T, E>) -> bool {
    if result.is_ok() {
        return true;
    }
    FAILED_NOTIFICATIONS.fetch_add(1, Ordering::Relaxed);
    false
}

/// Largest application event identifier supported by a notification service.
pub(crate) const MAX_EVENT_ID: usize = 255;

/// The ports and nodes one event service is created for.
///
/// A notifier keeps about 1.3 KB per listener the service allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EventLimits {
    pub(crate) notifiers: usize,
    pub(crate) listeners: usize,
    pub(crate) nodes: usize,
}

impl EventLimits {
    /// The control service of one `WaitSet`: its own listener and notifier.
    pub(crate) const CONTROL: Self = Self { notifiers: 1, listeners: 1, nodes: 1 };

    /// The event service of the topic of `service`, sized from the data service so
    /// both ends ask for the same event service.
    pub(crate) fn of_topic(service: &DataPortFactory) -> Self {
        let config = service.static_config();
        Self {
            notifiers: config.max_publishers(),
            listeners: config.max_subscribers(),
            nodes: config.max_nodes(),
        }
    }
}

/// Open or create the event service `service_name` on the node `node`, with `limits` or,
/// without them, the limits of the domain `Config`.
///
/// `owner` names the caller in the diagnostic written when the open fails.
pub(crate) fn open_event_service(
    node: &SharedNode,
    service_name: &ServiceName,
    limits: Option<EventLimits>,
    owner: &str,
) -> Result<iceoryx2::service::port_factory::event::PortFactory<Ipc>, PortError> {
    let opened = crate::service_open::open_within_window(
        "the notification service",
        node,
        PortError::Disconnected,
        |node_ref| open_or_create_event(node_ref, service_name, limits),
        crate::service_open::event_is_transient,
    )?;
    opened.map_err(|cause| {
        log::error!(target: "hypha/transport", "hypha_iceoryx2: {owner} open_or_create failed: {cause:?}");
        PortError::Backend(7)
    })
}

/// One attempt of [`open_event_service`]: the node must be locked by the caller.
fn open_or_create_event(
    node: &Node<Ipc>,
    service_name: &ServiceName,
    limits: Option<EventLimits>,
) -> Result<
    iceoryx2::service::port_factory::event::PortFactory<Ipc>,
    iceoryx2::service::builder::event::EventOpenOrCreateError,
> {
    let builder = node.service_builder(service_name).event();
    match limits {
        Some(limits) => builder
            .max_notifiers(limits.notifiers)
            .max_listeners(limits.listeners)
            .max_nodes(limits.nodes)
            .open_or_create(),
        None => builder.open_or_create(),
    }
}

pub(crate) fn create_notifier(
    node: &SharedNode,
    service_name: &str,
    limits: Option<EventLimits>,
) -> Result<Notifier<Ipc>, PortError> {
    let service_name: ServiceName = service_name.try_into().map_err(|_| PortError::InvalidTopic)?;
    let service = open_event_service(node, &service_name, limits, "create_notifier")?;
    service.notifier_builder().create().map_err(|cause| {
        log::error!(target: "hypha/transport", "hypha_iceoryx2: create_notifier create failed: {cause:?}");
        PortError::Backend(7)
    })
}

pub(crate) fn create_listener(
    node: &SharedNode,
    service_name: &str,
    limits: Option<EventLimits>,
) -> Result<Listener<Ipc>, PortError> {
    let service_name: ServiceName = service_name.try_into().map_err(|_| PortError::InvalidTopic)?;
    let service = open_event_service(node, &service_name, limits, "create_listener")?;
    service.listener_builder().create().map_err(|cause| {
        log::error!(target: "hypha/transport", "hypha_iceoryx2: notification listener creation failed: {cause:?}");
        PortError::Backend(7)
    })
}

#[cfg(test)]
mod tests {
    use super::{failed_notifications, notified};

    #[test]
    fn a_failed_notification_is_counted_and_a_sent_one_is_not() {
        let before = failed_notifications();
        assert!(notified::<(), ()>(&Ok(())));
        assert_eq!(failed_notifications(), before, "a sent notification counts nothing");
        assert!(!notified::<(), ()>(&Err(())));
        assert_eq!(failed_notifications(), before.wrapping_add(1));
    }
}
