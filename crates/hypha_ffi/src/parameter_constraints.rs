//! The C ABI wrappers of `hypha_node::parameter_constraints`.

use crate::boundary::borrow_str;
use hypha_core::parameter_constraints::{NumberConstraint, nonempty};
use hypha_node::HyphaNumberConstraint;
use hypha_node::status::HyphaStatus;
use std::os::raw::c_char;

/// Validate a numeric value against a `HyphaNumberConstraint` (any other value:
/// `InvalidArgument`). Bounds are read only for `Range`, inclusive. NaN and infinity
/// are always refused.
#[unsafe(no_mangle)]
pub extern "C" fn hypha_param_validate_number(
    value: f64,
    constraint: u32,
    minimum: f64,
    maximum: f64,
) -> HyphaStatus {
    let check = match constraint {
        c if c == HyphaNumberConstraint::Finite as u32 => NumberConstraint::Finite,
        c if c == HyphaNumberConstraint::Positive as u32 => NumberConstraint::Positive,
        c if c == HyphaNumberConstraint::NonNegative as u32 => NumberConstraint::NonNegative,
        c if c == HyphaNumberConstraint::Negative as u32 => NumberConstraint::Negative,
        c if c == HyphaNumberConstraint::NonPositive as u32 => NumberConstraint::NonPositive,
        c if c == HyphaNumberConstraint::Range as u32 => {
            NumberConstraint::Range { minimum, maximum }
        }
        _ => return HyphaStatus::InvalidArgument,
    };
    if check.accepts(value) { HyphaStatus::Ok } else { HyphaStatus::InvalidArgument }
}

/// Require at least one byte of UTF-8 text: `InvalidArgument` for a null pointer, a
/// zero length or text that is not UTF-8, else `Ok`.
///
/// # Safety
/// `value` must point to `value_len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_param_validate_nonempty(
    value: *const c_char,
    value_len: usize,
) -> HyphaStatus {
    crate::status::try_guard(|| {
        let value = unsafe { borrow_str(value, value_len) }?;
        if nonempty(value) { Ok(()) } else { Err(HyphaStatus::InvalidArgument) }
    })
}
