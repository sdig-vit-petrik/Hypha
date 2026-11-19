//! Tests for Rust message source generation.

use hypha_msgdef::spec::{Constant, Field, FieldType, MessageSpec, Multiplicity, TypeName};
use hypha_msggen::emit_rust::emit_message;

fn field(field_type: FieldType, multiplicity: Multiplicity, name: &str) -> Field {
    Field { field_type, multiplicity, name: String::from(name) }
}

fn constant(constant_type: FieldType, name: &str, value: &str) -> Constant {
    Constant { constant_type, name: String::from(name), value: String::from(value) }
}

fn spec_with_fields(type_name: TypeName, fields: Vec<Field>) -> MessageSpec {
    MessageSpec { type_name, constants: Vec::new(), fields }
}

fn spec_with_constants_and_fields(
    type_name: TypeName,
    constants: Vec<Constant>,
    fields: Vec<Field>,
) -> MessageSpec {
    MessageSpec { type_name, constants, fields }
}

#[test]
fn header_exact_output() {
    let spec = spec_with_fields(
        TypeName::new("std_msgs", "Header"),
        vec![
            field(FieldType::UInt32, Multiplicity::Unit, "seq"),
            field(FieldType::Time, Multiplicity::Unit, "stamp"),
            field(FieldType::Text, Multiplicity::Unit, "frame_id"),
        ],
    );

    let expected = r#"//! Generated from the `std_msgs/Header` definition. Do not edit.

use hypha_core::message::{DecodeOwned, EncodeInto, MessageIdentity};
use hypha_wire::codec::{CodecError, Reader};
use hypha_wire::ros_time::{RosTime};
use hypha_wire::slice_writer::SliceWriter;

/// The `std_msgs/Header` message.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Header {
    /// The `seq` field.
    pub seq: u32,
    /// The `stamp` field.
    pub stamp: RosTime,
    /// The `frame_id` field.
    pub frame_id: ::alloc::string::String,
}

impl Header {
    /// The full type name this struct was generated from.
    pub const TYPE_NAME: &'static str = "std_msgs/Header";

    /// The identity a receiver checks before decoding.
    pub const SCHEMA_HASH: &'static str = "2176decaecbce78abc3b96ef049fabed";

    /// Exactly how many bytes this message will encode to.
    #[must_use]
    pub fn encoded_len(&self) -> usize {
        let mut total = 0usize;
        total = total.saturating_add(4);
        total = total.saturating_add(8);
        total = total.saturating_add(4usize.saturating_add(self.frame_id.len()));
        total
    }

    /// Writes this message into a fixed-size buffer.
    ///
    /// # Errors
    /// Returns an error if the buffer is too small.
    pub fn encode(&self, writer: &mut SliceWriter<'_>) -> Result<(), CodecError> {
        writer.write_u32(self.seq)?;
        writer.write_time(self.stamp.secs, self.stamp.nsecs)?;
        writer.write_str(&self.frame_id)?;
        Ok(())
    }

    /// Reads one message from `reader`.
    ///
    /// # Errors
    /// Returns an error when the bytes run out or a field is malformed.
    pub fn decode(reader: &mut Reader<'_>) -> Result<Self, CodecError> {
        let seq = reader.read_u32()?;
        let stamp = RosTime::decode(reader)?;
        let frame_id = decode_string(reader)?;
        Ok(Self { seq, stamp, frame_id })
    }
}

impl MessageIdentity for Header {
    const TYPE_NAME: &'static str = "std_msgs/Header";
    const SCHEMA_HASH: &'static str = "2176decaecbce78abc3b96ef049fabed";
}

impl EncodeInto for Header {
    fn encoded_len(&self) -> usize {
        Self::encoded_len(self)
    }

    fn encode_into(&self, buffer: &mut [u8]) -> Result<usize, CodecError> {
        let mut writer = SliceWriter::new(buffer);
        Self::encode(self, &mut writer)?;
        Ok(writer.written())
    }
}

