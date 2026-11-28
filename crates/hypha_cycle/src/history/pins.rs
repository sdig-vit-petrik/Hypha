//! Which samples read in place keep their transport slot (#255): the ledger of one source.
//!
//! A source reads a large sample in its subscriber's slot instead of copying it into the
//! history. The subscriber has few slots, so before each receive the ledger gives back the
//! slots of samples no binding reads again, then copies out the oldest until a slot is free.

use std::sync::{Arc, Weak};

use super::PinnedPayload;

/// The samples of one source that are still in place, oldest first.
#[derive(Default)]
pub struct PinLedger {
    pins: Vec<Weak<PinnedPayload>>,
}

/// What a source knows of its bindings and its drain when it frees a slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PinReaders {
    /// The queued bindings that read the source: a sample is spent once each took it.
    pub queued: u32,
    /// True when a `Latest` binding reads the source: it reads the newest sample again.
    pub latest: bool,
    /// True when the drain holds a newer sample in hand, so the newest pinned one is not
    /// the one a `Latest` binding reads next.
    pub holding: bool,
    /// The slots of the subscriber.
    pub slots: usize,
}

impl PinLedger {
    /// Track `payload`, read in place.
    pub fn track(&mut self, payload: &Arc<PinnedPayload>) {
        self.pins.push(Arc::downgrade(payload));
    }

    /// The samples tracked whose bytes are still in their slot.
    #[must_use]
    pub fn in_place(&self) -> usize {
        self.pins.iter().filter(|pin| pin.upgrade().is_some_and(|pin| pin.in_place())).count()
    }

    /// True when no sample is in place.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pins.is_empty()
    }

    /// Before a receive: give back the slot of each spent sample (every queued binding took
    /// it, and it is not the newest that a `Latest` binding still reads), then copy out the
    /// oldest until a slot is free for the receive and for the sample in hand. `newest` is
    /// the history's newest sample. Returns the samples copied out.
    pub fn free_a_slot(
        &mut self,
        newest: Option<&Arc<PinnedPayload>>,
        readers: PinReaders,
    ) -> usize {
        self.pins.retain(|pin| pin.upgrade().is_some_and(|pin| pin.in_place()));
        if self.pins.is_empty() {
            return 0;
        }
        self.pins.retain(|pin| {
            let Some(pin) = pin.upgrade() else {
                return false;
            };
            let is_newest = newest.is_some_and(|newest| Arc::ptr_eq(newest, &pin));
            let held_by_latest = readers.latest && is_newest && !readers.holding;
            let spent = pin.taken() >= readers.queued && !held_by_latest;
            !(spent && pin.release())
        });
        let in_hand = usize::from(readers.holding);
        let mut copied = 0_usize;
        while self.pins.len().saturating_add(in_hand).saturating_add(1) > readers.slots {
            let oldest = self.pins.remove(0);
            if oldest.upgrade().is_some_and(|pin| pin.copy_out()) {
                copied = copied.saturating_add(1);
            }
        }
        copied
    }
}
