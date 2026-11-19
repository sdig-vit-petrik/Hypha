//! Emitting the encode method and `encoded_len` method.

use super::{base_type, field_ident, fixed_width, is_signed_byte, is_unsigned_byte};
use crate::put;
use hypha_msgdef::spec::{Field, FieldType, MessageSpec, Multiplicity};

/// Emits the `encoded_len` method that computes the exact byte count.
pub(super) fn emit_encoded_len_method(out: &mut String, spec: &MessageSpec) {
    out.push_str("    /// Exactly how many bytes this message will encode to.\n");
    out.push_str("    #[must_use]\n    pub fn encoded_len(&self) -> usize {\n");
    if spec.fields.is_empty() {
        out.push_str("        0usize\n    }\n\n");
        return;
    }
    out.push_str("        let mut total = 0usize;\n");
    for field in &spec.fields {
        out.push_str(&encoded_len_field(field));
    }
    out.push_str("        total\n    }\n\n");
}

/// The byte count of one element, as source: `target` names the element.
fn item_len(field_type: &FieldType, target: &str) -> String {
    match (fixed_width(field_type), field_type) {
        (Some(width), _) => width.to_string(),
        (None, FieldType::Text) => format!("4usize.saturating_add({target}.len())"),
        (None, _) => format!("{target}.encoded_len()"),
    }
}

fn add_to_total(len: &str) -> String {
    format!("        total = total.saturating_add({len});\n")
}

fn encoded_len_loop(item: &str, field: &Field, len: &str) -> String {
    format!(
        "        for {item} in &self.{} {{\n    {}        }}\n",
        field_ident(&field.name),
        add_to_total(len)
    )
}

fn encoded_len_field(field: &Field) -> String {
    let field_type = &field.field_type;
    match field.multiplicity {
        Multiplicity::Unit => {
            add_to_total(&item_len(field_type, &format!("self.{}", field_ident(&field.name))))
        }
        Multiplicity::Variable => {
            let item_is_used = matches!(field_type, FieldType::Text | FieldType::Message(_));
            let item = if item_is_used { "item" } else { "_item" };
            add_to_total("4") + &encoded_len_loop(item, field, &item_len(field_type, "item"))
        }
        Multiplicity::Fixed(count) => match (fixed_width(field_type), field_type) {
            (Some(width), _) => add_to_total(&count.saturating_mul(width).to_string()),
            (None, FieldType::Text) => encoded_len_loop("item", field, "4 + item.len()"),
            (None, _) => encoded_len_loop("item", field, "item.encoded_len()"),
        },
    }
}

/// Emits the encode method using `SliceWriter`.
pub(super) fn emit_encode_method(out: &mut String, spec: &MessageSpec) {
    let writer_name = if spec.fields.is_empty() { "_writer" } else { "writer" };
    put(
        out,
        format_args!(
            "    /// Writes this message into a fixed-size buffer.\n    ///\n    /// # Errors\n    /// Returns an error if the buffer is too small.\n    pub fn encode(&self, {writer_name}: &mut SliceWriter<'_>) -> Result<(), CodecError> {{\n"
        ),
    );
    for field in &spec.fields {
        put(out, format_args!("        {}\n", encode_field(field)));
    }
    out.push_str("        Ok(())\n    }\n\n");
}

fn encode_field(field: &Field) -> String {
    let id = field_ident(&field.name);
    let field_type = &field.field_type;
    let place = format!("self.{id}");
    match field.multiplicity {
        Multiplicity::Unit => encode_statement(field_type, &place, false),
        Multiplicity::Variable => {
            let length = format!("writer.write_array_len({place}.len())?;\n        ");
            if is_unsigned_byte(field_type) {
                length + &format!("writer.write_bytes(&{place})?;")
            } else if is_signed_byte(field_type) {
                // The bytes are converted into one temporary slice, written with one call.
                length
                    + &format!(
                        "writer.write_bytes(&{place}.iter().map(|&value| u8::from_ne_bytes(value.to_ne_bytes())).collect::<::alloc::vec::Vec<u8>>())?;"
                    )
            } else {
                length + &encode_loop(&place, field_type)
            }
        }
        Multiplicity::Fixed(_) => {
            if is_unsigned_byte(field_type) {
                format!("writer.write_bytes(&{place})?;")
            } else if is_signed_byte(field_type) {
                // A stack array of the same bytes, written with one call and no allocation.
                format!(
                    "writer.write_bytes(&{place}.map(|value| u8::from_ne_bytes(value.to_ne_bytes())))?;"
                )
            } else {
                encode_loop(&place, field_type)
            }
        }
    }
}

fn encode_loop(place: &str, field_type: &FieldType) -> String {
    format!(
        "for item in &{place} {{\n            {}\n        }}",
        encode_statement(field_type, "item", true)
    )
}

/// One write of a value of `field_type` at `target`, a reference when `is_reference`.
fn encode_statement(field_type: &FieldType, target: &str, is_reference: bool) -> String {
    match field_type {
        FieldType::Text if is_reference => format!("writer.write_str({target})?;"),
        FieldType::Text => format!("writer.write_str(&{target})?;"),
        FieldType::Time => format!("writer.write_time({target}.secs, {target}.nsecs)?;"),
        FieldType::Duration => format!("writer.write_duration({target}.secs, {target}.nsecs)?;"),
        FieldType::Message(_) => format!("{target}.encode(writer)?;"),
        scalar if is_reference => format!("writer.write_{}(*{target})?;", base_type(scalar)),
        scalar => format!("writer.write_{}({target})?;", base_type(scalar)),
    }
}
