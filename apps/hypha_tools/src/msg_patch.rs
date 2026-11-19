//! Field overrides of a ROS 1 payload, applied to its bytes against a runtime schema.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::ops::Range;

use hypha_node::sdk::codec::{CodecError, Reader};

use crate::msg_reflect::{MessageSchema, WalkEvent, format_significant, walk};

/// One builtin value, decoded.
#[derive(Debug, Clone, PartialEq)]
pub enum ScalarValue {
    /// `bool`.
    Bool(bool),
    /// `int8`, `byte`, `int16`, `int32`, `int64`.
    Int(i64),
    /// `uint8`, `char`, `uint16`, `uint32`, `uint64`.
    UInt(u64),
    /// `float32`, `float64`.
    Float(f64),
    /// `string`, as bytes: a ROS string need not be UTF-8.
    Text(Vec<u8>),
    /// `time`: seconds and nanoseconds.
    Time(u32, u32),
    /// `duration`: seconds and nanoseconds, both carrying the sign.
    Duration(i32, i32),
}

/// One override of a field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Override {
    /// Replace the value with this text, parsed against the field type when applied.
    Value(String),
    /// Set and then clear these bits of an integer.
    Bits {
        /// Bits forced to 1.
        set: u64,
        /// Bits forced to 0.
        clear: u64,
    },
}

/// One scalar of a payload: its path, type and bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    /// `a.b`, `arr[3].c`, `data[2]`.
    pub path: String,
    /// The builtin type.
    pub type_name: String,
    /// The full wire encoding in the payload.
    pub span: Range<usize>,
}

/// The base a number is shown in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Radix {
    /// Decimal.
    Dec,
    /// `0x` hexadecimal.
    Hex,
    /// `0b` binary.
    Bin,
}

/// What [`apply`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PatchReport {
    /// Overrides written.
    pub applied: usize,
    /// Overrides not written, with the reason, by path.
    pub failed: Vec<(String, String)>,
}

/// The reason given for an override whose path the message does not hold.
pub const NO_SUCH_FIELD: &str = "<no such field in this message>";

/// Byte width of an integer type, or `None` for any other type.
#[must_use]
pub fn integer_width(type_name: &str) -> Option<usize> {
    match type_name {
        "int8" | "byte" | "uint8" | "char" => Some(1),
        "int16" | "uint16" => Some(2),
        "int32" | "uint32" => Some(4),
        "int64" | "uint64" => Some(8),
        _ => None,
    }
}

fn is_signed(type_name: &str) -> bool {
    matches!(type_name, "int8" | "byte" | "int16" | "int32" | "int64")
}

/// The scalars of `bytes` as a `root` message, in wire order. With `want`, only
/// the paths it names. Byte arrays are read-only and give no slot.
///
/// # Errors
/// Returns the reason when the payload does not match the schema: a read fails,
/// bytes are left over, or a type is not defined.
pub fn index_slots(
    schema: &mut MessageSchema,
    root: &str,
    bytes: &[u8],
    want: Option<&BTreeSet<String>>,
) -> Result<Vec<Slot>, String> {
    let mut slots = Vec::new();
    let mut path: Vec<String> = Vec::new();
    let mut unresolved: Option<String> = None;
    let consumed = walk(schema, root, bytes, &mut |event| match event {
        WalkEvent::Enter { field } => path.push(field.name.clone()),
        WalkEvent::Element { index } => {
            if path.last().is_some_and(|last| last.starts_with('[')) {
                path.pop();
            }
            path.push(format!("[{index}]"));
        }
        WalkEvent::ArrayEnd => {
            if path.last().is_some_and(|last| last.starts_with('[')) {
                path.pop();
            }
        }
        WalkEvent::Leave => {
            path.pop();
        }
        WalkEvent::Scalar { type_name, span } => {
            let joined = join_path(&path);
            if want.is_none_or(|want| want.contains(&joined)) {
                slots.push(Slot { path: joined, type_name: type_name.to_owned(), span });
            }
        }
        WalkEvent::Unresolved { type_name } => {
            unresolved.get_or_insert_with(|| format!("type {type_name} is not defined"));
        }
        WalkEvent::Bytes { .. } | WalkEvent::ArrayStart { .. } => {}
    })
    .map_err(|cause: CodecError| format!("payload does not match the schema: {cause}"))?;
    if let Some(reason) = unresolved {
        return Err(reason);
    }
    if consumed != bytes.len() {
        return Err(format!(
            "payload does not match the schema: {} bytes left over",
            bytes.len().saturating_sub(consumed)
        ));
    }
    Ok(slots)
}

fn join_path(parts: &[String]) -> String {
    let mut joined = String::new();
    for part in parts {
        if !joined.is_empty() && !part.starts_with('[') {
            joined.push('.');
        }
        joined.push_str(part);
    }
    joined
}

