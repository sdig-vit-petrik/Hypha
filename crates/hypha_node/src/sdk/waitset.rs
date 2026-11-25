//! A `WaitSet` the application owns and runs on a thread it created.
//!
//! The framework creates no thread. Creating it starts nothing: call [`WaitSet::run`] or
//! [`WaitSet::run_for`] on the thread that should execute it. One thread runs it at a time.
//! The group borrows its node and cannot outlive it:
//!
//! ```compile_fail
//! use hypha_node::sdk::{BusNode, WaitSet};
//!
//! let group = {
//!     let bus = BusNode::new("worker")?;
//!     WaitSet::new(&bus)?
//! };
//! let _ = group;
//! # Ok::<(), String>(())
//! ```

use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::sdk::ffi;
use crate::sdk::loan::{RawSubscriber, SampleIdentity};
use crate::sdk::raw::BusNode;
use crate::sdk::status::{named_error, status_error};

/// Why a run returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// The application asked for the stop.
    Requested,
    /// A bounded run reached its time limit. No stop was asked for.
    Deadline,
    /// Building the native attachments failed. No callback ran.
    SetupFailed,
    /// The run failed after it started.
    RuntimeFailed,
}

impl StopReason {
    fn from_reason(reason: u32) -> Self {
        match reason {
            ffi::HYPHA_RUN_REASON_SETUP_FAILED => Self::SetupFailed,
            ffi::HYPHA_RUN_REASON_RUNTIME_FAILED => Self::RuntimeFailed,
            ffi::HYPHA_RUN_REASON_DEADLINE => Self::Deadline,
            _ => Self::Requested,
        }
    }
}

/// The outcome of one blocking run.
///
/// The call itself can be refused, and a call that completed can still report that
/// execution failed.
#[derive(Debug, Clone)]
#[must_use]
pub struct RunResult {
    /// The status of the call itself. `0` means the call was accepted.
    pub call_status: i32,
    /// Why the run returned.
    pub reason: StopReason,
    /// The execution status. Nonzero only when execution failed.
    pub status: i32,
    /// The attachment the failure belongs to. Zero when it belongs to none.
    pub failed_attachment: u64,
    /// Callbacks that ran longer than the configured warning threshold.
    pub slow_callbacks: u64,
    /// The message of the callback whose `Err` failed the run, if one did.
    pub callback_error: Option<String>,
}

impl RunResult {
    /// True only when the call succeeded and the run stopped as requested.
    #[must_use]
    pub fn ok(&self) -> bool {
        let stopped_normally = matches!(self.reason, StopReason::Requested | StopReason::Deadline);
        self.call_status == ffi::HYPHA_STATUS_OK && stopped_normally
    }

    /// The failure as an error message, or `None` when the run went well.
    ///
    /// This is the form to use with `?`: `result.err().map_or(Ok(()), Err)`.
    #[must_use]
    pub fn err(&self) -> Option<String> {
        if self.ok() {
            return None;
        }
        if self.call_status != ffi::HYPHA_STATUS_OK {
            return Some(status_error("the run was refused", self.call_status));
        }
        if let Some(message) = &self.callback_error {
            return Some(format!(
                "the run failed: attachment {}: {message}",
                self.failed_attachment
            ));
        }
        Some(status_error("the run failed", self.status))
    }

    fn api_failure(status: i32) -> Self {
        Self {
            call_status: status,
            reason: StopReason::SetupFailed,
            status,
            failed_attachment: 0,
            slow_callbacks: 0,
            callback_error: None,
        }
    }

    fn from_report(report: &ffi::HyphaWaitSetRunReport) -> Self {
        Self {
            call_status: ffi::HYPHA_STATUS_OK,
            reason: StopReason::from_reason(report.reason),
            status: report.status,
            failed_attachment: report.attachment,
            slow_callbacks: report.slow_callbacks,
            callback_error: None,
        }
    }
}

/// The node's default `WaitSet`: the one that carries its housekeeping. It borrows the
/// node and cannot outlive it. One handle per node at a time.
pub struct WaitSet<'node> {
    lease: ffi::WaitSetLease<'node>,
    /// The newest callback `Err` message, shared with every attachment context.
    failure: Arc<Mutex<Option<String>>>,
}

/// One sample a subscription attachment delivers. It borrows the delivery and is
/// valid only during the callback.
#[derive(Debug)]
pub struct Delivery<'a> {
    /// The message payload, or `None` for a frame that did not parse as a hypha
    /// frame. Its identity still names the topic.
    pub payload: Option<&'a [u8]>,
    /// Topic, schema hash and send times of the frame.
    pub identity: SampleIdentity<'a>,
}

/// One registration of a [`WaitSet`], returned by an attach.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AttachmentId(u64);

impl AttachmentId {
    /// The native id, the one [`RunResult::failed_attachment`] reports.
    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }
}

