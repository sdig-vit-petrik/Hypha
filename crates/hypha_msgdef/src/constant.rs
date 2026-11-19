//! The value of a constant, checked against its type.
//!
//! A constant keeps its value as written (the identity hashes that text).
//!
//! - An integer is a decimal or a `0x` hexadecimal literal with an optional
//!   sign, within the range of its type. `byte` is an `int8`, `char` a `uint8`.
//! - A float is a finite decimal literal; a `float32` must fit `f32`.
//! - A `bool` is `true`, `false`, `True`, `False`, `1` or `0`.
//! - A `string` is the rest of the line, as written.

use crate::spec::{Constant, FieldType};

/// A constant's value, read from its text.
#[derive(Debug, Clone, PartialEq)]
pub enum ConstantValue {
    /// A `bool` constant.
    Bool(bool),
    /// A signed integer constant (`byte`, `int8` to `int64`).
    Signed(i64),
    /// An unsigned integer constant (`char`, `uint8` to `uint64`).
    Unsigned(u64),
    /// A `float32` or `float64` constant.
    Float(f64),
    /// A `string` constant.
    Text(String),
}

/// Why a constant's text is not a value of its type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConstantValueError {
    /// The text is not a literal of the type.
    NotALiteral,
    /// The literal lies outside the range of the type.
    OutOfRange {
        /// The smallest value of the type, as text.
        min: String,
        /// The largest value of the type, as text.
        max: String,
    },
    /// The type cannot be the type of a constant (`time`, `duration`, a message).
    NotAConstantType,
}

/// The inclusive range of one integer type.
struct IntegerRange {
    min: i128,
    max: i128,
}

/// How an integer literal was read.
enum IntegerLiteral {
    /// A literal with this value.
    Value(i128),
    /// A well-formed literal too large for any integer type.
    TooLarge,
    /// Not an integer literal.
    Malformed,
}

/// Reads `constant.value` as a value of `constant.constant_type`.
///
/// # Errors
///
/// [`ConstantValueError`] when the text is not a literal of the type, lies
/// outside its range, or the type cannot hold a constant.
pub fn constant_value(constant: &Constant) -> Result<ConstantValue, ConstantValueError> {
    let text = constant.value.as_str();
    match &constant.constant_type {
        FieldType::Text => Ok(ConstantValue::Text(constant.value.clone())),
        FieldType::Bool => bool_value(text),
        FieldType::Float32 => float_value(text, true),
        FieldType::Float64 => float_value(text, false),
        FieldType::Time | FieldType::Duration | FieldType::Message(_) => {
            Err(ConstantValueError::NotAConstantType)
        }
        integer_type => integer_value(integer_type, text),
    }
}

fn bool_value(text: &str) -> Result<ConstantValue, ConstantValueError> {
    match text {
        "true" | "True" | "1" => Ok(ConstantValue::Bool(true)),
        "false" | "False" | "0" => Ok(ConstantValue::Bool(false)),
        _ => Err(ConstantValueError::NotALiteral),
    }
}

fn float_value(text: &str, is_float32: bool) -> Result<ConstantValue, ConstantValueError> {
    if !is_decimal_float_literal(text) {
        return Err(ConstantValueError::NotALiteral);
    }
    let value = text.parse::<f64>().map_err(|_| ConstantValueError::NotALiteral)?;
    let limit = if is_float32 { f64::from(f32::MAX) } else { f64::MAX };
    let within_type = value.is_finite() && value.abs() <= limit;
    if !within_type {
        return Err(ConstantValueError::OutOfRange {
            min: format!("{:e}", -limit),
            max: format!("{limit:e}"),
        });
    }
    Ok(ConstantValue::Float(value))
}

/// `[+-]?([0-9]+(\.[0-9]*)?|\.[0-9]+)([eE][+-]?[0-9]+)?`
fn is_decimal_float_literal(text: &str) -> bool {
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    let (mantissa, exponent) = match unsigned.find(['e', 'E']) {
        Some(at) => (unsigned.get(..at).unwrap_or(""), unsigned.get(at.saturating_add(1)..)),
        None => (unsigned, None),
    };
    let (whole, fraction) = match mantissa.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (mantissa, None),
    };
    let all_digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
    let fraction_digits = fraction.unwrap_or("");
    let has_digit = !whole.is_empty() || !fraction_digits.is_empty();
    let mantissa_ok = has_digit && all_digits(whole) && all_digits(fraction_digits);
    if !mantissa_ok {
        return false;
    }
    let Some(exponent) = exponent else {
        return true;
    };
    let exponent_digits = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
    !exponent_digits.is_empty() && all_digits(exponent_digits)
}

fn integer_value(field_type: &FieldType, text: &str) -> Result<ConstantValue, ConstantValueError> {
    let range = integer_range(field_type).ok_or(ConstantValueError::NotAConstantType)?;
    let out_of_range =
        ConstantValueError::OutOfRange { min: range.min.to_string(), max: range.max.to_string() };
    let value = match integer_literal(text) {
        IntegerLiteral::Value(value) => value,
        IntegerLiteral::TooLarge => return Err(out_of_range),
        IntegerLiteral::Malformed => return Err(ConstantValueError::NotALiteral),
    };
    let within_type = value >= range.min && value <= range.max;
    if !within_type {
        return Err(out_of_range);
    }
    let is_signed = range.min < 0;
    if is_signed {
        return i64::try_from(value).map(ConstantValue::Signed).map_err(|_| out_of_range);
    }
    u64::try_from(value).map(ConstantValue::Unsigned).map_err(|_| out_of_range)
}

fn integer_range(field_type: &FieldType) -> Option<IntegerRange> {
    let (min, max) = match field_type {
        FieldType::Byte | FieldType::Int8 => (i128::from(i8::MIN), i128::from(i8::MAX)),
        FieldType::Char | FieldType::UInt8 => (0, i128::from(u8::MAX)),
        FieldType::Int16 => (i128::from(i16::MIN), i128::from(i16::MAX)),
        FieldType::UInt16 => (0, i128::from(u16::MAX)),
        FieldType::Int32 => (i128::from(i32::MIN), i128::from(i32::MAX)),
        FieldType::UInt32 => (0, i128::from(u32::MAX)),
        FieldType::Int64 => (i128::from(i64::MIN), i128::from(i64::MAX)),
        FieldType::UInt64 => (0, i128::from(u64::MAX)),
        _ => return None,
    };
    Some(IntegerRange { min, max })
}

/// `[+-]?([0-9]+|0[xX][0-9a-fA-F]+)`
fn integer_literal(text: &str) -> IntegerLiteral {
    let (negative, unsigned) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    let hex_digits = unsigned.strip_prefix("0x").or_else(|| unsigned.strip_prefix("0X"));
    let (radix, digits) = match hex_digits {
        Some(digits) => (16, digits),
        None => (10, unsigned),
    };
    let well_formed = !digits.is_empty() && digits.chars().all(|c| c.is_digit(radix));
    if !well_formed {
        return IntegerLiteral::Malformed;
    }
    let Ok(magnitude) = i128::from_str_radix(digits, radix) else {
        return IntegerLiteral::TooLarge;
    };
    if !negative {
        return IntegerLiteral::Value(magnitude);
    }
    0_i128.checked_sub(magnitude).map_or(IntegerLiteral::TooLarge, IntegerLiteral::Value)
}
