//! Frames queued for one client, and the handle the server keeps for each client.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// One payload retained per channel so a client subscribing between two
/// publishes of a slow topic still gets data immediately.
#[derive(Clone)]
pub(super) struct LatchedSample {
    pub(super) timestamp_ns: u64,
    pub(super) payload: Arc<[u8]>,
}

/// One outgoing frame. A live sample or time frame goes stale; a frame that a
/// client needs to decode the stream (an advertisement, a latched replay) does not.
pub(super) struct QueuedFrame {
    pub(super) bytes: Vec<u8>,
    pub(super) stale_at: Option<Instant>,
}

impl QueuedFrame {
    pub(super) fn lasting(bytes: Vec<u8>) -> Self {
        Self { bytes, stale_at: None }
    }

    pub(super) fn live(bytes: Vec<u8>, max_age: Duration) -> Self {
        Self { bytes, stale_at: Instant::now().checked_add(max_age) }
    }

    pub(super) fn is_stale(&self, now: Instant) -> bool {
        self.stale_at.is_some_and(|stale_at| now >= stale_at)
    }
}

pub(super) struct ClientHandle {
    pub(super) tx: Sender<QueuedFrame>,
    pub(super) subscriptions: HashMap<u32, u32>, // sub_id -> channel_id
    pub(super) buffered_bytes: Arc<AtomicUsize>,
    pub(super) dropped: Arc<AtomicUsize>,
}

pub(crate) fn epoch_nanos_now() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => u64::try_from(d.as_nanos()).unwrap_or(u64::MAX),
        Err(_) => 0,
    }
}

/// Bytes one client may have queued before a new frame for it is dropped.
const SEND_BUFFER_LIMIT_BYTES: usize = 64 * 1024 * 1024;

/// Enqueues `frame` onto `handle`'s outgoing channel, dropping it instead if
/// that would push the client's buffered bytes past [`SEND_BUFFER_LIMIT_BYTES`].
pub(super) fn try_enqueue(handle: &ClientHandle, frame: QueuedFrame) {
    let frame_len = frame.bytes.len();
    let current = handle.buffered_bytes.load(Ordering::Relaxed);
    if current.saturating_add(frame_len) > SEND_BUFFER_LIMIT_BYTES {
        handle.dropped.fetch_add(1, Ordering::Relaxed);
        return;
    }
    handle.buffered_bytes.fetch_add(frame_len, Ordering::Relaxed);
    let _ = handle.tx.send(frame);
}
