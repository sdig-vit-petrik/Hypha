//! Numeric and text checks for typed configuration values.

use hypha_core::parameter_constraints::{NumberConstraint, nonempty};

#[test]
fn numeric_constraints_refuse_nonfinite_values() {
    for check in [
        NumberConstraint::Finite,
        NumberConstraint::Positive,
        NumberConstraint::Range { minimum: -2.0, maximum: 2.0 },
    ] {
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(!check.accepts(value));
        }
    }
    assert!(NumberConstraint::Finite.accepts(-2.0));
    assert!(NumberConstraint::Positive.accepts(0.5));
    assert!(!NumberConstraint::Positive.accepts(0.0));
    assert!(!NumberConstraint::Positive.accepts(-1.0));
}

#[test]
fn range_is_closed_and_refuses_invalid_bounds() {
    let check = NumberConstraint::Range { minimum: -2.0, maximum: 2.0 };
    assert!(check.accepts(-2.0));
    assert!(check.accepts(2.0));
    assert!(!check.accepts(-2.1));
    assert!(!check.accepts(2.1));
    assert!(!NumberConstraint::Range { minimum: 2.0, maximum: 1.0 }.accepts(1.5));
    assert!(!NumberConstraint::Range { minimum: f64::NAN, maximum: 1.0 }.accepts(0.0));
    assert!(!NumberConstraint::Range { minimum: 0.0, maximum: f64::INFINITY }.accepts(1.0));
}

#[test]
fn nonempty_accepts_unicode_without_changing_text() {
    assert!(!nonempty(""));
    assert!(nonempty("čidlo"));
    assert!(nonempty(" "));
}
