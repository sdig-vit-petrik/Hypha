//! Opening a service that another endpoint may be opening at the same moment.
//!
//! iceoryx2 reports the overlap states (not finalised yet, being created by
//! another instance, being torn down) as errors that clear within milliseconds,
//! so an open retries within a bounded window. The node lock is released before
//! each wait. An attempt waiting inside iceoryx2 can run up to its
//! `creation_timeout` (1 s by default), which cannot be lowered for service opens
//! alone.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How long a service open may keep retrying.
pub(crate) const SERVICE_OPEN_WINDOW: Duration = Duration::from_millis(500);

/// The number of backoff steps the wait grows over before it stays put.
const SERVICE_OPEN_ATTEMPTS: u32 = 8;

/// Unit of the wait between attempts; it grows by one unit per attempt.
const RETRY_BACKOFF_STEP: Duration = Duration::from_millis(15);

/// How long to wait before attempt number `attempt` is repeated.
///
/// Endpoints that reach a service together give up at the same instant, so a
/// clock offset keeps them from retrying in lockstep.
fn retry_delay(attempt: u32) -> Duration {
    let steps = attempt.min(SERVICE_OPEN_ATTEMPTS).saturating_add(1);
    let offset = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.subsec_nanos().checked_rem(4).unwrap_or(0));
    RETRY_BACKOFF_STEP.saturating_mul(steps.saturating_add(offset))
}

/// The retries of one service open, over [`SERVICE_OPEN_WINDOW`].
///
///
/// The caller takes any lock inside the attempt, so it is released before the wait.
/// A failure that is not transient is never retried.
pub(crate) struct OpenWindow {
    what: &'static str,
    started: Instant,
    attempt: u32,
}

impl OpenWindow {
    /// Start the window now. `what` names the service in the retry log line.
    pub(crate) fn start(what: &'static str) -> Self {
        Self { what, started: Instant::now(), attempt: 1 }
    }

    /// Judge a failed attempt. True when the caller should attempt again; the wait has
    /// been taken by then. False when the caller must report `cause`.
    pub(crate) fn retry_after<E: core::fmt::Debug>(
        &mut self,
        cause: &E,
        is_transient: bool,
    ) -> bool {
        let remaining = SERVICE_OPEN_WINDOW.saturating_sub(self.started.elapsed());
        let window_spent = remaining.is_zero();
        let should_retry = is_transient && !window_spent;
        if !should_retry {
            return false;
        }
        log::warn!(target: "hypha/transport", "hypha_iceoryx2: {} is in use by another endpoint, attempt {}: {cause:?}",
             self.what, self.attempt);
        std::thread::sleep(retry_delay(self.attempt).min(remaining));
        self.attempt = self.attempt.saturating_add(1);
        true
    }
}

/// Run `attempt` on the node until it succeeds, fails for good, or the window named
/// `what` is spent. The node lock is held for one attempt only. The inner `Err` is
/// the backend's last error; the outer one a failure of the node itself.
pub(crate) fn open_within_window<T, E: Copy + core::fmt::Debug>(
    what: &'static str,
    node: &crate::transport::SharedNode,
    node_gone: hypha_core::port::PortError,
    mut attempt: impl FnMut(&iceoryx2::node::Node<crate::transport::Ipc>) -> Result<T, E>,
    is_transient: impl Fn(E) -> bool,
) -> Result<Result<T, E>, hypha_core::port::PortError> {
    let mut window = OpenWindow::start(what);
    loop {
        let outcome = {
            let locked_node = crate::locked(node);
            let node_ref = locked_node.as_ref().ok_or(node_gone)?;
            attempt(node_ref)
        };
        let cause = match outcome {
            Ok(service) => return Ok(Ok(service)),
            Err(cause) => cause,
        };
        let transient = is_transient(cause);
        if !window.retry_after(&cause, transient) {
            return Ok(Err(cause));
        }
    }
}

/// True when a publish-subscribe failure comes from another endpoint working
/// on the same service, rather than from a setting that does not match.
pub(crate) fn publish_subscribe_is_transient(
    cause: iceoryx2::service::builder::publish_subscribe::PublishSubscribeOpenOrCreateError,
) -> bool {
    use iceoryx2::service::builder::publish_subscribe::{
        PublishSubscribeCreateError as Create, PublishSubscribeOpenOrCreateError as OpenOrCreate,
    };
    match cause {
        OpenOrCreate::SystemInFlux => true,
        OpenOrCreate::PublishSubscribeOpenError(open) => publish_subscribe_open_is_transient(open),
        OpenOrCreate::PublishSubscribeCreateError(create) => matches!(
            create,
            Create::HangsInCreation
                | Create::IsBeingCreatedByAnotherInstance
                | Create::AlreadyExists
        ),
    }
}