impl DecodeOwned for Header {
    fn decode_from(bytes: &[u8]) -> Result<Self, CodecError> {
        let mut reader = Reader::new(bytes);
        Self::decode(&mut reader)
    }
}
/// Reads one length-prefixed string field as text.
fn decode_string(reader: &mut Reader<'_>) -> Result<::alloc::string::String, CodecError> {
    let bytes = reader.read_string()?;
    let text = core::str::from_utf8(bytes).map_err(|_| CodecError::InvalidUtf8)?;
    Ok(::alloc::string::String::from(text))
}
"#;

    assert_eq!(emit_message(&spec, "2176decaecbce78abc3b96ef049fabed"), expected);
}

#[test]
fn single_float64_field() {
    let spec = spec_with_fields(
        TypeName::new("my_msgs", "Data"),
        vec![field(FieldType::Float64, Multiplicity::Unit, "data")],
    );

    let output = emit_message(&spec, "abc123");

    assert!(output.contains("pub struct Data {"));
    assert!(output.contains("pub data: f64,"));
    assert!(output.contains("pub const TYPE_NAME: &'static str = \"my_msgs/Data\";"));
    assert!(output.contains("writer.write_f64(self.data)?;"));
    assert!(output.contains("let data = reader.read_f64()?;"));
}

#[test]
fn variable_array_of_nested_type() {
    let spec = spec_with_fields(
        TypeName::new("geometry_msgs", "PointCloud"),
        vec![field(
            FieldType::Message(TypeName::new("geometry_msgs", "Point")),
            Multiplicity::Variable,
            "points",
        )],
    );

    let output = emit_message(&spec, "hash");

    assert!(output.contains("pub points: ::alloc::vec::Vec<Point>,"));
    assert!(output.contains("writer.write_array_len(self.points.len())?;"));
    assert!(output.contains("for item in &self.points {"));
    assert!(output.contains("item.encode(writer)?;"));
    // A message element has no fixed width: the count is held to the element budget.
    assert!(output.contains("let points_len = reader.read_array_len_bounded(0)?;"));
    assert!(output.contains("let mut points = ::alloc::vec::Vec::new();"));
    assert!(!output.contains("with_capacity(points_len)"));
    assert!(output.contains("for _ in 0..points_len {"));
    assert!(output.contains("points.push(Point::decode(reader)?);"));
}

#[test]
fn fixed_array() {
    let spec = spec_with_fields(
        TypeName::new("my_msgs", "Cov"),
        vec![field(FieldType::Float64, Multiplicity::Fixed(3), "covariance")],
    );

    let output = emit_message(&spec, "xyz");

    assert!(output.contains("pub covariance: [f64; 3],"));
    assert!(output.contains("for item in &self.covariance {"));
    assert!(output.contains("writer.write_f64(*item)?;"));
    assert!(output.contains("let mut covariance_items = ::alloc::vec::Vec::with_capacity(3);"));
    assert!(output.contains("for _ in 0..3 {"));
    assert!(output.contains("covariance_items.push(reader.read_f64()?);"));
    assert!(output.contains("CodecError::Truncated { needed: 3, available: 0 }"));
}

#[test]
fn with_constants() {
    let spec = spec_with_constants_and_fields(
        TypeName::new("my_msgs", "Status"),
        vec![
            constant(FieldType::Int32, "STATE_IDLE", "0"),
            constant(FieldType::Int32, "STATE_RUNNING", "1"),
            constant(FieldType::Text, "MODE_DEFAULT", "default_mode"),
        ],
        vec![field(FieldType::Int32, Multiplicity::Unit, "state")],
    );

    let output = emit_message(&spec, "const_hash");

    assert!(output.contains("/// The `STATE_IDLE` constant."));
    assert!(output.contains("pub const STATE_IDLE: i32 = 0;"));
    assert!(output.contains("/// The `STATE_RUNNING` constant."));
    assert!(output.contains("pub const STATE_RUNNING: i32 = 1;"));
    assert!(output.contains("/// The `MODE_DEFAULT` constant."));
    assert!(output.contains("pub const MODE_DEFAULT: &'static str = \"default_mode\";"));
}

