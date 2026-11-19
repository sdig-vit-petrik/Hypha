//! Flat, comma-separated rows for `hypha topic echo -p`.
//!
//! One message becomes one row. A nested field gets the name `outer.inner`, an
//! array element the name `field0`, `field1`, and so on, and a `time` or
//! `duration` the two columns `field.secs` and `field.nsecs`.
use crate::msg_reflect::{MessageSchema, ReflectedField, is_builtin_type};
use hypha_node::sdk::codec::{CodecError, Reader};
use hypha_node::sdk::msgdef::spec::MAX_NESTING_DEPTH;

/// One column of a row: the flattened field name and the printed value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlotColumn {
    /// Flattened field name.
    pub name: String,
    /// Printed value.
    pub value: String,
}

/// Flattens one wire payload into columns.
///
/// # Errors
/// Returns the reason, as text, when the type is not defined, the payload is
/// shorter than the schema needs, or the definition nests deeper than
/// [`MAX_NESTING_DEPTH`]. A caller writes no row in that case.
pub fn plot_values(
    type_name: &str,
    schema: &mut MessageSchema,
    bytes: &[u8],
) -> Result<Vec<PlotColumn>, String> {
    let Some(fields) = schema.fields_for(type_name) else {
        return Err(undefined(type_name));
    };
    let mut reader = Reader::new(bytes);
    let mut columns = Vec::new();
    flatten_fields(&mut columns, "", &fields, &mut reader, schema, 1)?;
    Ok(columns)
}

/// The header line: `%time` and the column names.
#[must_use]
pub fn plot_header(columns: &[PlotColumn]) -> String {
    let names: Vec<&str> = columns.iter().map(|column| column.name.as_str()).collect();
    format!("%time,{}", names.join(","))
}

/// One row: the publish time in nanoseconds and the column values.
#[must_use]
pub fn plot_row(stamp_nanos: u64, columns: &[PlotColumn]) -> String {
    let values: Vec<&str> = columns.iter().map(|column| column.value.as_str()).collect();
    format!("{stamp_nanos},{}", values.join(","))
}

/// Flattens the fields of one message. `level` is that message's depth, the root being 1.
fn flatten_fields(
    columns: &mut Vec<PlotColumn>,
    prefix: &str,
    fields: &[ReflectedField],
    reader: &mut Reader<'_>,
    schema: &mut MessageSchema,
    level: usize,
) -> Result<(), String> {
    let nests_too_deep = level > MAX_NESTING_DEPTH;
    if nests_too_deep {
        return Err(nesting_error());
    }
    for field in fields {
        let name = format!("{prefix}{}", field.name);
        if !field.is_array {
            flatten_value(columns, &name, &field.type_name, reader, schema, level)?;
            continue;
        }
        // Every element takes at least one byte, so a longer count is a corrupt length.
        let count = if field.is_fixed_length {
            field.array_length
        } else {
            from_codec(reader.read_array_len_bounded(1))?
        };
        for index in 0..count {
            let element = format!("{name}{index}");
            flatten_value(columns, &element, &field.type_name, reader, schema, level)?;
        }
    }
    Ok(())
}

/// Flattens one value of a field whose message is at `level`.
fn flatten_value(
    columns: &mut Vec<PlotColumn>,
    name: &str,
    type_name: &str,
    reader: &mut Reader<'_>,
    schema: &mut MessageSchema,
    level: usize,
) -> Result<(), String> {
    if !is_builtin_type(type_name) {
        let Some(inner) = schema.fields_for(type_name) else {
            return Err(undefined(type_name));
        };
        let nested_level = level.saturating_add(1);
        return flatten_fields(columns, &format!("{name}."), &inner, reader, schema, nested_level);
    }
    if type_name == "time" {
        let (secs, nsecs) = from_codec(reader.read_time())?;
        push_stamp(columns, name, secs.to_string(), nsecs.to_string());
        return Ok(());
    }
    if type_name == "duration" {
        let (secs, nsecs) = from_codec(reader.read_duration())?;
        push_stamp(columns, name, secs.to_string(), nsecs.to_string());
        return Ok(());
    }
    let value = scalar_text(type_name, reader)?;
    push(columns, name.to_owned(), value);
    Ok(())
}

fn push(columns: &mut Vec<PlotColumn>, name: String, value: String) {
    columns.push(PlotColumn { name, value });
}

/// A `time` or `duration` is two columns, `name.secs` and `name.nsecs`.
fn push_stamp(columns: &mut Vec<PlotColumn>, name: &str, secs: String, nsecs: String) {
    push(columns, format!("{name}.secs"), secs);
    push(columns, format!("{name}.nsecs"), nsecs);
}

fn scalar_text(type_name: &str, reader: &mut Reader<'_>) -> Result<String, String> {
    let text = match type_name {
        "bool" => u8::from(from_codec(reader.read_u8())? != 0).to_string(),
        "byte" | "int8" => from_codec(reader.read_i8())?.to_string(),
        "char" | "uint8" => from_codec(reader.read_u8())?.to_string(),
        "int16" => from_codec(reader.read_i16())?.to_string(),
        "uint16" => from_codec(reader.read_u16())?.to_string(),
        "int32" => from_codec(reader.read_i32())?.to_string(),
        "uint32" => from_codec(reader.read_u32())?.to_string(),
        "int64" => from_codec(reader.read_i64())?.to_string(),
        "uint64" => from_codec(reader.read_u64())?.to_string(),
        "float32" => float_text(f64::from(from_codec(reader.read_f32())?)),
        "float64" => float_text(from_codec(reader.read_f64())?),
        _ => String::from_utf8_lossy(from_codec(reader.read_string())?).into_owned(),
    };
    Ok(text)
}

/// The text of a codec failure, so the caller can name what went wrong.
fn from_codec<T>(result: Result<T, CodecError>) -> Result<T, String> {
    result.map_err(|cause| cause.to_string())
}

fn nesting_error() -> String {
    CodecError::NestingTooDeep { limit: MAX_NESTING_DEPTH }.to_string()
}

fn undefined(type_name: &str) -> String {
    format!("type {type_name} is not defined in its schema")
}

/// Shortest text that reads back to the same value; `nan` and `inf` as Octave reads them.
fn float_text(value: f64) -> String {
    if value.is_nan() {
        return "nan".to_owned();
    }
    if value.is_infinite() {
        return if value.is_sign_negative() { "-inf" } else { "inf" }.to_owned();
    }
    value.to_string()
}