/// True when opening an existing data service failed because another endpoint
/// is creating or removing that service.
pub(crate) fn publish_subscribe_open_is_transient(
    cause: iceoryx2::service::builder::publish_subscribe::PublishSubscribeOpenError,
) -> bool {
    use iceoryx2::service::builder::publish_subscribe::PublishSubscribeOpenError as Open;
    matches!(cause, Open::HangsInCreation | Open::IsMarkedForDestruction)
}

/// The event-service counterpart of [`publish_subscribe_is_transient`].
pub(crate) fn event_is_transient(
    cause: iceoryx2::service::builder::event::EventOpenOrCreateError,
) -> bool {
    use iceoryx2::service::builder::event::{
        EventCreateError as Create, EventOpenError as Open, EventOpenOrCreateError as OpenOrCreate,
    };
    match cause {
        OpenOrCreate::SystemInFlux => true,
        // A static/dynamic mismatch is reported as corrupted while another endpoint
        // creates or releases the pair; a really corrupted service fails once the
        // attempts run out. DoesNotExist is transient only here: another endpoint removed
        // the service between open_or_create's existence check and its open.
        OpenOrCreate::EventOpenError(open) => matches!(
            open,
            Open::HangsInCreation
                | Open::IsMarkedForDestruction
                | Open::ServiceInCorruptedState
                | Open::DoesNotExist
        ),
        OpenOrCreate::EventCreateError(create) => matches!(
            create,
            Create::IsBeingCreatedByAnotherInstance
                | Create::AlreadyExists
                | Create::ServiceInCorruptedState
        ),
    }
}

/// Backend code for a service another endpoint held for the whole attempt budget.
pub(crate) const BACKEND_SERVICE_CONTENDED: u32 = 20;
/// Backend code for a service the caller is not permitted to use.
pub(crate) const BACKEND_SERVICE_PERMISSIONS: u32 = 21;
/// Backend code for a service whose stored state is not readable.
pub(crate) const BACKEND_SERVICE_CORRUPTED: u32 = 22;

/// True when a data service stayed half-created: its creator died between the two
/// creation steps, or its metadata is damaged.
pub(crate) fn hangs_in_creation(
    cause: iceoryx2::service::builder::publish_subscribe::PublishSubscribeOpenOrCreateError,
) -> bool {
    use iceoryx2::service::builder::publish_subscribe::{
        PublishSubscribeCreateError as Create, PublishSubscribeOpenError as Open,
        PublishSubscribeOpenOrCreateError as OpenOrCreate,
    };
    matches!(
        cause,
        OpenOrCreate::PublishSubscribeOpenError(Open::HangsInCreation)
            | OpenOrCreate::PublishSubscribeCreateError(Create::HangsInCreation)
    )
}

/// The advice appended to a service open that ended on a half-created service, or
/// an empty text. Deleting another node's service files is not safe.
pub(crate) fn half_created_advice(hangs_in_creation: bool) -> String {
    use iceoryx2::prelude::SemanticString as _;

    if !hangs_in_creation {
        return String::new();
    }
    let root = crate::domain::config().map_or_else(
        |_| "the iceoryx2 root directory".to_owned(),
        |config| {
            let root = String::from_utf8_lossy(config.global.root_path().as_bytes()).into_owned();
            root.trim_end_matches('/').to_owned()
        },
    );
    format!(
        "; the service looks half-created or its metadata under {root}/services is damaged; if no \
         node is creating it right now, restart its publisher or run hypha clean with the domain \
         stopped"
    )
}