#[test]
fn keyword_field_name() {
    let spec = spec_with_fields(
        TypeName::new("my_msgs", "KeywordMsg"),
        vec![field(FieldType::UInt8, Multiplicity::Unit, "type")],
    );

    let output = emit_message(&spec, "kw_hash");

    assert!(output.contains("pub r#type: u8,"));
    assert!(output.contains("let r#type = reader.read_u8()?;"));
    assert!(output.contains("writer.write_u8(self.r#type)?;"));
}

#[test]
fn duration_field() {
    let spec = spec_with_fields(
        TypeName::new("my_msgs", "WithDuration"),
        vec![field(FieldType::Duration, Multiplicity::Unit, "elapsed")],
    );

    let output = emit_message(&spec, "dur_hash");

    assert!(output.contains("pub elapsed: RosDuration,"));
    assert!(output.contains("writer.write_duration(self.elapsed.secs, self.elapsed.nsecs)?;"));
    assert!(output.contains("let elapsed = RosDuration::decode(reader)?;"));
}

#[test]
fn all_scalar_types() {
    let spec = spec_with_fields(
        TypeName::new("test_msgs", "AllTypes"),
        vec![
            field(FieldType::Bool, Multiplicity::Unit, "b"),
            field(FieldType::Byte, Multiplicity::Unit, "by"),
            field(FieldType::Char, Multiplicity::Unit, "ch"),
            field(FieldType::Int8, Multiplicity::Unit, "i8"),
            field(FieldType::UInt8, Multiplicity::Unit, "u8"),
            field(FieldType::Int16, Multiplicity::Unit, "i16"),
            field(FieldType::UInt16, Multiplicity::Unit, "u16"),
            field(FieldType::Int32, Multiplicity::Unit, "i32"),
            field(FieldType::UInt32, Multiplicity::Unit, "u32"),
            field(FieldType::Int64, Multiplicity::Unit, "i64"),
            field(FieldType::UInt64, Multiplicity::Unit, "u64"),
            field(FieldType::Float32, Multiplicity::Unit, "f32"),
            field(FieldType::Float64, Multiplicity::Unit, "f64"),
        ],
    );

    let output = emit_message(&spec, "all_types_hash");

    assert!(output.contains("pub b: bool,"));
    // ROS 1: `byte` is int8 and `char` is uint8.
    assert!(output.contains("pub by: i8,"));
    assert!(output.contains("pub ch: u8,"));
    assert!(output.contains("let by = reader.read_i8()?;"));
    assert!(output.contains("let ch = reader.read_u8()?;"));
    assert!(output.contains("writer.write_i8(self.by)?;"));
    assert!(output.contains("writer.write_u8(self.ch)?;"));
    assert!(output.contains("pub i8: i8,"));
    assert!(output.contains("pub u8: u8,"));
    assert!(output.contains("pub i16: i16,"));
    assert!(output.contains("pub u16: u16,"));
    assert!(output.contains("pub i32: i32,"));
    assert!(output.contains("pub u32: u32,"));
    assert!(output.contains("pub i64: i64,"));
    assert!(output.contains("pub u64: u64,"));
    assert!(output.contains("pub f32: f32,"));
    assert!(output.contains("pub f64: f64,"));
}

#[test]
fn variable_array_of_scalars() {
    let spec = spec_with_fields(
        TypeName::new("test_msgs", "ScalarArray"),
        vec![field(FieldType::Int32, Multiplicity::Variable, "values")],
    );

    let output = emit_message(&spec, "scalar_array_hash");

    assert!(output.contains("pub values: ::alloc::vec::Vec<i32>,"));
    assert!(output.contains("writer.write_array_len(self.values.len())?;"));
    assert!(output.contains("for item in &self.values {"));
    assert!(output.contains("writer.write_i32(*item)?;"));
}