/// Decodes one builtin value from its full wire encoding.
///
/// # Errors
/// Returns the reason when `wire` is not one value of `type_name`.
pub fn read_scalar(type_name: &str, wire: &[u8]) -> Result<ScalarValue, String> {
    let mut reader = Reader::new(wire);
    let failed = |cause: CodecError| format!("{type_name}: {cause}");
    let value = match type_name {
        "bool" => ScalarValue::Bool(reader.read_u8().map_err(failed)? != 0),
        "int8" | "byte" => ScalarValue::Int(reader.read_i8().map_err(failed)?.into()),
        "uint8" | "char" => ScalarValue::UInt(reader.read_u8().map_err(failed)?.into()),
        "int16" => ScalarValue::Int(reader.read_i16().map_err(failed)?.into()),
        "uint16" => ScalarValue::UInt(reader.read_u16().map_err(failed)?.into()),
        "int32" => ScalarValue::Int(reader.read_i32().map_err(failed)?.into()),
        "uint32" => ScalarValue::UInt(reader.read_u32().map_err(failed)?.into()),
        "int64" => ScalarValue::Int(reader.read_i64().map_err(failed)?),
        "uint64" => ScalarValue::UInt(reader.read_u64().map_err(failed)?),
        "float32" => ScalarValue::Float(reader.read_f32().map_err(failed)?.into()),
        "float64" => ScalarValue::Float(reader.read_f64().map_err(failed)?),
        "string" => ScalarValue::Text(reader.read_string().map_err(failed)?.to_vec()),
        "time" => {
            let (sec, nsec) = reader.read_time().map_err(failed)?;
            ScalarValue::Time(sec, nsec)
        }
        "duration" => {
            let (sec, nsec) = reader.read_duration().map_err(failed)?;
            ScalarValue::Duration(sec, nsec)
        }
        _ => return Err(format!("{type_name} is not a builtin type")),
    };
    if !reader.is_empty() {
        return Err(format!("{type_name}: {} bytes left over", reader.remaining()));
    }
    Ok(value)
}

/// Parses override text against `type_name`. Integers take an optional `-`, then
/// `0x`, `0b` or decimal digits; time and duration take `sec.fraction`; a string
/// takes `\xNN` and `\\` escapes.
///
/// # Errors
/// Returns the reason, naming the type, when the text is not a value of it.
pub fn parse_value(type_name: &str, text: &str) -> Result<ScalarValue, String> {
    let text = if type_name == "string" { text } else { text.trim() };
    let refused = |why: &str| format!("'{text}' is not a {type_name}: {why}");
    match type_name {
        "bool" => match text {
            "true" | "1" => Ok(ScalarValue::Bool(true)),
            "false" | "0" => Ok(ScalarValue::Bool(false)),
            _ => Err(refused("expected true or false")),
        },
        "float32" | "float64" => {
            let value: f64 = text.parse().map_err(|_| refused("not a number"))?;
            let too_large =
                type_name == "float32" && value.is_finite() && value.abs() > f64::from(f32::MAX);
            if too_large {
                return Err(refused("out of range"));
            }
            Ok(ScalarValue::Float(value))
        }
        "string" => unescape(text).map(ScalarValue::Text).map_err(|why| refused(&why)),
        "time" => {
            let (negative, sec, nsec) =
                parse_sec_nsec(text).ok_or_else(|| refused("expected sec.nsec"))?;
            if negative {
                return Err(refused("a time is not negative"));
            }
            let sec = u32::try_from(sec).map_err(|_| refused("out of range"))?;
            Ok(ScalarValue::Time(sec, nsec))
        }
        "duration" => {
            let (negative, sec, nsec) =
                parse_sec_nsec(text).ok_or_else(|| refused("expected sec.nsec"))?;
            let sec = i32::try_from(sec).map_err(|_| refused("out of range"))?;
            let nsec = i32::try_from(nsec).map_err(|_| refused("out of range"))?;
            if negative {
                Ok(ScalarValue::Duration(sec.saturating_neg(), nsec.saturating_neg()))
            } else {
                Ok(ScalarValue::Duration(sec, nsec))
            }
        }
        _ => {
            let width = integer_width(type_name).ok_or_else(|| refused("unknown type"))?;
            let value = parse_integer(text).ok_or_else(|| refused("not an integer"))?;
            integer_value(type_name, width, value).ok_or_else(|| refused("out of range"))
        }
    }
}