type SubscriptionCallback = Box<dyn FnMut(&Delivery<'_>) -> Result<(), String> + Send>;

/// What a subscription registration owns: the callback, and the slot that carries a
/// callback error back to the run.
struct SubscriptionContext {
    callback: SubscriptionCallback,
    failure: Arc<Mutex<Option<String>>>,
    /// Whether each sample the callback took ends the run of its `WaitSet` (a wake).
    ends_run: bool,
}

impl ffi::SubscriptionHandler for SubscriptionContext {
    fn deliver(
        &mut self,
        delivery: &ffi::SampleDelivery<'_>,
        waitset: &ffi::HyphaWaitSet,
    ) -> ffi::CallbackAction {
        let view = Delivery { payload: Some(delivery.payload), identity: delivery.identity };
        let message = match (self.callback)(&view) {
            Ok(()) => {
                // The batch goes on, so every queued sample is delivered in this turn; the
                // run then ends as a requested stop.
                if self.ends_run {
                    let _requested = ffi::waitset_request_stop(waitset);
                }
                return ffi::CallbackAction::Continue;
            }
            Err(message) => message,
        };
        let mut slot = crate::locked(&self.failure);
        *slot = Some(message);
        drop(slot);
        ffi::CallbackAction::Fail(ffi::Error::Backend)
    }
}

impl<'node> WaitSet<'node> {
    /// The default `WaitSet` of `node`, created on first use. This starts nothing.
    ///
    /// # Errors
    /// Returns a message when the library refuses the creation, or for a second handle
    /// on the same node while this one lives.
    pub fn new(node: &'node BusNode) -> Result<Self, String> {
        ffi::node_default_waitset(&node.node)
            .map(|lease| Self { lease, failure: Arc::default() })
            .map_err(|error| named_error("cannot create the WaitSet", error))
    }

    fn set(&self) -> Result<&ffi::HyphaWaitSet, i32> {
        self.lease.waitset().map_err(ffi::status_code)
    }

    /// Run `callback` for each sample `subscriber` takes, inside a run, at the
    /// moments `trigger` names.
    ///
    /// The attachment owns the subscriber and the callback until
    /// [`WaitSet::detach`] or the end of the node. An `Err` from the callback stops
    /// the run as a failure. Refused while the `WaitSet` runs.
    ///
    /// # Errors
    /// Returns a message when the library refuses the attachment.
    pub fn attach_subscription(
        &mut self,
        subscriber: RawSubscriber,
        callback: impl FnMut(&Delivery<'_>) -> Result<(), String> + Send + 'static,
    ) -> Result<AttachmentId, String> {
        self.attach(subscriber, Box::new(callback), false)
    }

    /// As [`WaitSet::attach_subscription`], but a sample the callback takes ends the
    /// current run as a requested stop once the turn has delivered every queued sample.
    ///
    /// # Errors
    /// Returns a message when the library refuses the attachment.
    pub fn attach_waking_subscription(
        &mut self,
        subscriber: RawSubscriber,
        callback: impl FnMut(&Delivery<'_>) -> Result<(), String> + Send + 'static,
    ) -> Result<AttachmentId, String> {
        self.attach(subscriber, Box::new(callback), true)
    }

    fn attach(
        &mut self,
        subscriber: RawSubscriber,
        callback: SubscriptionCallback,
        ends_run: bool,
    ) -> Result<AttachmentId, String> {
        let context =
            SubscriptionContext { callback, failure: Arc::clone(&self.failure), ends_run };
        let attached = self.set().and_then(|set| {
            ffi::waitset_attach_subscription(set, subscriber.subscriber, Box::new(context))
                .map_err(ffi::status_code)
        });
        attached
            .map(AttachmentId)
            .map_err(|status| status_error("cannot attach the subscription", status))
    }

    /// Remove an attachment and release what it owns. Refused while the `WaitSet`
    /// runs, and for an id it does not hold.
    ///
    /// # Errors
    /// Returns a message when the library refuses the detach.
    pub fn detach(&mut self, id: AttachmentId) -> Result<(), String> {
        self.set()
            .and_then(|set| ffi::waitset_detach(set, id.0).map_err(ffi::status_code))
            .map_err(|status| status_error("cannot detach", status))
    }

    /// Run on the calling thread until a stop or a failure.
    pub fn run(&mut self) -> RunResult {
        self.run_bounded(None)
    }

    /// Run on the calling thread for at most `limit`, then stop.
    ///
    /// The deadline ends the run the way a requested stop does.
    pub fn run_for(&mut self, limit: Duration) -> RunResult {
        let limit_nanos = u64::try_from(limit.as_nanos()).unwrap_or(u64::MAX);
        // A zero limit would mean no limit; the shortest bounded run is one nanosecond.
        self.run_bounded(Some(Duration::from_nanos(limit_nanos.max(1))))
    }

    fn run_bounded(&mut self, limit: Option<Duration>) -> RunResult {
        let ran = self.set().and_then(|set| ffi::waitset_run(set, limit).map_err(ffi::status_code));
        let report = match ran {
            Ok(report) => report,
            Err(status) => return RunResult::api_failure(status),
        };
        let mut result = RunResult::from_report(&report);
        let mut slot = crate::locked(&self.failure);
        result.callback_error = slot.take().filter(|_| !result.ok());
        result
    }
}

impl BusNode {
    /// Every metric of this node, in the order the runtime emits them.
    ///
    /// # Errors
    /// Never fails: a node always has its metrics.
    pub fn metrics(&self) -> Result<Vec<(String, u64)>, String> {
        let node: &ffi::HyphaNode = &self.node;
        Ok(crate::sdk::metrics::collect(|emit| ffi::node_metrics(Some(node), emit)))
    }
}
