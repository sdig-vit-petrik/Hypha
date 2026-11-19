//! Configuration value checks through the C ABI.

use std::ptr;

use hypha_ffi::*;

const FINITE: u32 = HyphaNumberConstraint::Finite as u32;
const POSITIVE: u32 = HyphaNumberConstraint::Positive as u32;
const RANGE: u32 = HyphaNumberConstraint::Range as u32;
const NON_NEGATIVE: u32 = HyphaNumberConstraint::NonNegative as u32;

#[test]
fn number_constraints_accept_valid_values_and_refuse_invalid_ones() {
    assert_eq!(hypha_param_validate_number(-1.0, FINITE, 0.0, 0.0), HyphaStatus::Ok);
    assert_eq!(
        hypha_param_validate_number(f64::NAN, FINITE, 0.0, 0.0),
        HyphaStatus::InvalidArgument
    );
    assert_eq!(hypha_param_validate_number(0.5, POSITIVE, 0.0, 0.0), HyphaStatus::Ok);
    assert_eq!(hypha_param_validate_number(0.0, POSITIVE, 0.0, 0.0), HyphaStatus::InvalidArgument);
    assert_eq!(hypha_param_validate_number(1.0, RANGE, 0.0, 1.0), HyphaStatus::Ok);
    assert_eq!(hypha_param_validate_number(1.5, RANGE, 0.0, 1.0), HyphaStatus::InvalidArgument);
    assert_eq!(hypha_param_validate_number(0.0, NON_NEGATIVE, 0.0, 0.0), HyphaStatus::Ok);
    assert_eq!(hypha_param_validate_number(-0.0, NON_NEGATIVE, 0.0, 0.0), HyphaStatus::Ok);
    assert_eq!(hypha_param_validate_number(f64::MAX, NON_NEGATIVE, 0.0, 0.0), HyphaStatus::Ok);
    assert_eq!(
        hypha_param_validate_number(-1e-300, NON_NEGATIVE, 0.0, 0.0),
        HyphaStatus::InvalidArgument
    );
    assert_eq!(
        hypha_param_validate_number(f64::INFINITY, NON_NEGATIVE, 0.0, 0.0),
        HyphaStatus::InvalidArgument
    );
}

#[test]
fn an_unknown_number_constraint_is_refused() {
    assert_eq!(hypha_param_validate_number(1.0, 4, 0.0, 2.0), HyphaStatus::InvalidArgument);
    assert_eq!(hypha_param_validate_number(1.0, u32::MAX, 0.0, 2.0), HyphaStatus::InvalidArgument);
}

#[test]
fn nonempty_refuses_empty_null_and_invalid_text() {
    let text = "topic";
    let accepted = unsafe { hypha_param_validate_nonempty(text.as_ptr().cast(), text.len()) };
    assert_eq!(accepted, HyphaStatus::Ok);
    let empty = unsafe { hypha_param_validate_nonempty(text.as_ptr().cast(), 0) };
    assert_eq!(empty, HyphaStatus::InvalidArgument);
    let null = unsafe { hypha_param_validate_nonempty(ptr::null(), 4) };
    assert_eq!(null, HyphaStatus::InvalidArgument);
    let invalid = [0xff_u8, 0xfe];
    let not_utf8 = unsafe { hypha_param_validate_nonempty(invalid.as_ptr().cast(), invalid.len()) };
    assert_eq!(not_utf8, HyphaStatus::InvalidArgument);
}
