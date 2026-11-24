//! The transform buffer of a plain node: a [`TfState`] fed by two subscriptions on the
//! node's default `WaitSet`, looked up from any thread.
//!
//! A lookup takes the same lock as ingestion and never waits on the transport.

use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use hypha_core::port::QosRequest;
use hypha_msgs::tf2_msgs::TFMessage;

use super::{IngestPass, TfOwner, TfState, TransformLookup};
use crate::node::HyphaNode;
use crate::status::Error;
use crate::subscriber::{SubscriberIdentity, SubscriberSettings, subscribe_typed};
use crate::waitset::configuration::with_default_waitset;
use crate::waitset::{
    CallbackAction, HyphaAttachmentId, HyphaWaitSet, SampleDelivery, SubscriptionHandler,
    WaitSetSlot, waitset_attach_subscription, waitset_detach,
};

/// The transform buffer of a plain node. Opaque to the caller.
///
/// It names its node weakly, so it never keeps a destroyed node alive. Any thread may
/// look up through a shared reference.
pub struct HyphaTransformBuffer {
    node: Weak<HyphaNode>,
    state: Arc<Mutex<TfState>>,
    /// The `/tf` and the `/tf_static` registration on the node's default `WaitSet`.
    attachments: [HyphaAttachmentId; 2],
}

/// What a registration calls for each sample of one of the two topics.
struct Ingest {
    state: Arc<Mutex<TfState>>,
    tf: String,
    tf_static: String,
    is_static: bool,
}

impl SubscriptionHandler for Ingest {
    /// Ingest one message under the lock. A message that does not decode is counted and
    /// logged, and the run goes on.
    fn deliver(&mut self, delivery: &SampleDelivery<'_>, _set: &HyphaWaitSet) -> CallbackAction {
        let owner = TfOwner::Buffer { tf: &self.tf, tf_static: &self.tf_static };
        let mut pass = IngestPass::default();
        let mut state = crate::locked(&self.state);
        if state.ingest(owner, delivery.payload, self.is_static, &mut pass).is_err() {
            state.count_undecodable(&mut pass);
        }
        state.finish(owner, pass);
        CallbackAction::Continue
    }
}

/// Create a transform buffer on `node` over the topics `tf` and `tf_static`, attached to
/// the node's default `WaitSet`. `cache_nanos` of dynamic history is kept per frame.
///
/// # Errors
/// `InvalidArgument` for an empty topic, a zero cache or two names of one topic;
/// `InvalidState` while the default `WaitSet` runs or is lent; the error of a subscribe
/// or of an attach. A refused create leaves nothing attached and nothing subscribed.
pub fn transform_buffer_create(
    node: &Arc<HyphaNode>,
    tf: &str,
    tf_static: &str,
    request: &QosRequest,
    cache_nanos: u64,
) -> Result<Box<HyphaTransformBuffer>, Error> {
    let refused = tf.is_empty() || tf_static.is_empty() || cache_nanos == 0;
    if refused {
        return Err(Error::InvalidArgument);
    }
    let health = Arc::clone(&node.health);
    let state = Arc::new(Mutex::new(TfState::new(Duration::from_nanos(cache_nanos), health)));
    let topics = [tf, tf_static];
    let attachments =
        with_default_waitset(node, |set| attach_both(set, node, topics, request, &state))??;
    Ok(Box::new(HyphaTransformBuffer { node: Arc::downgrade(node), state, attachments }))
}

/// Subscribe both topics and attach both subscriptions to `set`; on a refusal nothing
/// stays attached.
fn attach_both(
    set: &HyphaWaitSet,
    node: &HyphaNode,
    [tf, tf_static]: [&str; 2],
    request: &QosRequest,
    state: &Arc<Mutex<TfState>>,
) -> Result<[HyphaAttachmentId; 2], Error> {
    let _claim = set.shared.claim()?;
    if set.is_running() {
        return Err(Error::InvalidState);
    }
    let identity = SubscriberIdentity {
        data_type: TFMessage::TYPE_NAME,
        expected_md5: Some(TFMessage::SCHEMA_HASH),
        definition: "",
    };
    let settings = || SubscriberSettings {
        depth: 0,
        no_publisher_warning_after_nanos: 0,
        request: request.clone(),
    };
    let dynamic = subscribe_typed(node, tf, &identity, settings())?;
    let fixed = subscribe_typed(node, tf_static, &identity, settings())?;
    if dynamic.topic == fixed.topic {
        return Err(Error::InvalidArgument);
    }
    let handler = |is_static| Ingest {
        state: Arc::clone(state),
        tf: dynamic.topic.clone(),
        tf_static: fixed.topic.clone(),
        is_static,
    };
    let (dynamic_handler, fixed_handler) = (handler(false), handler(true));
    let first = waitset_attach_subscription(set, dynamic, Box::new(dynamic_handler))?;
    let second =
        waitset_attach_subscription(set, fixed, Box::new(fixed_handler)).inspect_err(|_| {
            if let Err(error) = waitset_detach(set, first) {
                log::error!(target: "hypha", "hypha: a refused transform buffer could not detach its '/tf' subscription: {error:?}");
            }
        })?;
    Ok([first, second])
}

/// Look up, in `buffer`, `source` in `target` at `at` nanoseconds, or at the latest time
/// common to the chain for `None`. Any thread may call it, also during a run.
///
/// # Errors
/// `InvalidState` for an overflow composing the chain.
pub fn transform_buffer_lookup(
    buffer: &HyphaTransformBuffer,
    target: &str,
    source: &str,
    at: Option<i64>,
) -> Result<TransformLookup, Error> {
    crate::locked(&buffer.state).lookup(target, source, at)
}

/// Detach both registrations of `buffer`, which releases both subscribers. From a
/// callback of the running `WaitSet` the detach is deferred to the end of the run.
///
/// # Errors
/// `InvalidState` during a run on another thread, or while the default `WaitSet` is
/// lent to a Rust owner; nothing is detached then.
pub fn transform_buffer_destroy(buffer: &HyphaTransformBuffer) -> Result<(), Error> {
    let Some(node) = buffer.node.upgrade() else {
        return Ok(());
    };
    let slot = crate::locked(&node.default_waitset);
    let set = match &*slot {
        WaitSetSlot::Held(set) => set,
        WaitSetSlot::Absent | WaitSetSlot::Closed => return Ok(()),
        WaitSetSlot::Lent => return Err(Error::InvalidState),
    };
    let _claim = set.shared.claim()?;
    let [dynamic, fixed] = buffer.attachments;
    waitset_detach(set, dynamic)?;
    waitset_detach(set, fixed)
}

#[cfg(test)]
mod tests {
    use super::HyphaTransformBuffer;

    fn shared_across_threads<T: Send + Sync>() {}

    #[test]
    fn the_buffer_handle_may_be_shared_across_threads() {
        shared_across_threads::<HyphaTransformBuffer>();
    }
}
