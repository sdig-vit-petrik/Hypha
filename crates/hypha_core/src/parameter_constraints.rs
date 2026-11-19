//! Reusable checks for typed configuration values.

/// A finite numeric constraint. Ranges include both endpoints.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NumberConstraint {
    /// Refuse NaN and either infinity.
    Finite,
    /// Require a finite value greater than zero.
    Positive,
    /// Require a finite value of at least zero. Negative zero is accepted.
    NonNegative,
    /// Require a finite value less than zero.
    Negative,
    /// Require a finite value of at most zero.
    NonPositive,
    /// Require a finite value in the closed interval.
    Range {
        /// The lowest accepted value.
        minimum: f64,
        /// The highest accepted value.
        maximum: f64,
    },
}

impl NumberConstraint {
    /// True when the value and constraint bounds are valid.
    #[must_use]
    pub fn accepts(self, value: f64) -> bool {
        if !value.is_finite() {
            return false;
        }
        match self {
            Self::Finite => true,
            Self::Positive => value > 0.0,
            Self::NonNegative => value >= 0.0,
            Self::Negative => value < 0.0,
            Self::NonPositive => value <= 0.0,
            Self::Range { minimum, maximum } => {
                minimum.is_finite()
                    && maximum.is_finite()
                    && minimum <= maximum
                    && value >= minimum
                    && value <= maximum
            }
        }
    }
}

/// True when text contains at least one byte.
#[must_use]
pub fn nonempty(value: &str) -> bool {
    !value.is_empty()
}
