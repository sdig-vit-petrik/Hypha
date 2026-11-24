//! Simulated time going back ends the process; nothing rewinds in place.
//!
//! A `/clock` sample lower than the node's last one, and a rewound timer or cycle, end
//! the run with `ClockRewound`; `hypha launch` restarts
//! the process, which waits on its first node ([`rejoin`]) for a `/clock` sample of the
//! new timeline before it opens any port of its own.

use std::sync::PoisonError;
use std::time::{Duration, Instant};

use hypha_runtime::clock_barrier::NAMED_START_TIMEOUT;
use hypha_runtime::process_restart::{REJOIN_ENV, RestartState, parse_rejoin_marker};

use crate::node::HyphaNode;
use crate::node_progress::{self, ClockRead};
use crate::status::{Error, HyphaStatus};
use crate::subscriber::ReceiveFailure;

/// How often the rejoin looks for the next `/clock` sample.
const REJOIN_POLL: Duration = Duration::from_millis(1);

/// The rewinds this process has refused. The rules are in `hypha_runtime::process_restart`.
static RESTART: RestartState = RestartState::new();

/// Refuse a rewind of simulated time to `now_nanos` seen by `subject` (a cycle or a timer).
pub(crate) fn refuse_rewind(subject: &str, now_nanos: i64) -> HyphaStatus {
    log::error!(target: "hypha", "hypha: {subject} saw simulated time go back to {now_nanos} ns: a running cycle never rewinds; the process exits 75 for a backward seek");
    HyphaStatus::ClockRewound
}

/// Refuse a `/clock` sample of `to_nanos` that `node` read after it applied `from_nanos`.
pub(crate) fn refuse_clock_rewind(node: &str, from_nanos: u64, to_nanos: u64) -> HyphaStatus {
    log::error!(target: "hypha", "hypha: '{node}' saw /clock go back from {from_nanos} ns to {to_nanos} ns: nothing rewinds in place; the process exits 75 for a backward seek");
    HyphaStatus::ClockRewound
}

/// In a restarted process, wait on `node`'s first creation for a `/clock` sample sent
/// after the one that rewound the old process, and start the node's timeline there.
/// Older `/clock` samples are dropped. Nothing to do in a process that did not restart.
///
/// # Errors
/// `InvalidArgument` for a malformed marker, `Timeout` when no such sample comes within
/// [`NAMED_START_TIMEOUT`], `InvalidState` when the process is asked to stop first, and
/// the status of a `/clock` sample that cannot be applied.
pub(crate) fn rejoin(node: &HyphaNode) -> Result<(), Error> {
    let Some(marker) = std::env::var_os(REJOIN_ENV) else {
        return Ok(());
    };
    if !RESTART.claim_rejoin() {
        return Ok(());
    }
    let Some(after) = marker.to_str().and_then(parse_rejoin_marker) else {
        log::error!(target: "hypha", "hypha: {REJOIN_ENV} is not a send time: {}", marker.to_string_lossy());
        return Err(Error::InvalidArgument);
    };
    let started = Instant::now();
    loop {
        if let Some(read) = clock_sent_after(node, after)? {
            node_progress::record_rejoin(node, read);
            log::info!(target: "hypha", "hypha: '{}' rejoins the replay at {} ns", node.name, read.nanos);
            return Ok(());
        }
        if crate::process_stop::stop_requested() {
            log::error!(target: "hypha", "hypha: '{}' was asked to stop while it waited to rejoin the replay", node.name);
            return Err(Error::InvalidState);
        }
        if started.elapsed() >= NAMED_START_TIMEOUT {
            log::error!(target: "hypha", "hypha: '{}' restarted for a backward seek, but no new /clock came within {NAMED_START_TIMEOUT:?}", node.name);
            return Err(Error::Timeout);
        }
        std::thread::sleep(REJOIN_POLL);
    }
}

/// Apply the first queued `/clock` sample sent after `after` and drop the ones before it.
fn clock_sent_after(node: &HyphaNode, after: u64) -> Result<Option<ClockRead>, Error> {
    let mut guard = node.sim_clock_subscriber.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(subscriber) = guard.as_deref_mut() else {
        return Err(Error::InvalidState);
    };
    loop {
        match subscriber.receive_validated() {
            Ok(Some(sample)) if sample.header().metadata.transport_sent_nanos > after => {
                return crate::waitset::apply_sim_clock_sample(&sample).map(Some);
            }
            Ok(Some(_)) | Err(ReceiveFailure::Refused(_)) => {}
            Ok(None) => return Ok(None),
            Err(ReceiveFailure::Transport(status)) => return Err(status),
        }
    }
}
