//! Typed configuration checks at the language boundary.

/// Numeric configuration constraints. A range includes both endpoints.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HyphaNumberConstraint {
    /// Refuse NaN and infinity.
    Finite = 0,
    /// Require a finite value greater than zero.
    Positive = 1,
    /// Require a finite value between finite, ordered bounds.
    Range = 2,
    /// Require a finite value of at least zero.
    NonNegative = 3,
    /// Require a finite value less than zero.
    Negative = 4,
    /// Require a finite value of at most zero.
    NonPositive = 5,
}
