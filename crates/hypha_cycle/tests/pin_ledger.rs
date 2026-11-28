//! Which samples read in place keep their transport slot (#255), decided on fake slots:
//! a spent sample gives its slot back, the newest that a `Latest` binding reads stays, and
//! the oldest are copied out until a slot is free for the receive and the sample in hand.
#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "a failed setup is a test failure"
)]

use std::sync::Arc;

use hypha_cycle::{PinLedger, PinReaders, PinnedBytes, PinnedPayload, SamplePayload};

struct Slot(Vec<u8>);

impl PinnedBytes for Slot {
    fn bytes(&self) -> &[u8] {
        &self.0
    }
}

/// `count` samples in place, tracked oldest first; each holds its index as its byte.
fn pinned(ledger: &mut PinLedger, count: u8) -> Vec<Arc<PinnedPayload>> {
    (0..count)
        .map(|index| {
            let pin = PinnedPayload::new(Box::new(Slot(vec![index])));
            ledger.track(&pin);
            pin
        })
        .collect()
}

fn readers(queued: u32, latest: bool, holding: bool) -> PinReaders {
    PinReaders { queued, latest, holding, slots: 4 }
}

fn bytes(pin: &Arc<PinnedPayload>) -> Vec<u8> {
    SamplePayload::pinned(Arc::clone(pin)).to_vec()
}

#[test]
fn a_sample_every_queued_binding_took_gives_its_slot_back_and_reads_as_no_bytes() {
    let mut ledger = PinLedger::default();
    let pins = pinned(&mut ledger, 2);
    SamplePayload::pinned(Arc::clone(&pins[0])).mark_taken();
    assert_eq!(ledger.free_a_slot(pins.last(), readers(1, false, false)), 0, "nothing copied");
    assert_eq!(ledger.in_place(), 1);
    assert!(!pins[0].in_place() && bytes(&pins[0]).is_empty(), "released, not copied");
    assert!(pins[1].in_place(), "not taken yet");
}

#[test]
fn a_sample_is_spent_only_once_every_queued_binding_took_it() {
    let mut ledger = PinLedger::default();
    let pins = pinned(&mut ledger, 1);
    SamplePayload::pinned(Arc::clone(&pins[0])).mark_taken();
    ledger.free_a_slot(None, readers(2, false, false));
    assert!(pins[0].in_place(), "one of two bindings took it");
    SamplePayload::pinned(Arc::clone(&pins[0])).mark_taken();
    ledger.free_a_slot(None, readers(2, false, false));
    assert!(!pins[0].in_place());
}

#[test]
fn the_newest_sample_stays_for_a_latest_binding_unless_a_newer_one_is_in_hand() {
    for (holding, kept) in [(false, true), (true, false)] {
        let mut ledger = PinLedger::default();
        let pins = pinned(&mut ledger, 1);
        // No queued binding: every sample is taken by all of them.
        ledger.free_a_slot(pins.last(), readers(0, true, holding));
        assert_eq!(pins[0].in_place(), kept, "holding {holding}");
    }
}

#[test]
fn the_oldest_are_copied_out_until_a_slot_is_free_for_the_receive_and_the_sample_in_hand() {
    // Four slots: the receive needs one, a sample in hand another, so two may stay pinned.
    for (holding, copied) in [(false, 1), (true, 2)] {
        let mut ledger = PinLedger::default();
        let pins = pinned(&mut ledger, 4);
        assert_eq!(ledger.free_a_slot(pins.last(), readers(1, false, holding)), copied);
        assert_eq!(ledger.in_place(), 4 - copied, "holding {holding}");
        for (index, pin) in pins.iter().enumerate() {
            assert_eq!(pin.in_place(), index >= copied, "sample {index}");
            assert_eq!(bytes(pin), [u8::try_from(index).expect("small")], "bytes kept");
        }
    }
}

#[test]
fn a_dropped_or_already_copied_sample_leaves_the_ledger() {
    let mut ledger = PinLedger::default();
    let mut pins = pinned(&mut ledger, 3);
    assert!(pins[1].copy_out());
    pins.remove(0);
    assert_eq!(ledger.in_place(), 1);
    assert_eq!(ledger.free_a_slot(None, readers(1, false, false)), 0);
    assert!(pins[1].in_place());
}
