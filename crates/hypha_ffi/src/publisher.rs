//! The C ABI wrappers of `hypha_node::publisher`.

use crate::boundary::{borrow_str, handle_ref, read_in, require_non_null, take_box, write_out};
use crate::status::try_guard;
use hypha_node::HyphaPublisherOptions;
use hypha_node::safe::{
    HyphaNode, HyphaPublisher, PublisherSettings, publish_with_qos,
    publisher_min_subscriber_buffer, publisher_subscriber_count,
};
use hypha_node::status::HyphaStatus;
use std::os::raw::c_char;

/// Publish `topic` with the endpoint `options`.
///
/// With a non-zero `options.latch`, a late subscriber still receives the most recent
/// samples. `InvalidTopic`: a `definition` that does not match `md5`, or a `~/` topic on
/// a CLI tool node. `InvalidArgument`: a null `node`, `options` or `out`.
/// # Safety
/// `topic`, `data_type`, `md5` and `definition` point to their lengths of bytes.
/// `options` points to an initialized [`HyphaPublisherOptions`]. The caller releases
/// the publisher with `hypha_publisher_destroy`.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn hypha_publish_with_qos(
    node: *mut HyphaNode,
    topic: *const c_char,
    topic_len: usize,
    data_type: *const c_char,
    data_type_len: usize,
    md5: *const c_char,
    md5_len: usize,
    definition: *const c_char,
    definition_len: usize,
    options: *const HyphaPublisherOptions,
    out: *mut *mut HyphaPublisher,
) -> HyphaStatus {
    try_guard(|| {
        let session = unsafe { handle_ref(node) }?;
        require_non_null(out)?;
        let topic = unsafe { borrow_str(topic, topic_len) }?;
        let data_type = unsafe { borrow_str(data_type, data_type_len) }?;
        let md5 = unsafe { borrow_str(md5, md5_len) }?;
        let definition = unsafe { borrow_str(definition, definition_len) }?;
        let options = unsafe { read_in(options) }?;
        let qos_name = unsafe { crate::boundary::qos_name(&options.qos) }?;
        let settings = PublisherSettings::from_options(&options, qos_name)?;
        let publisher = publish_with_qos(session, topic, data_type, md5, definition, settings)?;
        unsafe { write_out(out, Box::into_raw(Box::new(publisher))) }
    })
}

/// Release a publisher. Passing null is allowed and does nothing.
///
/// # Safety
/// `publisher` must come from `hypha_publish_with_qos` and must not be used afterwards.
/// Every loan taken from it must be released first.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_publisher_destroy(publisher: *mut HyphaPublisher) {
    drop(unsafe { take_box(publisher) });
}

/// Writes the number of subscribers connected to the publisher's topic and returns 1.
/// Returns 0, with `count` unmodified, when an argument is null.
///
/// # Safety
/// `publisher` must come from `hypha_publish_with_qos`, or be null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_publisher_subscriber_count(
    publisher: *const HyphaPublisher,
    count: *mut usize,
) -> i32 {
    let Ok(endpoint) = (unsafe { handle_ref(publisher) }) else {
        return 0;
    };
    let Some(subscriber_count) = publisher_subscriber_count(endpoint) else {
        return 0;
    };
    i32::from(unsafe { write_out(count, subscriber_count) }.is_ok())
}

/// Write the smallest queue size among the subscribers connected to `publisher` into
/// `out`, or 0 when none is connected. `InvalidArgument`, with `out` unwritten, when
/// `publisher` or `out` is null.
///
/// # Safety
/// `publisher` is live or null. `out` points to a writable `size_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_publisher_min_subscriber_buffer(
    publisher: *const HyphaPublisher,
    out: *mut usize,
) -> HyphaStatus {
    try_guard(|| {
        let smallest = publisher_min_subscriber_buffer(unsafe { handle_ref(publisher) }?);
        unsafe { write_out(out, smallest) }
    })
}
