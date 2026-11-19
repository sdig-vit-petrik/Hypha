//! The C ABI wrappers of `hypha_node::subscriber`.

use crate::status::try_guard;
use hypha_node::HyphaSubscriberOptions;
use hypha_node::qos::read_declaration;
use hypha_node::safe::{
    HyphaNode, HyphaSubscriber, SubscriberIdentity, SubscriberSettings, subscribe_typed,
    subscriber_publisher_count,
};
use hypha_node::status::HyphaStatus;
mod counters;
mod missing;
mod read;
pub use counters::hypha_subscriber_counters;
pub use missing::hypha_subscriber_missing_sequences;
pub use read::{hypha_subscriber_destroy, hypha_subscriber_topic};

use crate::boundary::{borrow_str, handle_ref, read_in, require_non_null, write_out};
use std::os::raw::c_char;
use std::sync::Arc;

/// Subscribe to `topic` with `options` and record `data_type` (null: empty) in the node
/// introspection. `definition` (length 0: none) is the definition of `expected_md5`
/// (length 0: none); with one, a non-empty `data_type` and a valid `expected_md5` are
/// required (`InvalidSchema` otherwise). `InvalidArgument`: a null `node`, `options` or
/// `out`. `InvalidTopic`: a `~/` topic on a CLI tool node.
/// # Safety
/// `topic`, `data_type`, `expected_md5` and `definition` point to their lengths of bytes.
/// `options` points to an initialized [`HyphaSubscriberOptions`]. The caller releases the
/// subscriber with `hypha_subscriber_destroy`.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn hypha_subscribe_typed(
    node: *mut HyphaNode,
    topic: *const c_char,
    topic_len: usize,
    data_type: *const c_char,
    data_type_len: usize,
    expected_md5: *const c_char,
    expected_md5_len: usize,
    definition: *const c_char,
    definition_len: usize,
    options: *const HyphaSubscriberOptions,
    out: *mut *mut HyphaSubscriber,
) -> HyphaStatus {
    try_guard(|| {
        let data_type =
            if data_type.is_null() { "" } else { unsafe { borrow_str(data_type, data_type_len) }? };
        let options = unsafe { read_in(options) }?;
        let expected_md5 = if expected_md5_len == 0 {
            None
        } else {
            Some(unsafe { borrow_str(expected_md5, expected_md5_len) }?)
        };
        let definition = if definition_len == 0 {
            ""
        } else {
            unsafe { borrow_str(definition, definition_len) }?
        };
        let session = unsafe { handle_ref(node) }?;
        require_non_null(out)?;
        let topic = unsafe { borrow_str(topic, topic_len) }?;
        let identity = SubscriberIdentity { data_type, expected_md5, definition };
        let qos_name = unsafe { crate::boundary::qos_name(&options.qos) }?;
        let declaration = read_declaration(&options.qos, qos_name)?;
        let settings = SubscriberSettings {
            depth: options.depth,
            no_publisher_warning_after_nanos: options.no_publisher_warning_after_nanos,
            request: declaration.request,
        };
        let subscriber = subscribe_typed(session, topic, &identity, settings)?;
        // An `Arc`: a `WaitSet` registration shares the subscriber.
        unsafe { write_out(out, Arc::into_raw(subscriber).cast_mut()) }
    })
}

/// Write the number of publishers currently connected to the subscriber's topic to
/// `out`. `InvalidArgument`, with `out` unwritten, when `subscriber` or `out` is null.
///
/// # Safety
/// `subscriber` is null or live. `out` is null or one writable `size_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_subscriber_publisher_count(
    subscriber: *const HyphaSubscriber,
    out: *mut usize,
) -> HyphaStatus {
    try_guard(|| {
        let count = subscriber_publisher_count(unsafe { handle_ref(subscriber) }?)?;
        unsafe { write_out(out, count) }
    })
}
