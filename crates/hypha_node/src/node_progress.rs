//! The node's side of the replay progress contract: the publisher of `/clock_progress`,
//! the reads of `/clock` the node has seen and the offers of its `WaitSet`s. The rules are
//! in `hypha_runtime::replay_progress`.

use std::sync::{MutexGuard, PoisonError};

use hypha_msgs::hypha_msgs::ReplayProgress;

use hypha_runtime::replay_progress::{self as runtime, REPLAY_PROGRESS_DEFINITION};
pub(crate) use hypha_runtime::replay_progress::{ClockRead, PROGRESS_INPUT_DEPTH, PROGRESS_TOPIC};
use hypha_runtime::upstream_reports::decode_report;

use crate::node::HyphaNode;
use crate::publisher::{HyphaPublisher, PublisherSettings, open_publisher, send_frame_with};
use crate::qos::Declaration;
use crate::status::HyphaStatus;

/// The node's progress rules and state, and the progress publisher.
#[derive(Default)]
pub(crate) struct NodeProgress {
    rules: runtime::ProgressReporter,
    publisher: Option<HyphaPublisher>,
    publish_failure_reported: bool,
}

/// Record a `/clock` sample that a `WaitSet` of the node has read. The record is node-wide.
pub(crate) fn record_clock_read(node: &HyphaNode, read: ClockRead) {
    node.progress.lock().unwrap_or_else(PoisonError::into_inner).rules.note_read(read);
}

/// Record `read`, the `/clock` sample a restarted process rejoins the replay on.
pub(crate) fn record_rejoin(node: &HyphaNode, read: ClockRead) {
    let mut reporter = node.progress.lock().unwrap_or_else(PoisonError::into_inner);
    reporter.rules.rejoin(read);
}

/// The send time of the `/clock` sample this restarted process rejoined on, or zero.
pub(crate) fn epoch(node: &HyphaNode) -> u64 {
    node.progress.lock().unwrap_or_else(PoisonError::into_inner).rules.epoch()
}

/// The newest `/clock` sample that any `WaitSet` of the node has read, if any.
pub(crate) fn newest_read(node: &HyphaNode) -> Option<ClockRead> {
    node.progress.lock().unwrap_or_else(PoisonError::into_inner).rules.newest()
}

/// Offer `read` as the progress of the `WaitSet` at `waitset`, and send the node's report
/// if every offer now answers one sample. A failure is logged once, and the node keeps running.
pub(crate) fn report_progress(node: &HyphaNode, waitset: usize, read: ClockRead) {
    if node.replay_follow {
        return;
    }
    let mut reporter = node.progress.lock().unwrap_or_else(PoisonError::into_inner);
    reporter.rules.offer(waitset, read);
    let mut reporter = open_if_due(node, reporter);
    send_due(node, &mut reporter);
}

/// Forget the `WaitSet` at `waitset`. The offers left may now all answer one sample.
pub(crate) fn withdraw_progress(node: &HyphaNode, waitset: usize) {
    if node.replay_follow {
        return;
    }
    let mut reporter = node.progress.lock().unwrap_or_else(PoisonError::into_inner);
    if reporter.rules.withdraw(waitset) {
        let mut reporter = open_if_due(node, reporter);
        send_due(node, &mut reporter);
    }
}

/// Open the progress publisher when a report is due and none is open. The progress lock
/// is released while it runs, because the open may wait for other endpoints of the domain.
fn open_if_due<'a>(
    node: &'a HyphaNode,
    reporter: MutexGuard<'a, NodeProgress>,
) -> MutexGuard<'a, NodeProgress> {
    let publisher_missing = reporter.publisher.is_none();
    let report_due = reporter.rules.due().is_some();
    let open_needed = publisher_missing && report_due;
    if !open_needed {
        return reporter;
    }
    drop(reporter);
    let opened = open_progress_publisher(node);
    let mut reporter = node.progress.lock().unwrap_or_else(PoisonError::into_inner);
    match opened {
        Ok(publisher) => {
            if reporter.publisher.is_none() {
                reporter.publisher = Some(publisher);
            }
        }
        Err(status) => {
            if !reporter.publish_failure_reported {
                reporter.publish_failure_reported = true;
                log::error!(target: "hypha", "hypha: cannot publish the replay progress on {PROGRESS_TOPIC}: {status:?}");
            }
        }
    }
    reporter
}

