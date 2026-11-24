//! Named counters of a node and of a subscriber, read through one callback.
//!
//! Each call emits every metric in a fixed order. A status other than `Ok` can follow
//! metrics already emitted.

use std::os::raw::{c_char, c_void};

use crate::node::HyphaNode;
use crate::subscriber::{HyphaSubscriber, emit_subscriber_metrics};

/// Receives one metric. `name` points to `name_len` UTF-8 bytes and is valid
/// only during the call. `user_data` is the pointer passed to the reader.
pub type HyphaMetricCallback =
    Option<extern "C" fn(name: *const c_char, name_len: usize, value: u64, user_data: *mut c_void)>;

/// Emit the metrics of `node` to `emit`: its execution counters, its transform counters,
/// its schema counters and the process-wide log counters, in that order. Without a node,
/// only the log counters.
pub fn node_metrics(node: Option<&HyphaNode>, emit: &mut dyn FnMut(&str, u64)) {
    if let Some(node) = node {
        node.health.emit_metrics(emit);
        node.health.emit_tf_metrics(emit);
        crate::schema::emit_schema_metrics(node, emit);
    }
    crate::log::emit_log_metrics(emit);
}

/// Emit the `integrity.*` and `loan.*` metrics of `subscriber` to `emit`, in that order.
pub fn subscriber_metrics(subscriber: &HyphaSubscriber, emit: &mut dyn FnMut(&str, u64)) {
    emit_subscriber_metrics(subscriber, emit);
}
