//! The participants' reports: an attachment of the player's `WaitSet` queues them and
//! ends the current run, so a report wakes a wait at once instead of being found by a poll.

use crate::lock;
use hypha_node::sdk::waitset::Delivery;
use hypha_node::sdk::{RawSubscriber, WaitSet};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// The report payloads the attachment has queued and the drain has not read yet.
#[derive(Debug, Default)]
pub(super) struct ReportQueue {
    payloads: VecDeque<Vec<u8>>,
    /// Payloads dropped because the queue was full, since the last drain.
    dropped: u64,
    /// A frame that was no hypha frame, since the last drain.
    malformed: bool,
}

/// The most reports the queue holds between two drains: the player's input depth for
/// each of the 32 publishers a framework topic admits.
const QUEUE_LIMIT: usize = hypha_node::sdk::clock_barrier::PROGRESS_DEPTH * 32;

/// The attachment callback: queue each report, dropping (and counting) beyond the limit.
struct QueueReports {
    queue: Arc<Mutex<ReportQueue>>,
}

impl QueueReports {
    fn push(&self, delivery: &Delivery<'_>) {
        let mut queue = lock(&self.queue);
        let Some(payload) = delivery.payload else {
            queue.malformed = true;
            return;
        };
        let full = queue.payloads.len() >= QUEUE_LIMIT;
        if full {
            queue.dropped = queue.dropped.saturating_add(1);
            return;
        }
        queue.payloads.push_back(payload.to_vec());
    }
}

/// Attach `subscriber` to `waitset` so each report is queued as it arrives.
pub(super) fn attach(
    waitset: &mut WaitSet<'_>,
    subscriber: RawSubscriber,
) -> Result<Arc<Mutex<ReportQueue>>, String> {
    let queue = Arc::new(Mutex::new(ReportQueue::default()));
    let callback = QueueReports { queue: Arc::clone(&queue) };
    // The attachment takes a closure; this one only forwards to the named functor.
    waitset.attach_waking_subscription(subscriber, move |delivery| {
        callback.push(delivery);
        Ok(())
    })?;
    Ok(queue)
}

/// The queued payloads, oldest first, and the reports dropped since the last take.
pub(super) struct Taken {
    pub(super) payloads: VecDeque<Vec<u8>>,
    pub(super) dropped: u64,
    pub(super) malformed: bool,
}

/// Take everything `queue` holds.
pub(super) fn take(queue: &Mutex<ReportQueue>) -> Taken {
    let mut queue = lock(queue);
    Taken {
        payloads: std::mem::take(&mut queue.payloads),
        dropped: std::mem::take(&mut queue.dropped),
        malformed: std::mem::take(&mut queue.malformed),
    }
}