#[test]
fn impossible_constant_silently_skipped() {
    let bad_constant = Constant {
        constant_type: FieldType::Message(TypeName::new("geometry_msgs", "Point")),
        name: String::from("IMPOSSIBLE"),
        value: String::from("ignored"),
    };

    let spec = spec_with_constants_and_fields(
        TypeName::new("test_msgs", "BadConst"),
        vec![bad_constant],
        vec![field(FieldType::Bool, Multiplicity::Unit, "flag")],
    );

    let output = emit_message(&spec, "bad_hash");

    assert!(!output.contains("IMPOSSIBLE"), "impossible constant should not appear in output");
    assert!(output.contains("pub struct BadConst"), "struct should still be generated");
    assert!(output.contains("pub flag: bool"), "fields should still be encoded");
}

#[test]
// The test checks deterministic sorted imports by position.
#[allow(clippy::indexing_slicing)]
fn nested_types_use_statements_sorted_and_deduped() {
    let spec = spec_with_fields(
        TypeName::new("nav_msgs", "Path"),
        vec![
            field(
                FieldType::Message(TypeName::new("std_msgs", "Header")),
                Multiplicity::Unit,
                "header",
            ),
            field(
                FieldType::Message(TypeName::new("geometry_msgs", "Point")),
                Multiplicity::Variable,
                "points",
            ),
            field(
                FieldType::Message(TypeName::new("std_msgs", "Header")),
                Multiplicity::Unit,
                "footer",
            ),
        ],
    );

    let output = emit_message(&spec, "test_hash");

    assert!(output.contains("use crate::geometry_msgs::point::Point;"));
    assert!(output.contains("use crate::std_msgs::header::Header;"));

    let lines: Vec<&str> = output.lines().collect();
    let mut use_lines: Vec<&str> =
        lines.iter().filter(|&&line| line.starts_with("use crate::")).copied().collect();

    assert_eq!(
        use_lines.len(),
        2,
        "should have 2 use lines (std_msgs/Header appears twice but deduped)"
    );

    use_lines.sort_unstable();
    assert_eq!(use_lines[0], "use crate::geometry_msgs::point::Point;");
    assert_eq!(use_lines[1], "use crate::std_msgs::header::Header;");
}

#[test]
fn encoded_len_with_mixed_types() {
    let spec = spec_with_fields(
        TypeName::new("test_msgs", "MixedTypes"),
        vec![
            field(FieldType::UInt32, Multiplicity::Unit, "id"),
            field(FieldType::Text, Multiplicity::Unit, "name"),
            field(
                FieldType::Message(TypeName::new("geometry_msgs", "Point")),
                Multiplicity::Unit,
                "pos",
            ),
        ],
    );

    let output = emit_message(&spec, "mixed_hash");

    assert!(output.contains("pub fn encoded_len(&self) -> usize"));
    assert!(output.contains("total = total.saturating_add(4);"));
    assert!(
        output.contains("total = total.saturating_add(4usize.saturating_add(self.name.len()));")
    );
    assert!(output.contains("total = total.saturating_add(self.pos.encoded_len());"));
}

#[test]
fn encoded_len_variable_array() {
    let spec = spec_with_fields(
        TypeName::new("test_msgs", "VarPoints"),
        vec![field(
            FieldType::Message(TypeName::new("geometry_msgs", "Point")),
            Multiplicity::Variable,
            "points",
        )],
    );

    let output = emit_message(&spec, "var_points_hash");

    assert!(output.contains("pub fn encoded_len(&self) -> usize"));
    assert!(output.contains("total = total.saturating_add(4);"));
    assert!(output.contains("for item in &self.points {"));
    assert!(output.contains("total = total.saturating_add(item.encoded_len());"));
}

#[test]
fn byte_arrays_are_copied_as_one_slice() {
    let spec = spec_with_fields(
        TypeName::new("my_msgs", "Bytes"),
        vec![
            field(FieldType::UInt8, Multiplicity::Fixed(4), "id"),
            field(FieldType::Char, Multiplicity::Variable, "data"),
        ],
    );

    let output = emit_message(&spec, "hash");

    assert!(output.contains("writer.write_bytes(&self.id)?;"));
    assert!(output.contains(
        "writer.write_array_len(self.data.len())?;\n        writer.write_bytes(&self.data)?;"
    ));
    assert!(output.contains(
        "let id = <[u8; 4]>::try_from(reader.read_bytes(4)?)\n            .map_err(|_| CodecError::Truncated { needed: 4, available: 0 })?;"
    ));
    assert!(output.contains(
        "let data_len = reader.read_array_len_bounded(1)?;\n            ::alloc::vec::Vec::from(reader.read_bytes(data_len)?)"
    ));
    assert!(!output.contains("write_u8(*item)"));
    assert!(!output.contains("read_u8()"));
}