/// An optional `-`, then `0x`/`0b`/decimal digits.
fn parse_integer(text: &str) -> Option<i128> {
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let lower = digits.to_ascii_lowercase();
    let magnitude = if let Some(hex) = lower.strip_prefix("0x") {
        u64::from_str_radix(hex, 16).ok()?
    } else if let Some(bin) = lower.strip_prefix("0b") {
        u64::from_str_radix(bin, 2).ok()?
    } else {
        lower.parse::<u64>().ok()?
    };
    let magnitude = i128::from(magnitude);
    Some(if negative { magnitude.checked_neg()? } else { magnitude })
}

/// `value` as a value of the integer type, or `None` when it does not fit.
fn integer_value(type_name: &str, width: usize, value: i128) -> Option<ScalarValue> {
    let bits = u32::try_from(width.checked_mul(8)?).ok()?;
    if is_signed(type_name) {
        let limit = 1_i128.checked_shl(bits.checked_sub(1)?)?;
        let fits = value >= limit.checked_neg()? && value < limit;
        fits.then(|| i64::try_from(value).ok().map(ScalarValue::Int)).flatten()
    } else {
        let limit = 1_i128.checked_shl(bits)?;
        let fits = value >= 0 && value < limit;
        fits.then(|| u64::try_from(value).ok().map(ScalarValue::UInt)).flatten()
    }
}

/// `[-]sec[.fraction]` with up to 9 fraction digits: sign, seconds, nanoseconds.
fn parse_sec_nsec(text: &str) -> Option<(bool, u64, u32)> {
    let (negative, body) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (sec_text, fraction) = body.split_once('.').unwrap_or((body, ""));
    let digits_only = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
    if sec_text.is_empty() || fraction.len() > 9 || !digits_only(sec_text) || !digits_only(fraction)
    {
        return None;
    }
    let sec = sec_text.parse().ok()?;
    let padded = format!("{fraction:0<9}");
    let nsec = padded.parse().ok()?;
    Some((negative, sec, nsec))
}

fn unescape(text: &str) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::with_capacity(text.len());
    let mut rest = text.as_bytes();
    while let Some((&byte, tail)) = rest.split_first() {
        rest = tail;
        if byte != b'\\' {
            bytes.push(byte);
            continue;
        }
        match rest {
            [b'\\', tail @ ..] => {
                bytes.push(b'\\');
                rest = tail;
            }
            [b'x', high, low, tail @ ..] => {
                let pair = [*high, *low];
                let hex = std::str::from_utf8(&pair).map_err(|_| "bad \\x escape".to_owned())?;
                bytes.push(u8::from_str_radix(hex, 16).map_err(|_| "bad \\x escape".to_owned())?);
                rest = tail;
            }
            _ => return Err("an escape is \\\\ or \\xNN".to_owned()),
        }
    }
    Ok(bytes)
}

/// Encodes one value as `type_name`, in the full wire encoding.
///
/// # Errors
/// Returns the reason when the value is not of that type or does not fit it.
pub fn encode_scalar(type_name: &str, value: &ScalarValue) -> Result<Vec<u8>, String> {
    let mismatch = || format!("{value:?} is not a {type_name}");
    match (type_name, value) {
        ("bool", ScalarValue::Bool(flag)) => Ok(vec![u8::from(*flag)]),
        ("float32", ScalarValue::Float(number)) => {
            if number.is_finite() && number.abs() > f64::from(f32::MAX) {
                return Err(format!("{number} is out of range for float32"));
            }
            // The range is checked above; the narrowing rounds to the nearest float32.
            #[allow(clippy::cast_possible_truncation)]
            let narrow = *number as f32;
            Ok(narrow.to_le_bytes().to_vec())
        }
        ("float64", ScalarValue::Float(number)) => Ok(number.to_le_bytes().to_vec()),
        ("string", ScalarValue::Text(bytes)) => {
            let length = u32::try_from(bytes.len()).map_err(|_| "string too long".to_owned())?;
            let mut wire = length.to_le_bytes().to_vec();
            wire.extend_from_slice(bytes);
            Ok(wire)
        }
        ("time", ScalarValue::Time(sec, nsec)) => {
            Ok([sec.to_le_bytes(), nsec.to_le_bytes()].concat())
        }
        ("duration", ScalarValue::Duration(sec, nsec)) => {
            Ok([sec.to_le_bytes(), nsec.to_le_bytes()].concat())
        }
        (_, ScalarValue::Int(_) | ScalarValue::UInt(_)) => {
            let width = integer_width(type_name).ok_or_else(mismatch)?;
            let wide = match value {
                ScalarValue::Int(number) => i128::from(*number),
                ScalarValue::UInt(number) => i128::from(*number),
                _ => return Err(mismatch()),
            };
            integer_value(type_name, width, wide)
                .ok_or_else(|| format!("{wide} is out of range for {type_name}"))?;
            let bytes = wide.to_le_bytes();
            Ok(bytes.get(..width).ok_or_else(mismatch)?.to_vec())
        }
        _ => Err(mismatch()),
    }
}

