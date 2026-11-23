//! The meaning of each iceoryx2 open failure, kept by the private mapping.

use hypha_core::port::PortError;
use iceoryx2::service::builder::publish_subscribe::PublishSubscribeOpenError as Open;

use super::{
    BACKEND_SERVICE_CONTENDED, BACKEND_SERVICE_CORRUPTED, BACKEND_SERVICE_PERMISSIONS,
    half_created_advice, hangs_in_creation, publish_subscribe_open_is_transient,
    publish_subscribe_open_port_error,
};

#[test]
fn existing_service_overlap_states_are_transient() {
    assert!(publish_subscribe_open_is_transient(Open::HangsInCreation));
    assert!(publish_subscribe_open_is_transient(Open::IsMarkedForDestruction));
    assert!(!publish_subscribe_open_is_transient(Open::IncompatibleTypes));
}

#[test]
fn existing_service_failures_keep_their_meaning() {
    assert_eq!(
        publish_subscribe_open_port_error(Open::HangsInCreation),
        PortError::Backend(BACKEND_SERVICE_CONTENDED)
    );
    assert_eq!(
        publish_subscribe_open_port_error(Open::InsufficientPermissions),
        PortError::Backend(BACKEND_SERVICE_PERMISSIONS)
    );
    assert_eq!(
        publish_subscribe_open_port_error(Open::ServiceInCorruptedState),
        PortError::Backend(BACKEND_SERVICE_CORRUPTED)
    );
    assert_eq!(
        publish_subscribe_open_port_error(Open::ExceedsMaxNumberOfNodes),
        PortError::ResourceLimit
    );
    assert_eq!(publish_subscribe_open_port_error(Open::DoesNotExist), PortError::NotAvailable);
    assert_eq!(publish_subscribe_open_port_error(Open::IncompatibleTypes), PortError::QosMismatch);
    assert_eq!(
        publish_subscribe_open_port_error(Open::DoesNotSupportRequestedMinBufferSize),
        PortError::QosMismatch
    );
    assert_eq!(
        publish_subscribe_open_port_error(Open::DoesNotSupportRequestedAmountOfPublishers),
        PortError::QosMismatch
    );
    assert_eq!(publish_subscribe_open_port_error(Open::InternalFailure), PortError::Backend(0));
}

#[test]
fn an_exhausted_half_created_service_says_how_to_recover() {
    use iceoryx2::service::builder::publish_subscribe::PublishSubscribeOpenOrCreateError as OpenOrCreate;

    let hangs = OpenOrCreate::PublishSubscribeOpenError(Open::HangsInCreation);
    assert!(hangs_in_creation(hangs));
    assert!(!hangs_in_creation(OpenOrCreate::PublishSubscribeOpenError(Open::IncompatibleTypes)));
    let advice = half_created_advice(true);
    assert!(advice.contains("looks half-created or its metadata under"), "{advice}");
    assert!(advice.contains("/services is damaged"), "{advice}");
    assert!(advice.contains("restart its publisher or run ./clean-shm.sh"), "{advice}");
    let found = half_created_advice(false);
    assert!(found.is_empty(), "{found:?}");
}