#[test]
fn a_byte_array_is_signed_and_still_read_in_one_call() {
    // `byte` is int8, so its bytes are read as one slice and converted, not read one by one.
    let spec = spec_with_fields(
        TypeName::new("my_msgs", "SignedBytes"),
        vec![field(FieldType::Byte, Multiplicity::Variable, "data")],
    );

    let output = emit_message(&spec, "hash");

    assert!(output.contains("pub data: ::alloc::vec::Vec<i8>,"));
    assert!(output.contains(
        "reader.read_bytes(data_len)?.iter().map(|&byte| i8::from_ne_bytes([byte])).collect::<::alloc::vec::Vec<i8>>()"
    ));
    assert!(output.contains(
        "writer.write_bytes(&self.data.iter().map(|&value| u8::from_ne_bytes(value.to_ne_bytes())).collect::<::alloc::vec::Vec<u8>>())?;"
    ));
    assert!(!output.contains("data.push(reader.read_i8()?);"));
    assert!(!output.contains("write_i8(*item)"));
}

#[test]
fn a_fixed_byte_array_is_signed_and_copied_as_one_array() {
    let spec = spec_with_fields(
        TypeName::new("my_msgs", "SignedFixed"),
        vec![field(FieldType::Int8, Multiplicity::Fixed(4), "data")],
    );

    let output = emit_message(&spec, "hash");

    assert!(output.contains(
        "let data = <[u8; 4]>::try_from(reader.read_bytes(4)?)\n            .map_err(|_| CodecError::Truncated { needed: 4, available: 0 })?\n            .map(|byte| i8::from_ne_bytes([byte]));"
    ));
    assert!(output.contains(
        "writer.write_bytes(&self.data.map(|value| u8::from_ne_bytes(value.to_ne_bytes())))?;"
    ));
    assert!(!output.contains("write_i8(*item)"));
}

#[test]
fn variable_arrays_bound_their_count_by_element_width() {
    // A fixed-width element bounds its count by its width, a string by its length prefix.
    let cases = [
        ("Samples", FieldType::Float64, "values", 8),
        ("Blob", FieldType::UInt8, "data", 1),
        ("Labels", FieldType::Text, "names", 4),
    ];
    for (name, field_type, field_name, width) in cases {
        let spec = spec_with_fields(
            TypeName::new("my_msgs", name),
            vec![field(field_type, Multiplicity::Variable, field_name)],
        );
        let output = emit_message(&spec, "hash");
        let expected = format!("let {field_name}_len = reader.read_array_len_bounded({width})?;");
        assert!(output.contains(&expected), "{name}: {expected} is missing");
    }
}

#[test]
fn a_float_constant_written_as_an_integer_compiles_as_rust() {
    let spec = spec_with_constants_and_fields(
        TypeName::new("my_pkg", "Sectors"),
        vec![
            constant(FieldType::Float64, "UNKNOWN", "-1"),
            constant(FieldType::Float32, "HALF", ".5"),
            constant(FieldType::Bool, "ON", "1"),
            constant(FieldType::UInt32, "MASK", "0x10"),
        ],
        Vec::new(),
    );
    let source = emit_message(&spec, "00000000000000000000000000000000");
    for line in [
        "    pub const UNKNOWN: f64 = -1.0_f64;\n",
        "    pub const HALF: f32 = 0.5_f32;\n",
        "    pub const ON: bool = true;\n",
        "    pub const MASK: u32 = 16;\n",
    ] {
        assert!(source.contains(line), "missing {line:?} in\n{source}");
    }
}
