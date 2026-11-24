//! The C ABI wrappers of `hypha_node::cycle::transforms`.

use super::{CycleTarget, HyphaCycle};
use crate::boundary::{borrow_str, handle_ref, write_out};
use crate::status::try_guard;
use hypha_node::safe::{
    HyphaInputSource, ImageView, TransformLookup, cycle_bind_transforms, image_lookup_transform,
};
use hypha_node::status::HyphaStatus;
use hypha_node::{HYPHA_TF_FOUND, HyphaCycleImage, HyphaTransform};

/// Bind a TF input to an uncommitted cycle over two different managed
/// `tf2_msgs/TFMessage` sources of its `WaitSet`; the caller still destroys both handles.
/// `cache_nanos` > 0 is the history per frame. `*out_index` (0 on a refusal) counts TF
/// inputs from zero per cycle. `InvalidArgument`: null, zero cache, the same source
/// twice, an unwired or foreign source. `InvalidState`: committed cycle or running `WaitSet`.
///
/// # Safety
/// `cycle`, `tf`, `tf_static` and `out_index` are null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_cycle_bind_transforms(
    cycle: *mut HyphaCycle,
    tf: *mut HyphaInputSource,
    tf_static: *mut HyphaInputSource,
    cache_nanos: u64,
    out_index: *mut u32,
) -> HyphaStatus {
    try_guard(|| {
        unsafe { write_out(out_index, 0) }?;
        let cycle = unsafe { CycleTarget::of(cycle) }?;
        let tf = unsafe { handle_ref(tf) }?;
        let tf_static = unsafe { handle_ref(tf_static) }?;
        // Refused before the run check, as `cycle_bind_transforms` does.
        if cache_nanos == 0 {
            return Err(HyphaStatus::InvalidArgument);
        }
        let (set, cycle) = unsafe { cycle.waitset() }?;
        let index = cycle_bind_transforms(&set, cycle, tf, tf_static, cache_nanos)?;
        unsafe { write_out(out_index, index) }
    })
}

/// Look up, in a step, `source` in `target` at `at_nanos`, or with `latest` nonzero at the
/// latest time common to the chain (`at_nanos` must then be 0). Names are UTF-8 of the
/// given length and lose one leading `/`; equal names give the identity. On `Ok`,
/// `*out_result` is a `HYPHA_TF_*` value; `*out` is written only for `HYPHA_TF_FOUND`.
/// `InvalidArgument`: null, unknown index, non-UTF-8 names, `latest` with a nonzero
/// `at_nanos`. `InvalidState`: an overflow composing the chain.
///
/// # Safety
/// `image` is null or the step's live image. `target` and `source` are null or hold
/// their byte lengths. `out_result` and `out` are null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_cycle_image_lookup_transform(
    image: *const HyphaCycleImage,
    index: u32,
    target: *const std::os::raw::c_char,
    target_len: usize,
    source: *const std::os::raw::c_char,
    source_len: usize,
    at_nanos: i64,
    latest: u32,
    out_result: *mut u32,
    out: *mut HyphaTransform,
) -> HyphaStatus {
    try_guard(|| {
        let view = unsafe { handle_ref(image.cast::<ImageView<'_>>()) }?;
        let missing_output = out_result.is_null() || out.is_null();
        if missing_output {
            return Err(HyphaStatus::InvalidArgument);
        }
        let at = match (latest, at_nanos) {
            (0, at) => Some(at),
            (_, 0) => None,
            _ => return Err(HyphaStatus::InvalidArgument),
        };
        let target = unsafe { borrow_str(target, target_len) }?;
        let source = unsafe { borrow_str(source, source_len) }?;
        match image_lookup_transform(view, index, target, source, at)? {
            TransformLookup::Found(transform) => unsafe {
                write_out(out, transform)?;
                write_out(out_result, HYPHA_TF_FOUND)
            },
            TransformLookup::Missing(code) => unsafe { write_out(out_result, code) },
        }
    })
}
