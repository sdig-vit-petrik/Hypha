//! A plain client's handle: create, call, wait for a server, status, destroy.

use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use hypha_iceoryx2::Iox2Listener;

use super::super::call_log::CallLog;
use super::super::outcome::CallStatus;
use super::super::port::ClientPort;
use super::super::spec::ServiceDecl;
use super::{CallDone, ClientCore, open_state};
use crate::node::HyphaNode;
use crate::status::Error;

/// A client, owned by its caller. Dropping it drops the outcomes of its calls in flight
/// without calling them: their callbacks belong to an owner that is gone.
pub struct HyphaClient {
    core: Arc<ClientCore>,
    node: Weak<HyphaNode>,
    /// The registration on the node's `WaitSet`; `None` for a blocking client, which owns
    /// its listener instead.
    attachment: Option<crate::waitset::HyphaAttachmentId>,
    blocking: Option<Blocking>,
}

/// A blocking client's own wait: its response listener, and the lock that makes its calls
/// one at a time.
struct Blocking {
    listener: Iox2Listener,
    turn: Mutex<()>,
}

/// What `client_status` reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientStatus {
    /// A live server of the client's stream exists now.
    pub ready: bool,
    /// Calls whose outcome is still to come.
    pub pending: usize,
    /// Records the call log dropped: no free sample, or a record above its bound.
    pub log_dropped: u64,
}

/// Create a client of `decl` on `node`. A `blocking` client is attached to nothing; any
/// other is attached to the node's `WaitSet`, whose run delivers its outcomes. A server
/// need not exist.
///
/// # Errors
/// The errors of the declaration (`InvalidArgument`, `InvalidTopic`); `ResourceLimit` when
/// the clients of the stream are at their limit; `QosMismatch` for a stream of another
/// policy; `InvalidState` during a run; the transport's error otherwise. Each is logged
/// with its reason and recorded for `last_error`.
pub fn service_client_create(
    node: &Arc<HyphaNode>,
    decl: &ServiceDecl<'_>,
    blocking: bool,
) -> Result<HyphaClient, Error> {
    crate::last_error::clear();
    let state = open_state(node, decl, None, None)?;
    let listener = match state.port.as_ref() {
        Some(ClientPort::Bus(port)) => port.open_listener().map_err(Error::from)?,
        _ => return Err(Error::InvalidState),
    };
    let core = Arc::new(ClientCore { state: Mutex::new(state) });
    let mut client =
        HyphaClient { core, node: Arc::downgrade(node), attachment: None, blocking: None };
    if blocking {
        client.blocking = Some(Blocking { listener, turn: Mutex::new(()) });
    } else {
        let target = crate::waitset::ServiceTarget::Client(Arc::clone(&client.core));
        client.attachment = Some(crate::waitset::attach_service(node, listener, target)?);
    }
    Ok(client)
}

/// Destroy `client`: it is detached from its `WaitSet`, and the outcomes of its calls in
/// flight are dropped uncalled.
pub fn service_client_destroy(client: HyphaClient) {
    drop(client);
}

impl Drop for HyphaClient {
    fn drop(&mut self) {
        let (calls, ready) = {
            let mut state = crate::locked(&self.core.state);
            state.port = None;
            (std::mem::take(&mut state.calls), std::mem::take(&mut state.ready))
        };
        let dropped = calls.len().saturating_add(ready.len());
        if dropped > 0 {
            log::info!(target: "hypha", "hypha: a client was destroyed with {dropped} calls in flight; their outcomes are dropped");
        }
        if let (Some(attachment), Some(node)) = (self.attachment, self.node.upgrade()) {
            crate::waitset::detach_service(&node, attachment);
        }
    }
}

/// Call with `request`. `done` receives the call's one outcome: on the `WaitSet` thread at
/// a later turn, or, for a blocking client, on this thread before the call returns. A call
/// refused as `Busy` or `TooLarge` sends nothing and drops `done` uncalled.
///
/// # Errors
/// `InvalidState` for a blocking call on a thread that runs a `WaitSet` dispatch: that
/// thread would have to deliver the answer it waits for (FSR-43). `done` drops uncalled.
pub fn client_call(
    client: &HyphaClient,
    request: &[u8],
    done: Box<dyn CallDone>,
) -> Result<CallStatus, Error> {
    let stamp = hypha_time::now().nanos();
    let Some(blocking) = client.blocking.as_ref() else {
        let now = hypha_time::scheduling_now().nanos();
        return Ok(crate::locked(&client.core.state).call(request, Some(done), now, stamp));
    };
    refuse_in_dispatch("callAndWait")?;
    let _one_at_a_time = crate::locked(&blocking.turn);
    let now = hypha_time::scheduling_now().nanos();
    let status = crate::locked(&client.core.state).call(request, Some(done), now, stamp);
    if status != CallStatus::Sent {
        return Ok(status);
    }
    let node = client.node.upgrade();
    let transport = node.as_deref().map(|node| &node.transport);
    loop {
        let (finished, next) = {
            let mut state = crate::locked(&client.core.state);
            let finished = state.drive(hypha_time::scheduling_now().nanos(), transport, None);
            (finished, state.next_deadline())
        };
        if !finished.is_empty() {
            for (done, result, _) in finished {
                if let Some(done) = done {
                    let _ = done.done(&result);
                }
            }
            return Ok(CallStatus::Sent);
        }
        let wait = next.map_or(Duration::from_millis(10), |deadline| {
            let left = deadline.saturating_sub(hypha_time::scheduling_now().nanos()).max(0);
            Duration::from_nanos(left.unsigned_abs()).min(Duration::from_millis(10))
        });
        // A response wakes the wait; a server that appears is seen at the next slice.
        let _ = blocking.listener.timed_wait(|_| {}, wait.max(Duration::from_micros(100)));
    }
}

/// Wait up to `timeout` for a live server of a blocking client's stream.
///
/// # Errors
/// `InvalidState` for a client attached to a `WaitSet`, or on a thread that runs a dispatch.
pub fn client_wait_for_server(client: &HyphaClient, timeout: Duration) -> Result<bool, Error> {
    if client.blocking.is_none() {
        return crate::last_error::refuse(
            Err(Error::InvalidState),
            "waitForServer is for a blocking client (hypha::blocking); a client in a node calls and reads the outcome",
        );
    }
    refuse_in_dispatch("waitForServer")?;
    let started = std::time::Instant::now();
    loop {
        if client_status(client).ready {
            return Ok(true);
        }
        if started.elapsed() >= timeout {
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(1).min(timeout.saturating_sub(started.elapsed())));
    }
}

/// Whether a live server exists now, and how many calls are pending.
#[must_use]
pub fn client_status(client: &HyphaClient) -> ClientStatus {
    let state = crate::locked(&client.core.state);
    ClientStatus {
        ready: state.port.as_ref().is_some_and(ClientPort::server_available),
        pending: state.calls.len().saturating_add(state.ready.len()),
        log_dropped: state.log.as_ref().map_or(0, CallLog::dropped),
    }
}

fn refuse_in_dispatch(what: &str) -> Result<(), Error> {
    if !crate::waitset::in_dispatch() {
        return Ok(());
    }
    Err(crate::last_error::refuse(
        Error::InvalidState,
        &format!(
            "{what} on the run thread would wait for the thread that must deliver its answer; \
             use call(request, onResult) of a client of the node in a callback"
        ),
    ))
}
