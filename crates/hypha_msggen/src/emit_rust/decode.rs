//! Emitting the decode method.

use super::{base_type, field_ident, fixed_width, is_signed_byte, is_unsigned_byte};
use crate::put;
use hypha_msgdef::spec::{Field, FieldType, MessageSpec, Multiplicity};

/// Emits the decode method.
pub(super) fn emit_decode_method(out: &mut String, spec: &MessageSpec) {
    let reader_name = if spec.fields.is_empty() { "_reader" } else { "reader" };
    put(
        out,
        format_args!(
            "    /// Reads one message from `reader`.\n    ///\n    /// # Errors\n    /// Returns an error when the bytes run out or a field is malformed.\n    pub fn decode({reader_name}: &mut Reader<'_>) -> Result<Self, CodecError> {{\n"
        ),
    );
    for field in &spec.fields {
        put(
            out,
            format_args!("        let {} = {};\n", field_ident(&field.name), decode_field(field)),
        );
    }
    let names: Vec<String> = spec.fields.iter().map(|field| field_ident(&field.name)).collect();
    put(out, format_args!("        Ok(Self {{ {} }})\n    }}\n", names.join(", ")));
}

fn decode_field(field: &Field) -> String {
    match field.multiplicity {
        Multiplicity::Unit => decode_item(&field.field_type),
        Multiplicity::Variable => decode_variable(field),
        Multiplicity::Fixed(count) => decode_fixed(field, count),
    }
}

/// The read of one element, whether alone or inside an array.
fn decode_item(field_type: &FieldType) -> String {
    match field_type {
        FieldType::Text => "decode_string(reader)?".to_string(),
        FieldType::Time => "RosTime::decode(reader)?".to_string(),
        FieldType::Duration => "RosDuration::decode(reader)?".to_string(),
        FieldType::Message(name) => format!("{}::decode(reader)?", name.name),
        scalar => format!("reader.read_{}()?", base_type(scalar)),
    }
}

/// Smallest byte count one element of `field_type` occupies on the wire.
///
/// Zero means the width is not fixed; the reader then holds the count to its budget.
fn min_element_bytes(field_type: &FieldType) -> usize {
    match field_type {
        // A string is a four byte count and then its bytes.
        FieldType::Text => 4,
        other => fixed_width(other).unwrap_or(0),
    }
}

fn decode_variable(field: &Field) -> String {
    let id = field_ident(&field.name);
    let field_type = &field.field_type;
    let is_bytes = is_unsigned_byte(field_type) || is_signed_byte(field_type);
    let element_bytes = if is_bytes { 1 } else { min_element_bytes(field_type) };
    let length = format!(
        "{{\n            let {id}_len = reader.read_array_len_bounded({element_bytes})?;\n"
    );
    if is_unsigned_byte(field_type) {
        return length
            + &format!(
                "            ::alloc::vec::Vec::from(reader.read_bytes({id}_len)?)\n        }}"
            );
    }
    if is_signed_byte(field_type) {
        // One slice read of the bytes the count covers, each converted to `i8`.
        return length
            + &format!(
                "            reader.read_bytes({id}_len)?.iter().map(|&byte| i8::from_ne_bytes([byte])).collect::<::alloc::vec::Vec<i8>>()\n        }}"
            );
    }
    // A message element has no known width: the count is held to the element budget and
    // nothing is reserved. A known width bounds the count by the bytes that remain.
    let width_is_known = element_bytes > 0;
    let allocation = if width_is_known {
        format!("::alloc::vec::Vec::with_capacity({id}_len)")
    } else {
        "::alloc::vec::Vec::new()".to_string()
    };
    let item = decode_item(field_type);
    length
        + &format!(
            "            let mut {id} = {allocation};\n            for _ in 0..{id}_len {{\n                {id}.push({item});\n            }}\n            {id}\n        }}"
        )
}

fn decode_fixed(field: &Field, count: usize) -> String {
    let id = field_ident(&field.name);
    let field_type = &field.field_type;
    // One slice read of `count` bytes, as a `[u8; count]`.
    let bytes = format!(
        "<[u8; {count}]>::try_from(reader.read_bytes({count})?)\n            .map_err(|_| CodecError::Truncated {{ needed: {count}, available: 0 }})?"
    );
    if is_unsigned_byte(field_type) {
        return bytes;
    }
    if is_signed_byte(field_type) {
        return bytes + "\n            .map(|byte| i8::from_ne_bytes([byte]))";
    }
    let item = decode_item(field_type);
    let element = base_type(field_type);
    format!(
        "{{\n            let mut {id}_items = ::alloc::vec::Vec::with_capacity({count});\n            for _ in 0..{count} {{\n                {id}_items.push({item});\n            }}\n            let {id}: [{element}; {count}] =\n                {id}_items.try_into().map_err(|_| CodecError::Truncated {{ needed: {count}, available: 0 }})?;\n            {id}\n        }}"
    )
}

pub(super) fn emit_decode_string_helper(out: &mut String) {
    out.push_str(
        "/// Reads one length-prefixed string field as text.
fn decode_string(reader: &mut Reader<'_>) -> Result<::alloc::string::String, CodecError> {
    let bytes = reader.read_string()?;
    let text = core::str::from_utf8(bytes).map_err(|_| CodecError::InvalidUtf8)?;
    Ok(::alloc::string::String::from(text))
}
",
    );
}
