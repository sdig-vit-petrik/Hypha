//! Recover the silent-listener state of the iceoryx2 0.10.0 race on a data listener.
#![allow(clippy::expect_used)]

mod support;
use support::sealed_send;

use std::sync::atomic::Ordering;
use std::time::Duration;

use hypha_core::port::{
    PortError, TopicSpec,
    dispatch::{AttachmentId, StopReason},
};
use hypha_iceoryx2::waitset::{Dispatcher, EventOutcome, Progression, Trigger};
use hypha_iceoryx2::{Iox2Publisher, Iox2Subscriber, Iox2Transport};
use hypha_testkit::unique_name as unique;
use iceoryx2::prelude::{FileName, SemanticString as _};
use iceoryx2_bb_posix::file::AccessMode;
use iceoryx2_cal::dynamic_storage::{
    DynamicStorage, DynamicStorageBuilder, posix_shared_memory::Storage,
};
use iceoryx2_cal::event::{
    event_state::counting_bit_set::RelocatableCountingBitSet, trigger::State,
};
use iceoryx2_cal::named_concept::{
    NamedConceptBuilder, NamedConceptConfiguration, NamedConceptMgmt,
};

type EventState = State<RelocatableCountingBitSet, ()>;
type EventStorage = Storage<EventState>;
type DataListener = iceoryx2::port::listener::Listener<iceoryx2::service::ipc_threadsafe::Service>;

/// Open only this test listener's state; never acquire ownership of its resource.
fn listener_state(listener: &DataListener) -> EventStorage {
    let config = hypha_iceoryx2::domain::config().expect("the domain configuration");
    let mut suffix = config.global.service.event_connection_suffix;
    suffix.push_bytes(b"_mgmt").expect("state suffix");
    let settings = <EventStorage as NamedConceptMgmt>::Configuration::default()
        .prefix(&config.global.prefix)
        .suffix(&suffix)
        .path_hint(config.global.root_path());
    let name = FileName::new(listener.id().value().to_string().as_bytes()).expect("listener name");
    <EventStorage as DynamicStorage<EventState>>::Builder::new(&name)
        .config(&settings)
        .has_ownership(false)
        .open(AccessMode::ReadWrite)
        .expect("listener state")
}

/// The socket is empty, but producers believe it already carries a wake-up.
fn suppress_wakeups(state: &EventStorage) {
    state.get().notification_state.store(2, Ordering::SeqCst);
}

struct DataDispatcher {
    state: EventStorage,
    publisher: Iox2Publisher,
    subscriber: Iox2Subscriber,
    sent: bool,
    fail: bool,
    received: Vec<Vec<u8>>,
    /// The next turn was started by the audit alone.
    audit_turn: bool,
    /// Samples taken on an audit-started turn: each is a wake the audit recovered.
    recovered: u64,
}

impl Dispatcher for DataDispatcher {
    fn on_audit_turn(&mut self, _: AttachmentId) {
        self.audit_turn = true;
    }

    fn on_event(&mut self, _: AttachmentId) -> EventOutcome {
        let audit_turn = std::mem::take(&mut self.audit_turn);
        match self.subscriber.try_receive() {
            Ok(Some(sample)) => {
                if audit_turn {
                    self.recovered = self.recovered.saturating_add(1);
                }
                self.received.push(sample.payload().to_vec());
                if self.fail {
                    EventOutcome::Fail(PortError::InvalidTopic)
                } else {
                    EventOutcome::Stop
                }
            }
            Ok(None) => EventOutcome::Drained,
            Err(cause) => EventOutcome::Fail(cause),
        }
    }

    fn on_tick(&mut self) -> Progression {
        if !self.sent {
            suppress_wakeups(&self.state);
            let mut loan = self.publisher.loan(4).expect("loan");
            loan.as_mut_bytes().copy_from_slice(b"scan");
            loan.send_frame(sealed_send::metadata(), 0).expect("send despite suppressed socket");
            self.sent = true;
        }
        Progression::Continue
    }
}

#[test]
fn a_silent_data_listener_recovers_without_restarting_the_run() {
    data_recovery(false);
}

#[test]
fn a_recovered_data_callback_can_fail_the_run() {
    data_recovery(true);
}

fn data_recovery(fail: bool) {
    let transport = Iox2Transport::with_name(&unique("wake_data")).expect("transport");
    let spec = TopicSpec::new(&format!("/{}", unique("wake_data"))).with_type("hypha_test/Data");
    let publisher = transport.publish(&spec).expect("publisher");
    let subscriber = transport.subscribe(&spec).expect("subscriber");
    let listener = subscriber.open_listener().expect("open a listener").expect("notified listener");
    let state = listener_state(&listener);
    let mut waitset = transport.waitset(&unique("wake_ctl")).expect("waitset");
    let attachment = waitset.attach(Some(listener), Trigger::EventDriven).expect("attach");
    let mut dispatcher = DataDispatcher {
        state,
        publisher,
        subscriber,
        sent: false,
        fail,
        received: Vec::new(),
        audit_turn: false,
        recovered: 0,
    };
    let report = waitset.run_for(64, Duration::from_millis(500), &mut dispatcher);
    if fail {
        assert_eq!(report.reason, StopReason::RuntimeFailed);
        assert_eq!(report.error, Some(PortError::InvalidTopic));
        assert_eq!(report.attachment, Some(attachment));
    } else {
        assert_eq!(report.reason, StopReason::Requested, "silent data was never dispatched");
    }
    assert_eq!(dispatcher.received, vec![b"scan".to_vec()]);
    assert_eq!(
        dispatcher.recovered, 1,
        "the audit took the sample whose wake was lost, and counts it"
    );
}