/// Shows a value: integers in `radix`, floats with 6 significant digits, strings
/// with `\xNN` for bytes that are not printable ASCII, time as `sec.nnnnnnnnn`.
#[must_use]
pub fn format_value(value: &ScalarValue, radix: Radix) -> String {
    match value {
        ScalarValue::Bool(flag) => flag.to_string(),
        ScalarValue::Int(number) => {
            let sign = if *number < 0 { "-" } else { "" };
            format!("{sign}{}", format_magnitude(number.unsigned_abs(), radix))
        }
        ScalarValue::UInt(number) => format_magnitude(*number, radix),
        ScalarValue::Float(number) => format_significant(*number),
        ScalarValue::Text(bytes) => escape(bytes),
        ScalarValue::Time(sec, nsec) => format!("{sec}.{nsec:09}"),
        ScalarValue::Duration(sec, nsec) => {
            let sign = if *sec < 0 || *nsec < 0 { "-" } else { "" };
            format!("{sign}{}.{:09}", sec.unsigned_abs(), nsec.unsigned_abs())
        }
    }
}

fn format_magnitude(number: u64, radix: Radix) -> String {
    match radix {
        Radix::Dec => number.to_string(),
        Radix::Hex => format!("{number:#x}"),
        Radix::Bin => format!("{number:#b}"),
    }
}

/// A string's bytes as text: printable ASCII as is, `\\` for a backslash and
/// `\xNN` for every other byte. [`parse_value`] reads it back.
#[must_use]
pub fn escape(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len());
    for &byte in bytes {
        match byte {
            b'\\' => text.push_str("\\\\"),
            b' '..=b'~' => text.push(char::from(byte)),
            _ => {
                let _ = write!(text, "\\x{byte:02X}");
            }
        }
    }
    text
}

/// Writes `input` with `overrides` applied into `out`.
///
/// An override that cannot be applied (a path the message does not hold, a value
/// that does not parse, bits on a non-integer) is listed in the report, and its
/// field keeps its input bytes; the others still apply.
///
/// # Errors
/// Returns the reason when the payload does not match the schema. `out` is then not
/// a message, and the caller forwards `input` unchanged.
pub fn apply(
    schema: &mut MessageSchema,
    root: &str,
    input: &[u8],
    overrides: &BTreeMap<String, Override>,
    out: &mut Vec<u8>,
) -> Result<PatchReport, String> {
    let want: BTreeSet<String> = overrides.keys().cloned().collect();
    let slots = index_slots(schema, root, input, Some(&want))?;
    let mut report = PatchReport::default();
    out.clear();
    let mut cursor = 0;
    for slot in &slots {
        let original = input.get(slot.span.clone()).ok_or("slot outside the payload")?;
        out.extend_from_slice(input.get(cursor..slot.span.start).ok_or("slots out of order")?);
        cursor = slot.span.end;
        let Some(wanted) = overrides.get(&slot.path) else {
            out.extend_from_slice(original);
            continue;
        };
        match patched(&slot.type_name, original, wanted) {
            Ok(bytes) => {
                out.extend_from_slice(&bytes);
                report.applied = report.applied.saturating_add(1);
            }
            Err(reason) => {
                out.extend_from_slice(original);
                report.failed.push((slot.path.clone(), reason));
            }
        }
    }
    out.extend_from_slice(input.get(cursor..).ok_or("slots out of order")?);
    for path in overrides.keys() {
        if !slots.iter().any(|slot| &slot.path == path) {
            report.failed.push((path.clone(), NO_SUCH_FIELD.to_owned()));
        }
    }
    Ok(report)
}

/// The new encoding of one field.
fn patched(type_name: &str, original: &[u8], wanted: &Override) -> Result<Vec<u8>, String> {
    match wanted {
        Override::Value(text) => encode_scalar(type_name, &parse_value(type_name, text)?),
        Override::Bits { set, clear } => {
            let width = integer_width(type_name)
                .ok_or_else(|| format!("bits apply to integers, not {type_name}"))?;
            let bits = u32::try_from(width.saturating_mul(8)).unwrap_or(64);
            let outside = (set | clear).checked_shr(bits).unwrap_or(0);
            if outside != 0 {
                return Err(format!("bits beyond the {bits} bits of {type_name}"));
            }
            if original.len() != width {
                return Err(format!("{type_name} field holds {} bytes", original.len()));
            }
            let mut raw = [0_u8; 8];
            raw.get_mut(..width).ok_or("bad width")?.copy_from_slice(original);
            let next = (u64::from_le_bytes(raw) | set) & !clear;
            Ok(next.to_le_bytes().get(..width).ok_or("bad width")?.to_vec())
        }
    }
}
