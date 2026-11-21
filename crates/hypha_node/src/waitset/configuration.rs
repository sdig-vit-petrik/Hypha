//! `WaitSet` construction and the C ABI call that hands out a node's `WaitSet`.

use super::{
    Arc, DispatchLimits, Error, HyphaNode, HyphaWaitSet, Registration, Target, Trigger,
    WaitSetShared,
};
use std::cell::RefCell;

/// The callback duration that produces a warning, 10 ms.
const CALLBACK_WARN_NANOS: u64 = 10_000_000;

/// The shortest interval between two warnings for one attachment, 1 s.
const WARNING_INTERVAL_NANOS: u64 = 1_000_000_000;

/// A new `WaitSet` on `node`, which co-owns the node.
fn build(node: Arc<HyphaNode>) -> Result<Box<HyphaWaitSet>, Error> {
    let session: &HyphaNode = &node;
    let native = session.transport.waitset(&control_service_name()).map_err(Error::from)?;
    let shared = Arc::new(WaitSetShared::new(native.stop_handle()));
    let health = Arc::clone(&session.health);
    Ok(Box::new(HyphaWaitSet {
        node,
        native: RefCell::new(native),
        shared,
        registrations: RefCell::new(Vec::new()),
        cycles: RefCell::new(Vec::new()),
        next_cycle: std::cell::Cell::new(1),
        limits: DispatchLimits::default(),
        warn_nanos: CALLBACK_WARN_NANOS,
        warn_interval_nanos: WARNING_INTERVAL_NANOS,
        pending_detach: RefCell::new(Vec::new()),
        live_ids: RefCell::new(Vec::new()),
        input_sources: RefCell::new(Vec::new()),
        health,
    }))
}

/// Where a node keeps its default `WaitSet`.
pub(crate) enum WaitSetSlot {
    /// Not created yet.
    Absent,
    /// Created, and owned here.
    Held(Box<HyphaWaitSet>),
    /// Lent to a Rust owner, a [`WaitSetLease`], which brings it back.
    Lent,
    /// The node was destroyed: a lease that comes back releases its `WaitSet`.
    Closed,
}

impl WaitSetSlot {
    /// The `WaitSet` held here, if one is.
    pub(crate) fn held(&self) -> Option<&HyphaWaitSet> {
        match self {
            Self::Held(set) => Some(set),
            Self::Absent | Self::Lent | Self::Closed => None,
        }
    }

    /// Close the slot for a node that is destroyed, and hand out what it held.
    pub(crate) fn close(&mut self) -> Option<Box<HyphaWaitSet>> {
        match std::mem::replace(self, Self::Closed) {
            Self::Held(set) => Some(set),
            Self::Absent | Self::Lent | Self::Closed => None,
        }
    }

    /// Created with the node's housekeeping on first use. `InvalidState` while it is lent
    /// or after the node was destroyed.
    fn held_or_built(&mut self, node: &Arc<HyphaNode>) -> Result<&HyphaWaitSet, Error> {
        if matches!(self, Self::Absent) {
            let waitset = build(Arc::clone(node))?;
            // Attached once, here, so a repeated run never duplicates it.
            attach_housekeeping(&waitset)?;
            *self = Self::Held(waitset);
        }
        match self {
            Self::Held(set) => Ok(set),
            Self::Absent => Err(Error::Backend),
            Self::Lent | Self::Closed => Err(Error::InvalidState),
        }
    }
}

/// Run `read` on the default `WaitSet` of `node`, created on first use. The node owns
/// it, and `node_destroy` releases it.
///
/// # Errors
/// The error of its creation, or `InvalidState` while a [`WaitSetLease`] holds it.
pub fn with_default_waitset<R>(
    node: &Arc<HyphaNode>,
    read: impl FnOnce(&HyphaWaitSet) -> R,
) -> Result<R, Error> {
    let mut slot = node.default_waitset.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let set = slot.held_or_built(node)?;
    Ok(read(set))
}

