//! The C ABI wrappers of `hypha_node::transforms::buffer`: the transform buffer of a
//! plain node, and the one with no node that its caller feeds.

use std::os::raw::c_char;

use crate::boundary::{
    borrow_bytes, borrow_str, handle_ref, qos_name, read_in, require_non_null, take_box, write_out,
};
use crate::status::try_guard;
use crate::waitset::configuration::share_node;
use hypha_node::qos::{HyphaQos, read_declaration};
use hypha_node::safe::{
    HyphaNode, HyphaSentTransform, HyphaTransformBuffer, TransformLookup, transform_buffer_create,
    transform_buffer_create_fed, transform_buffer_destroy, transform_buffer_feed,
    transform_buffer_lookup, transform_check,
};
use hypha_node::status::HyphaStatus;
use hypha_node::{HYPHA_TF_FOUND, HyphaTransform};

/// Create a transform buffer of `node` over the topics `tf_topic` (dynamic) and
/// `tf_static_topic` (static), UTF-8 of the given lengths, borrowed for the call. Both
/// are subscribed as `tf2_msgs/TFMessage` with `qos` (read for the call; payload fields
/// ignored) and attached to the node's default `WaitSet`, whose runs ingest them.
/// `cache_nanos` > 0 is the dynamic history kept per frame. A message that does not
/// decode is counted and logged and fails nothing.
///
/// `*out` (null on a refusal) is owned by the caller, released with
/// `hypha_transform_buffer_destroy`. `InvalidArgument`: null `node`, `qos` or `out`,
/// an empty or non-UTF-8 topic, zero `cache_nanos`, two names of one topic, an invalid
/// policy. `InvalidState`: the default `WaitSet` runs. `QosMismatch`: a topic exists
/// with another policy. A refused create leaves nothing attached. Call it on the thread
/// that configures the node.
///
/// # Safety
/// Each pointer is null or valid: topics hold their byte lengths, `qos` is initialized
/// with `name_len` name bytes, `out` is one writable pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_transform_buffer_create(
    node: *mut HyphaNode,
    tf_topic: *const c_char,
    tf_topic_len: usize,
    tf_static_topic: *const c_char,
    tf_static_topic_len: usize,
    qos: *const HyphaQos,
    cache_nanos: u64,
    out: *mut *mut HyphaTransformBuffer,
) -> HyphaStatus {
    try_guard(|| {
        // SAFETY: `out` is null or writable by the caller's contract; null is refused.
        unsafe { write_out(out, std::ptr::null_mut()) }?;
        require_non_null(node)?;
        // SAFETY: the topic holds `tf_topic_len` bytes or is null, by the contract.
        let tf = unsafe { borrow_str(tf_topic, tf_topic_len) }?;
        // SAFETY: as above, for the static topic.
        let tf_static = unsafe { borrow_str(tf_static_topic, tf_static_topic_len) }?;
        // SAFETY: `qos` is null or initialized by the contract; null is refused.
        let qos = unsafe { read_in(qos) }?;
        // SAFETY: the policy name holds `name_len` bytes by the contract.
        let name = unsafe { qos_name(&qos) }?;
        let declaration = read_declaration(&qos, name)?;
        // SAFETY: `node` is non-null (checked above) and a live node handle.
        let node = unsafe { share_node(node) };
        let buffer =
            transform_buffer_create(&node, tf, tf_static, &declaration.request, cache_nanos)?;
        // SAFETY: `out` was written above, so it is non-null and writable.
        unsafe { write_out(out, Box::into_raw(buffer)) }
    })
}

/// Create a transform buffer with no node and no subscriber, which
/// `hypha_transform_buffer_feed` fills: the TF input of a step test. `cache_nanos` > 0
/// is the dynamic history kept per frame. Look it up and destroy it as any buffer.
///
/// `*out` (null on a refusal) is owned by the caller, released with
/// `hypha_transform_buffer_destroy`. `InvalidArgument`: null `out`, zero `cache_nanos`.
///
/// # Safety
/// `out` is null or one writable pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_transform_buffer_create_fed(
    cache_nanos: u64,
    out: *mut *mut HyphaTransformBuffer,
) -> HyphaStatus {
    try_guard(|| {
        // SAFETY: `out` is null or writable by the caller's contract; null is refused.
        unsafe { write_out(out, std::ptr::null_mut()) }?;
        let buffer = transform_buffer_create_fed(cache_nanos)?;
        // SAFETY: `out` was written above, so it is non-null and writable.
        unsafe { write_out(out, Box::into_raw(buffer)) }
    })
}

/// Insert into `buffer`, made by `hypha_transform_buffer_create_fed`, every transform of
/// the `tf2_msgs/TFMessage` in `payload` (ROS 1 wire bytes, `payload_len` of them,
/// borrowed for the call), as static transforms when `is_static` is nonzero, as a
/// cycle's TF input ingests a sample it captured. A transform that input refuses (an
/// empty frame name, a stamp that is no time, a value that is not finite, a rotation
/// that is not a unit quaternion, a second parent) is logged at a bounded rate and left
/// out, the others are kept, and a lookup on its frame answers `HYPHA_TF_REFUSED`.
///
/// `InvalidArgument`: null `buffer` or `payload`. `InvalidState`: a buffer that
/// `hypha_transform_buffer_create` subscribed. `InvalidFrame`: `payload` does not
/// decode; nothing is inserted. Any thread may call it while the handle is live.
///
/// # Safety
/// `buffer` is null or a live handle that no thread destroys during the call. `payload`
/// is null or holds `payload_len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_transform_buffer_feed(
    buffer: *const HyphaTransformBuffer,
    payload: *const u8,
    payload_len: usize,
    is_static: u32,
) -> HyphaStatus {
    try_guard(|| {
        // SAFETY: `buffer` is null or live, and destroyed by no thread, by the contract.
        let buffer = unsafe { handle_ref(buffer) }?;
        // SAFETY: the payload holds `payload_len` bytes or is null, by the contract.
        let payload = unsafe { borrow_bytes(payload, payload_len) }?;
        transform_buffer_feed(buffer, payload, is_static != 0)?;
        Ok(())
    })
}