/// Translate a publish-subscribe failure into the error the caller sees.
///
/// Contention is not a settings mismatch and is not reported as one.
pub(crate) fn publish_subscribe_port_error(
    cause: iceoryx2::service::builder::publish_subscribe::PublishSubscribeOpenOrCreateError,
) -> hypha_core::port::PortError {
    use hypha_core::port::PortError;
    use iceoryx2::service::builder::publish_subscribe::{
        PublishSubscribeCreateError as Create, PublishSubscribeOpenOrCreateError as OpenOrCreate,
    };

    // The overlap never cleared within the budget; the caller's settings are not in question.
    if publish_subscribe_is_transient(cause) {
        return PortError::Backend(BACKEND_SERVICE_CONTENDED);
    }

    match cause {
        OpenOrCreate::PublishSubscribeOpenError(open) => publish_subscribe_open_port_error(open),
        OpenOrCreate::PublishSubscribeCreateError(create) => match create {
            Create::InsufficientPermissions => PortError::Backend(BACKEND_SERVICE_PERMISSIONS),
            Create::ServiceInCorruptedState => PortError::Backend(BACKEND_SERVICE_CORRUPTED),
            Create::SubscriberBufferMustBeLargerThanHistorySize
            | Create::UnableToAcquireTypeDefinition
            | Create::InvalidTypeDefinition => PortError::QosMismatch,
            Create::Interrupt
            | Create::InternalFailure
            | Create::UnableToCreateServiceTag
            | Create::ServiceConfigCouldNotBeCreated
            | Create::UnableToGenerateUniqueServiceId => PortError::Backend(0),
            Create::AlreadyExists
            | Create::IsBeingCreatedByAnotherInstance
            | Create::HangsInCreation => PortError::Backend(BACKEND_SERVICE_CONTENDED),
        },
        OpenOrCreate::SystemInFlux => PortError::Backend(BACKEND_SERVICE_CONTENDED),
    }
}

/// Translate a failure from opening an existing data service.
pub(crate) fn publish_subscribe_open_port_error(
    cause: iceoryx2::service::builder::publish_subscribe::PublishSubscribeOpenError,
) -> hypha_core::port::PortError {
    use hypha_core::port::PortError;
    use iceoryx2::service::builder::publish_subscribe::PublishSubscribeOpenError as Open;

    if publish_subscribe_open_is_transient(cause) {
        return PortError::Backend(BACKEND_SERVICE_CONTENDED);
    }
    match cause {
        Open::DoesNotExist => PortError::NotAvailable,
        Open::IncompatibleTypes
        | Open::IncompatibleMessagingPattern
        | Open::IncompatibleAttributes
        | Open::IncompatibleOverflowBehavior
        | Open::UnableToAcquireTypeDefinition
        | Open::InvalidTypeDefinition => PortError::QosMismatch,
        Open::DoesNotSupportRequestedMinBufferSize
        | Open::DoesNotSupportRequestedMinHistorySize
        | Open::DoesNotSupportRequestedMinSubscriberBorrowedSamples
        | Open::DoesNotSupportRequestedAmountOfPublishers
        | Open::DoesNotSupportRequestedAmountOfSubscribers
        | Open::DoesNotSupportRequestedAmountOfNodes => {
            log::error!(target: "hypha/transport", "hypha_iceoryx2: {cause}: a stale shared-memory segment or a different hypha \
                     build created this service; run `hypha clean` with the domain stopped");
            PortError::QosMismatch
        }
        Open::InsufficientPermissions => PortError::Backend(BACKEND_SERVICE_PERMISSIONS),
        Open::ServiceInCorruptedState => PortError::Backend(BACKEND_SERVICE_CORRUPTED),
        Open::HangsInCreation | Open::IsMarkedForDestruction => {
            PortError::Backend(BACKEND_SERVICE_CONTENDED)
        }
        Open::ExceedsMaxNumberOfNodes => PortError::ResourceLimit,
        Open::Interrupt
        | Open::InternalFailure
        | Open::UnableToCreateServiceTag
        | Open::VersionMismatch => PortError::Backend(0),
    }
}

#[cfg(test)]
#[path = "../tests/support/service_open_errors.rs"]
mod service_open_errors;

#[cfg(test)]
mod window_tests {
    use super::{OpenWindow, SERVICE_OPEN_WINDOW};

    #[test]
    fn a_failure_that_is_not_transient_is_not_retried() {
        let mut window = OpenWindow::start("the test service");
        assert!(!window.retry_after(&"incompatible type", false));
    }

    /// The retries of one open are bounded by one window.
    #[test]
    fn transient_failures_are_retried_only_inside_the_window() {
        let mut window = OpenWindow::start("the test service");
        let started = std::time::Instant::now();
        let mut retries = 0_u32;
        while window.retry_after(&"busy", true) {
            retries = retries.saturating_add(1);
            assert!(retries < 1_000, "the window must end");
        }
        let waited = started.elapsed();
        assert!(retries >= 1, "a transient failure is retried");
        assert!(waited >= SERVICE_OPEN_WINDOW.saturating_sub(std::time::Duration::from_millis(50)));
        assert!(
            waited < SERVICE_OPEN_WINDOW.saturating_add(std::time::Duration::from_millis(500)),
            "{waited:?}"
        );
    }
}