/// The default `WaitSet` of a node, lent to one Rust owner until it drops the lease.
/// What is attached to it stays.
pub struct WaitSetLease<'node> {
    node: &'node HyphaNode,
    set: Option<Box<HyphaWaitSet>>,
}

impl WaitSetLease<'_> {
    /// The lent `WaitSet`.
    ///
    /// # Errors
    /// `InvalidState` once the lease gave it back.
    pub fn waitset(&self) -> Result<&HyphaWaitSet, Error> {
        self.set.as_deref().ok_or(Error::InvalidState)
    }
}

impl Drop for WaitSetLease<'_> {
    fn drop(&mut self) {
        let Some(set) = self.set.take() else {
            return;
        };
        let mut slot = crate::locked(&self.node.default_waitset);
        let returned = matches!(*slot, WaitSetSlot::Lent);
        if returned {
            *slot = WaitSetSlot::Held(set);
            return;
        }
        drop(slot);
        drop(set);
    }
}

/// Lend the default `WaitSet` of `node`, created on first use, to the caller until the
/// lease drops. One lease at a time.
///
/// # Errors
/// The error of its creation, or `InvalidState` while another lease holds it.
pub fn node_default_waitset(node: &Arc<HyphaNode>) -> Result<WaitSetLease<'_>, Error> {
    let mut slot = node.default_waitset.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    slot.held_or_built(node)?;
    match std::mem::replace(&mut *slot, WaitSetSlot::Lent) {
        WaitSetSlot::Held(set) => Ok(WaitSetLease { node, set: Some(set) }),
        other => {
            *slot = other;
            Err(Error::InvalidState)
        }
    }
}

/// Attach this node's own housekeeping to its default `WaitSet`.
///
/// Latched replay progresses only while a `WaitSet` runs.
fn attach_housekeeping(waitset: &HyphaWaitSet) -> Result<(), Error> {
    // A subscriber created before this listener exists fired its graph event
    // with no listener to hear it. One replay now covers those joins.
    let transport = &waitset.node().transport;
    let graph = transport.graph_listener().map_err(Error::from)?;
    attach_framework(waitset, Some(graph), Trigger::Event, Target::LatchReplay)?;
    transport.replay_latched();

    attach_sim_clock(waitset)?;
    Ok(())
}

fn attach_sim_clock(waitset: &HyphaWaitSet) -> Result<(), Error> {
    let node = waitset.node();
    if !node.uses_sim_time {
        return Ok(());
    }
    // A sample of either input wakes this WaitSet, which then drains both.
    attach_input_listener(waitset, &node.sim_clock_subscriber)?;
    attach_input_listener(waitset, &node.progress_subscriber)
}

/// Attach the listener of one framework input, so that its samples wake this `WaitSet`.
fn attach_input_listener(
    waitset: &HyphaWaitSet,
    subscriber_mutex: &std::sync::Mutex<Option<Box<crate::subscriber::HyphaSubscriber>>>,
) -> Result<(), Error> {
    let subscriber_guard = crate::locked(subscriber_mutex);
    let subscriber = subscriber_guard.as_deref().ok_or(Error::InvalidState)?;
    let Some(listener) = subscriber.port.open_listener().map_err(Error::from)? else {
        return Err(Error::InvalidState);
    };
    attach_framework(waitset, Some(listener), Trigger::Event, Target::SimClock)
}

/// Attach one framework registration with no caller context.
fn attach_framework(
    waitset: &HyphaWaitSet,
    listener: Option<hypha_iceoryx2::Iox2Listener>,
    trigger: Trigger,
    target: Target,
) -> Result<(), Error> {
    let mut registrations =
        waitset.registrations.try_borrow_mut().map_err(|_| Error::InvalidState)?;
    let id = waitset.native_attach(listener, trigger)?;
    registrations.push(Registration::new(id, target));
    Ok(())
}

/// A process-local control service name that no two `WaitSet`s share.
fn control_service_name() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let ordinal = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    format!("hypha/hk/waitset_ctl/{}_{nanos}_{ordinal}", std::process::id())
}