/// Look up, in `buffer`, `source` in `target` at `at_nanos`, or with `latest` nonzero at
/// the latest time common to the chain (`at_nanos` must then be 0). Names are UTF-8 of
/// the given length, borrowed for the call. On `Ok`, `*out_result` is a `HYPHA_TF_*`
/// value; `*out` is written only for `HYPHA_TF_FOUND`. Semantics are those of
/// `hypha_cycle_image_lookup_transform`.
///
/// Any thread may call it while the handle is live, concurrently with ingestion and
/// other lookups. `InvalidArgument`: null `buffer`, `target`, `source`, `out_result` or
/// `out`, non-UTF-8 names, `latest` with a nonzero `at_nanos`; no output is written then.
/// `InvalidState`: an overflow composing the chain.
///
/// # Safety
/// `buffer` is null or a live handle no thread destroys during the call. `target` and
/// `source` are null or hold their byte lengths. `out_result` and `out` are null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_transform_buffer_lookup(
    buffer: *const HyphaTransformBuffer,
    target: *const c_char,
    target_len: usize,
    source: *const c_char,
    source_len: usize,
    at_nanos: i64,
    latest: u32,
    out_result: *mut u32,
    out: *mut HyphaTransform,
) -> HyphaStatus {
    try_guard(|| {
        // SAFETY: `buffer` is null or live, and destroyed by no thread, by the contract.
        let buffer = unsafe { handle_ref(buffer) }?;
        require_non_null(out_result)?;
        require_non_null(out)?;
        let at = match (latest, at_nanos) {
            (0, at) => Some(at),
            (_, 0) => None,
            _ => return Err(HyphaStatus::InvalidArgument),
        };
        // SAFETY: the name holds `target_len` bytes or is null, by the contract.
        let target = unsafe { borrow_str(target, target_len) }?;
        // SAFETY: as above, for `source`.
        let source = unsafe { borrow_str(source, source_len) }?;
        match transform_buffer_lookup(buffer, target, source, at)? {
            TransformLookup::Found(transform) => {
                // SAFETY: `out` is non-null (checked above) and writable by the contract.
                unsafe { write_out(out, transform) }?;
                // SAFETY: as above, for `out_result`.
                unsafe { write_out(out_result, HYPHA_TF_FOUND) }
            }
            // SAFETY: as above.
            TransformLookup::Missing(code) => unsafe { write_out(out_result, code) },
        }
    })
}

/// Destroy `buffer`: detach both subscribers from the node's default `WaitSet` and free
/// the handle. Null does nothing. From a callback of the running `WaitSet` the detach is
/// deferred to the end of the run. After `hypha_node_destroy`, and for a buffer from
/// `hypha_transform_buffer_create_fed`, it only frees the handle.
///
/// `InvalidState`: the `WaitSet` runs on another thread; nothing changes and the handle
/// stays live. Never call it during a lookup on the same handle.
///
/// # Safety
/// `buffer` is null or a live handle from `hypha_transform_buffer_create` or
/// `hypha_transform_buffer_create_fed`, not used after an `Ok`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_transform_buffer_destroy(
    buffer: *mut HyphaTransformBuffer,
) -> HyphaStatus {
    if buffer.is_null() {
        return HyphaStatus::Ok;
    }
    try_guard(|| {
        // SAFETY: non-null (checked above) and live by the contract.
        let handle = unsafe { handle_ref(buffer) }?;
        transform_buffer_destroy(handle)?;
        // SAFETY: from `Box::into_raw` in the create; unused by the caller after `Ok`.
        drop(unsafe { take_box(buffer) });
        Ok(())
    })
}

/// Check one transform before it is sent with the check every transform buffer and TF
/// input applies on ingest: `child` in `parent`, UTF-8 of the given lengths, borrowed
/// for the call, with `*transform` read for the call; `is_static` nonzero for a
/// `/tf_static` transform, whose stamp is not read. Needs no node and keeps nothing.
///
/// `Ok`: every buffer accepts it, unless it gives the child a second parent or closes a
/// cycle, which only a buffer that holds the other transforms can see.
/// `InvalidArgument`: a null pointer, non-UTF-8 text, an empty frame name, a stamp that
/// is no time (`stamp_nsecs` of 1 000 000 000 or more), a translation or rotation that is
/// not finite, or a rotation that is not a unit quaternion. The reason, naming both
/// frames, is then the calling thread's `hypha_last_error`; nothing is logged.
///
/// # Safety
/// `parent` holds `parent_len` bytes and `child` holds `child_len` bytes, or each is
/// null; `transform` is null or points to one initialized `HyphaSentTransform`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_transform_check(
    parent: *const c_char,
    parent_len: usize,
    child: *const c_char,
    child_len: usize,
    transform: *const HyphaSentTransform,
    is_static: u32,
) -> HyphaStatus {
    try_guard(|| {
        // SAFETY: the parent holds `parent_len` bytes or is null, by the contract.
        let parent = unsafe { borrow_str(parent, parent_len) }?;
        // SAFETY: the child holds `child_len` bytes or is null, by the contract.
        let child = unsafe { borrow_str(child, child_len) }?;
        // SAFETY: `transform` is null or initialized by the contract; null is refused.
        let transform = unsafe { read_in(transform) }?;
        transform_check(parent, child, &transform, is_static != 0)?;
        Ok(())
    })
}
