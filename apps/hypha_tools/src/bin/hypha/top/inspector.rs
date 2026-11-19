//! Message inspector: schema resolution and payload decoding for the modal popup.

pub(crate) use hypha_tools::msg_reflect::SchemaCache;
use hypha_tools::msg_reflect::{MessageSchema, decode_message};

/// Decodes `payload` for `data_type` against `schema`, returning the indented dump lines.
pub(crate) fn decode_lines(
    data_type: &str,
    schema: &mut MessageSchema,
    payload: &[u8],
) -> Vec<String> {
    let mut out = String::new();
    decode_message(&mut out, data_type, schema, payload);
    out.lines().map(str::to_string).collect()
}