/// Send the report the rules make due, if any, and record it as sent.
fn send_due(node: &HyphaNode, reporter: &mut NodeProgress) {
    let Some(read) = reporter.rules.due() else {
        return;
    };
    match send_progress(node, reporter, read) {
        Ok(()) => reporter.rules.mark_reported(read),
        Err(status) => {
            if !reporter.publish_failure_reported {
                reporter.publish_failure_reported = true;
                log::error!(target: "hypha", "hypha: cannot publish the replay progress on {PROGRESS_TOPIC}: {status:?}");
            }
        }
    }
}

/// Open the progress publisher unless it is open already, so that an upstream is visible
/// to its downstream before its first report.
/// A node outside the lockstep (`HYPHA_USE_SIM_TIME=follow`) opens none: it reports nothing.
pub(crate) fn ensure_publisher(node: &HyphaNode) -> Result<(), HyphaStatus> {
    if node.replay_follow {
        return Ok(());
    }
    let is_open = node.progress.lock().unwrap_or_else(PoisonError::into_inner).publisher.is_some();
    if is_open {
        return Ok(());
    }
    let publisher = open_progress_publisher(node)?;
    let mut reporter = node.progress.lock().unwrap_or_else(PoisonError::into_inner);
    if reporter.publisher.is_none() {
        reporter.publisher = Some(publisher);
    }
    Ok(())
}

/// Open the progress publisher. The caller stores it, and holds no progress lock.
fn open_progress_publisher(node: &HyphaNode) -> Result<HyphaPublisher, HyphaStatus> {
    let settings = PublisherSettings {
        latch: false,
        loaned_samples: 0,
        // The report of the longest node name; the node name makes the type variable.
        declaration: Declaration::undeclared(hypha_core::port::PayloadCapacity::Bounded(
            hypha_runtime::clock_barrier::PROGRESS_MAX_PAYLOAD_BYTES,
        )),
    };
    let publisher = open_publisher(
        node,
        PROGRESS_TOPIC,
        ReplayProgress::TYPE_NAME,
        ReplayProgress::SCHEMA_HASH,
        REPLAY_PROGRESS_DEFINITION,
        settings,
    )?;
    Ok(publisher)
}

/// Encodes the borrowed fields straight into the progress publisher's transport loan. A
/// send that finds no publisher fails.
fn send_progress(
    node: &HyphaNode,
    reporter: &mut NodeProgress,
    read: ClockRead,
) -> Result<(), HyphaStatus> {
    let publisher = reporter.publisher.as_mut().ok_or(HyphaStatus::Backend)?;
    let name = node.name.as_str();
    let payload_len = runtime::progress_payload_len(name).ok_or(HyphaStatus::TooLarge)?;
    send_frame_with(publisher, payload_len, |payload| {
        runtime::encode_progress(payload, name, read).map_err(|_| HyphaStatus::InvalidFrame)
    })
}

/// Keep the report in `sample`, unless it is this node's own. A frame or payload that
/// does not decode fails the run.
pub(crate) fn accept_report(
    node: &HyphaNode,
    sample: &hypha_iceoryx2::Iox2Sample,
) -> Result<(), HyphaStatus> {
    let report =
        decode_report(&node.name, sample.payload()).map_err(|_| HyphaStatus::CorruptPayload)?;
    if let Some((name, report)) = report {
        node.upstream.lock().unwrap_or_else(PoisonError::into_inner).store(name, report);
    }
    Ok(())
}

/// Log an error when the progress input has missed reports since the last check: a
/// downstream may wait for a clock it never receives.
pub(crate) fn note_progress_gaps(node: &HyphaNode) {
    let missing = match node.progress_subscriber.try_lock() {
        Ok(guard) => guard.as_deref().map(crate::subscriber::HyphaSubscriber::missing_sequences),
        Err(_) => None,
    };
    let Some(missing) = missing else {
        return;
    };
    node.upstream.lock().unwrap_or_else(PoisonError::into_inner).note_missing(missing);
}

/// The clock node `name` reported for a sample sent at or after `epoch`, if it has.
pub(crate) fn valid_clock(node: &HyphaNode, name: &str, epoch: u64) -> Option<u64> {
    node.upstream.lock().unwrap_or_else(PoisonError::into_inner).valid_clock(name, epoch)
}
