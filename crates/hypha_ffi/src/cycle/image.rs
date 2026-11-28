//! The C ABI wrappers of `hypha_node::cycle::image`.

use crate::boundary::{handle_mut, handle_ref, require_non_null, write_out};
use crate::status::try_guard;
use hypha_node::safe::{
    EmissionView, ImageView, emission_commit, emission_reserve, image_alive, image_binding,
    image_held, image_reject_sample, image_sample, image_set_ignored,
};
use hypha_node::status::HyphaStatus;
use hypha_node::{
    HyphaCycleBindingView, HyphaCycleEmission, HyphaCycleImage, HyphaCycleSampleView,
};

/// Read sample `sample_index` of binding `binding_index` from the current image: a
/// borrowed view, valid only during the callback. `InvalidArgument` for a null pointer or
/// an unknown binding; `NotAvailable` for an index past the samples of the step.
///
/// # Safety
/// `image` and `out` must be callback-live pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_cycle_image_sample(
    image: *const HyphaCycleImage,
    binding_index: u32,
    sample_index: usize,
    out: *mut HyphaCycleSampleView,
) -> HyphaStatus {
    try_guard(|| {
        let view = unsafe { handle_ref(image.cast::<ImageView<'_>>()) }?;
        require_non_null(out)?;
        let sample = image_sample(view, binding_index, sample_index)?;
        unsafe { write_out(out, sample) }
    })
}

/// Reserve a transport loan of `len` payload bytes for one message of `output_index`. On
/// `HYPHA_STATUS_OK`, `*out` is exactly `len` writable bytes, owned by the library until
/// `hypha_cycle_emission_commit` or the end of the step. A null `*out` with
/// `HYPHA_STATUS_OK` means the message was dropped: write nothing and do not commit. An
/// unknown index, a `len` above the payload limit, a message past the per-step count, an
/// allocation failure or a reserve while one is open is latched and returned. A null
/// `emission` or `out` is `InvalidArgument` and latches nothing.
/// # Safety
/// `emission` must be valid for the current callback. `out` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_cycle_emission_reserve(
    emission: *mut HyphaCycleEmission,
    output_index: u32,
    len: usize,
    out: *mut *mut u8,
) -> HyphaStatus {
    try_guard(|| {
        let view = unsafe { handle_mut(emission.cast::<EmissionView<'_, '_>>()) }?;
        unsafe { write_out(out, std::ptr::null_mut()) }?;
        let payload = emission_reserve(view, output_index, len)?;
        match payload {
            Some(buffer) => unsafe { write_out(out, buffer.as_mut_ptr()) },
            None => Ok(()),
        }
    })
}

/// Seal the reservation of the last `hypha_cycle_emission_reserve` for dispatch after
/// the step. `HYPHA_STATUS_INVALID_STATE`, latched, when no reservation is open. A
/// reservation still open when the step returns fails the step.
///
/// # Safety
/// `emission` must be valid for the current callback.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_cycle_emission_commit(
    emission: *mut HyphaCycleEmission,
) -> HyphaStatus {
    try_guard(|| {
        let view = unsafe { handle_mut(emission.cast::<EmissionView<'_, '_>>()) }?;
        Ok(emission_commit(view)?)
    })
}

/// Read the held sample of one image binding: the newest usable sample it captured. The
/// payload is valid only during the callback. `NotAvailable` while `has_held` is zero;
/// `InvalidArgument` for a null pointer or an unknown binding.
///
/// # Safety
/// `image` and `view` must be callback-live pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_cycle_image_held(
    image: *const HyphaCycleImage,
    binding_index: u32,
    view: *mut HyphaCycleSampleView,
) -> HyphaStatus {
    try_guard(|| {
        let image_view = unsafe { handle_ref(image.cast::<ImageView<'_>>()) }?;
        require_non_null(view)?;
        let sample = image_held(image_view, binding_index)?;
        unsafe { write_out(view, sample) }
    })
}

/// Report the alive counter of the newest sample this callback read, applied after the
/// step. Valid only during the step callback; the last value per binding counts.
/// `InvalidArgument` for a null `image` or an unknown binding.
///
/// # Safety
/// `image` must be a callback-live pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_cycle_image_alive(
    image: *const HyphaCycleImage,
    binding_index: u32,
    value: u64,
) -> HyphaStatus {
    try_guard(|| {
        let view = unsafe { handle_ref(image.cast::<ImageView<'_>>()) }?;
        Ok(image_alive(view, binding_index, value)?)
    })
}

/// Override (nonzero `ignore`) or restore the fault of one binding; the fault bits do
/// not change. Valid only during the step callback; applied after the step returns.
/// `InvalidArgument` for a null `image` or an unknown binding.
///
/// # Safety
/// `image` must be a callback-live pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_cycle_image_set_ignored(
    image: *const HyphaCycleImage,
    binding_index: u32,
    ignore: u8,
) -> HyphaStatus {
    try_guard(|| {
        let view = unsafe { handle_ref(image.cast::<ImageView<'_>>()) }?;
        Ok(image_set_ignored(view, binding_index, ignore != 0)?)
    })
}

/// Fill `out` with the health, held sample and supervision of binding `binding_index`
/// at this step. `InvalidArgument` for a null pointer or an index past the last binding;
/// `out` is unchanged on an error.
///
/// # Safety
/// `image` and `out` must be callback-live pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_cycle_image_binding(
    image: *const HyphaCycleImage,
    binding_index: u32,
    out: *mut HyphaCycleBindingView,
) -> HyphaStatus {
    try_guard(|| {
        let view = unsafe { handle_ref(image.cast::<ImageView<'_>>()) }?;
        require_non_null(out)?;
        let binding = image_binding(view, binding_index)?;
        unsafe { write_out(out, binding) }
    })
}

/// Report that the sample `sequence` of binding `binding_index` did not decode.
///
/// Valid only in the step callback, for a sample this capture selected. No binding
/// selects it again; the binding is judged again without it and `out` receives that
/// view. `InvalidArgument`: null `image` or `out`, an index past the last binding.
/// `NotAvailable`: a sequence the capture did not select. Calling it twice is harmless.
///
/// # Safety
/// `image` and `out` must be callback-live pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_cycle_image_reject_sample(
    image: *const HyphaCycleImage,
    binding_index: u32,
    sequence: u64,
    out: *mut HyphaCycleBindingView,
) -> HyphaStatus {
    try_guard(|| {
        let view = unsafe { handle_ref(image.cast::<ImageView<'_>>()) }?;
        require_non_null(out)?;
        let binding = image_reject_sample(view, binding_index, sequence)?;
        unsafe { write_out(out, binding) }
    })
}
