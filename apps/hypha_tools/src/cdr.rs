//! CDR, the serialization of ROS 2, to the ROS 1 serialization of hypha, driven by the
//! ROS 1 definition of a type.
//!
//! The two encodings hold the same fields in the same order. They differ in four ways:
//! - CDR aligns each primitive to its size, counted from the end of its 4-byte
//!   encapsulation header; ROS 1 packs them.
//! - A CDR string's length counts its terminating NUL; a ROS 1 string has none.
//! - ROS 2's `std_msgs/Header` has no `seq`: the transcoder writes the caller's sequence
//!   number in its place.
//! - ROS 2 spells `time` and `duration` as `builtin_interfaces/Time` and `Duration`, an
//!   `int32` second and a `uint32` nanosecond. ROS 1's `time` is unsigned, so a negative
//!   second is refused.
//!
//! Only plain little-endian CDR (XCDR1, encapsulation `00 01`) is read, which is what
//! rosbag2 records. Any other encapsulation, a message that ends early, or one with more
//! than its trailing alignment left over is an error that names what was found.

use hypha_node::sdk::msgdef::resolve::SpecSet;
use hypha_node::sdk::msgdef::spec::{Field, FieldType, Multiplicity, TypeName};

/// Bytes of the CDR encapsulation header in front of every message.
const ENCAPSULATION: usize = 4;
/// The encapsulation of little-endian plain CDR.
const CDR_LITTLE_ENDIAN: [u8; 2] = [0x00, 0x01];
/// The most bytes a message may hold after its last field: alignment padding.
const MAX_TRAILING_PADDING: usize = 7;

/// Why a CDR message could not be transcoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CdrError {
    /// The encapsulation header is not little-endian plain CDR.
    Encapsulation([u8; 2]),
    /// The message ended before a field: `needed` bytes at `offset`.
    Truncated {
        /// The offset in the message where the field starts.
        offset: usize,
        /// The bytes the field needs.
        needed: usize,
    },
    /// More bytes than alignment padding follow the last field.
    Trailing(usize),
    /// A `time` field holds a negative second, which ROS 1's unsigned `time` cannot.
    NegativeTime(i32),
    /// A `duration` field holds a nanosecond count above `int32`.
    DurationNanoseconds(u32),
    /// A field names a type that has no definition.
    MissingType(String),
    /// A count or a size does not fit this machine.
    TooLarge,
}

impl std::fmt::Display for CdrError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Encapsulation([first, second]) => write!(
                formatter,
                "encapsulation {first:02x} {second:02x} is not little-endian plain CDR (00 01)"
            ),
            Self::Truncated { offset, needed } => {
                write!(formatter, "the message ends at {offset}, before a field of {needed} bytes")
            }
            Self::Trailing(left) => {
                write!(formatter, "{left} bytes follow the last field, more than its padding")
            }
            Self::NegativeTime(second) => {
                write!(formatter, "a time of second {second}: a ROS 1 time cannot be negative")
            }
            Self::DurationNanoseconds(nanos) => {
                write!(formatter, "a duration of {nanos} nanoseconds does not fit int32")
            }
            Self::MissingType(name) => write!(formatter, "no definition of {name}"),
            Self::TooLarge => write!(formatter, "a count or a size does not fit this machine"),
        }
    }
}

impl std::error::Error for CdrError {}

/// The ROS 1 encoding of `cdr`, a CDR message of type `root`, whose definition and those
/// of the types it names are in `specs`. Every `std_msgs/Header` in it gets `header_seq`
/// as its `seq`.
///
/// # Errors
/// A [`CdrError`] that says what in the message could not be read.
pub fn transcode(
    specs: &SpecSet,
    root: &TypeName,
    cdr: &[u8],
    header_seq: u32,
) -> Result<Vec<u8>, CdrError> {
    let encapsulation = cdr.get(..2).ok_or(CdrError::Truncated { offset: 0, needed: 2 })?;
    if encapsulation != CDR_LITTLE_ENDIAN {
        let mut found = [0; 2];
        found.copy_from_slice(encapsulation);
        return Err(CdrError::Encapsulation(found));
    }
    if cdr.len() < ENCAPSULATION {
        return Err(CdrError::Truncated { offset: 0, needed: ENCAPSULATION });
    }
    let mut reader = Reader { data: cdr, position: ENCAPSULATION };
    // A ROS 1 message is never larger than its CDR one, but strings shrink by one byte.
    let mut out = Vec::with_capacity(cdr.len());
    let transcoder = Transcoder { specs, header_seq };
    transcoder.message(&mut reader, &mut out, root)?;
    let left = cdr.len().saturating_sub(reader.position);
    if left > MAX_TRAILING_PADDING {
        return Err(CdrError::Trailing(left));
    }
    Ok(out)
}

struct Transcoder<'s> {
    specs: &'s SpecSet,
    header_seq: u32,
}

/// A read cursor over one CDR message; `position` counts from the message's first byte.
struct Reader<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    /// Skip to the next multiple of `size` past the encapsulation header.
    fn align(&mut self, size: usize) -> Result<(), CdrError> {
        let offset = self.position.checked_sub(ENCAPSULATION).ok_or(CdrError::TooLarge)?;
        let remainder = offset.checked_rem(size).ok_or(CdrError::TooLarge)?;
        if remainder != 0 {
            let padding = size.checked_sub(remainder).ok_or(CdrError::TooLarge)?;
            self.position = self.position.checked_add(padding).ok_or(CdrError::TooLarge)?;
        }
        Ok(())
    }

    /// The next `count` bytes.
    fn take(&mut self, count: usize) -> Result<&'a [u8], CdrError> {
        let end = self.position.checked_add(count).ok_or(CdrError::TooLarge)?;
        let bytes = self
            .data
            .get(self.position..end)
            .ok_or(CdrError::Truncated { offset: self.position, needed: count })?;
        self.position = end;
        Ok(bytes)
    }

    /// The next primitive of `size` bytes, aligned to its size.
    fn primitive(&mut self, size: usize) -> Result<&'a [u8], CdrError> {
        self.align(size)?;
        self.take(size)
    }

    fn u32(&mut self) -> Result<u32, CdrError> {
        let bytes = self.primitive(4)?;
        let mut word = [0; 4];
        word.copy_from_slice(bytes);
        Ok(u32::from_le_bytes(word))
    }

    fn i32(&mut self) -> Result<i32, CdrError> {
        let bytes = self.primitive(4)?;
        let mut word = [0; 4];
        word.copy_from_slice(bytes);
        Ok(i32::from_le_bytes(word))
    }

    /// A sequence or string length.
    fn length(&mut self) -> Result<usize, CdrError> {
        usize::try_from(self.u32()?).map_err(|_| CdrError::TooLarge)
    }
}

/// The size in bytes of a primitive, `None` for a string, a time, a duration or a message.
const fn primitive_size(field_type: &FieldType) -> Option<usize> {
    match field_type {
        FieldType::Bool
        | FieldType::Byte
        | FieldType::Char
        | FieldType::Int8
        | FieldType::UInt8 => Some(1),
        FieldType::Int16 | FieldType::UInt16 => Some(2),
        FieldType::Int32 | FieldType::UInt32 | FieldType::Float32 => Some(4),
        FieldType::Int64 | FieldType::UInt64 | FieldType::Float64 => Some(8),
        FieldType::Text | FieldType::Time | FieldType::Duration | FieldType::Message(_) => None,
    }
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

impl Transcoder<'_> {
    fn message(
        &self,
        reader: &mut Reader<'_>,
        out: &mut Vec<u8>,
        name: &TypeName,
    ) -> Result<(), CdrError> {
        let spec = self.specs.get(name).ok_or_else(|| CdrError::MissingType(name.full()))?;
        let is_header = name.package == "std_msgs" && name.name == "Header";
        for field in &spec.fields {
            if is_header && field.name == "seq" && field.field_type == FieldType::UInt32 {
                // ROS 2's header has no sequence number.
                push_u32(out, self.header_seq);
                continue;
            }
            self.field(reader, out, field)?;
        }
        Ok(())
    }

    fn field(
        &self,
        reader: &mut Reader<'_>,
        out: &mut Vec<u8>,
        field: &Field,
    ) -> Result<(), CdrError> {
        let count = match field.multiplicity {
            Multiplicity::Unit => return self.value(reader, out, &field.field_type),
            Multiplicity::Fixed(count) => count,
            Multiplicity::Variable => {
                let count = reader.length()?;
                push_u32(out, u32::try_from(count).map_err(|_| CdrError::TooLarge)?);
                count
            }
        };
        if primitive_size(&field.field_type) == Some(1) {
            // Bytes need no alignment: copy them whole, a point cloud's data included.
            out.extend_from_slice(reader.take(count)?);
            return Ok(());
        }
        for _ in 0..count {
            self.value(reader, out, &field.field_type)?;
        }
        Ok(())
    }

    fn value(
        &self,
        reader: &mut Reader<'_>,
        out: &mut Vec<u8>,
        field_type: &FieldType,
    ) -> Result<(), CdrError> {
        match field_type {
            FieldType::Text => {
                let length = reader.length()?;
                let bytes = reader.take(length)?;
                // The length counts the terminating NUL; an empty CDR string may be 0.
                let text = bytes.strip_suffix(&[0]).unwrap_or(bytes);
                push_u32(out, u32::try_from(text.len()).map_err(|_| CdrError::TooLarge)?);
                out.extend_from_slice(text);
            }
            FieldType::Time => {
                let second = reader.i32()?;
                let nanosecond = reader.u32()?;
                let second = u32::try_from(second).map_err(|_| CdrError::NegativeTime(second))?;
                push_u32(out, second);
                push_u32(out, nanosecond);
            }
            FieldType::Duration => {
                let second = reader.i32()?;
                let nanosecond = reader.u32()?;
                let nanosecond = i32::try_from(nanosecond)
                    .map_err(|_| CdrError::DurationNanoseconds(nanosecond))?;
                out.extend_from_slice(&second.to_le_bytes());
                out.extend_from_slice(&nanosecond.to_le_bytes());
            }
            FieldType::Message(name) => self.message(reader, out, name)?,
            primitive => {
                // Both encodings are little-endian: only the alignment differs.
                let size = primitive_size(primitive).ok_or(CdrError::TooLarge)?;
                out.extend_from_slice(reader.primitive(size)?);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    reason = "a test whose setup fails is a failed test; its sizes are small and fixed"
)]
mod tests {
    use hypha_node::sdk::message::EncodeInto;
    use hypha_node::sdk::msgdef::resolve::SpecSet;
    use hypha_node::sdk::msgdef::spec::TypeName;
    use hypha_node::sdk::msgs::geometry_msgs::{Quaternion, Transform, TransformStamped, Vector3};
    use hypha_node::sdk::msgs::sensor_msgs::{Imu, PointCloud2, PointField};
    use hypha_node::sdk::msgs::std_msgs::{Duration, Header, String as Text};
    use hypha_node::sdk::msgs::tf2_msgs::TFMessage;
    use hypha_node::sdk::{RosDuration, RosTime};

    use super::{CdrError, transcode};
    use crate::msg_library::MessageLibrary;

    /// A CDR writer for the tests: little-endian plain CDR, each primitive aligned to its
    /// size from the end of the encapsulation header.
    struct Cdr(Vec<u8>);

    impl Cdr {
        fn new() -> Self {
            Self(vec![0x00, 0x01, 0x00, 0x00])
        }
        fn align(&mut self, size: usize) {
            while !(self.0.len() - 4).is_multiple_of(size) {
                self.0.push(0);
            }
        }
        fn u8(&mut self, value: u8) -> &mut Self {
            self.0.push(value);
            self
        }
        fn u32(&mut self, value: u32) -> &mut Self {
            self.align(4);
            self.0.extend_from_slice(&value.to_le_bytes());
            self
        }
        fn i32(&mut self, value: i32) -> &mut Self {
            self.align(4);
            self.0.extend_from_slice(&value.to_le_bytes());
            self
        }
        fn f64(&mut self, value: f64) -> &mut Self {
            self.align(8);
            self.0.extend_from_slice(&value.to_le_bytes());
            self
        }
        fn text(&mut self, value: &str) -> &mut Self {
            self.u32(u32::try_from(value.len() + 1).expect("a short string"));
            self.0.extend_from_slice(value.as_bytes());
            self.0.push(0);
            self
        }
        /// A ROS 2 header: stamp and frame, no sequence number.
        fn header(&mut self, sec: i32, nanosec: u32, frame: &str) -> &mut Self {
            self.i32(sec).u32(nanosec).text(frame)
        }
        fn vector(&mut self, x: f64, y: f64, z: f64) -> &mut Self {
            self.f64(x).f64(y).f64(z)
        }
    }

    fn specs(types: &[(&str, &str)]) -> SpecSet {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/hypha_msgs/msg");
        let mut library = MessageLibrary::new(vec![path]);
        for (package, name) in types {
            library.resolve(&TypeName::new(package, name)).expect("a hypha message");
        }
        library.specs().clone()
    }

    fn ros1<M: EncodeInto>(message: &M) -> Vec<u8> {
        let mut bytes = vec![0; message.encoded_len()];
        let written = message.encode_into(&mut bytes).expect("encodes");
        bytes.truncate(written);
        bytes
    }

    fn header(seq: u32, secs: u32, nsecs: u32, frame: &str) -> Header {
        Header { seq, stamp: RosTime { secs, nsecs }, frame_id: frame.to_owned() }
    }

    #[test]
    fn an_imu_gets_its_header_sequence_and_loses_its_alignment() {
        let specs = specs(&[("sensor_msgs", "Imu")]);
        let covariance = [0.5, 0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.5];
        let mut cdr = Cdr::new();
        // "base_link" leaves the cursor off 8: the quaternion's first double is padded.
        cdr.header(1_656_075_302, 311_699_773, "base_link").f64(0.1).f64(0.2).f64(0.3).f64(0.9);
        for value in covariance {
            cdr.f64(value);
        }
        cdr.vector(1.0, -2.0, 3.0);
        for value in covariance {
            cdr.f64(value);
        }
        cdr.vector(0.0, 0.0, 9.81);
        for value in covariance {
            cdr.f64(value);
        }
        let expected = Imu {
            header: header(7, 1_656_075_302, 311_699_773, "base_link"),
            orientation: Quaternion { x: 0.1, y: 0.2, z: 0.3, w: 0.9 },
            orientation_covariance: covariance,
            angular_velocity: Vector3 { x: 1.0, y: -2.0, z: 3.0 },
            angular_velocity_covariance: covariance,
            linear_acceleration: Vector3 { x: 0.0, y: 0.0, z: 9.81 },
            linear_acceleration_covariance: covariance,
        };
        let root = TypeName::new("sensor_msgs", "Imu");
        assert_eq!(transcode(&specs, &root, &cdr.0, 7), Ok(ros1(&expected)));
    }

    #[test]
    fn a_point_cloud_keeps_its_fields_and_copies_its_data_whole() {
        let specs = specs(&[("sensor_msgs", "PointCloud2")]);
        let data: Vec<u8> = (0..32).collect();
        let mut cdr = Cdr::new();
        cdr.header(10, 20, "lidar").u32(1).u32(2).u32(2);
        cdr.text("x").u32(0).u8(PointField::FLOAT32).u32(1);
        cdr.text("intensity").u32(12).u8(PointField::UINT8).u32(1);
        cdr.u8(0).u32(16).u32(32).u32(32);
        cdr.0.extend_from_slice(&data);
        cdr.u8(1);
        let field = |name: &str, offset, datatype| PointField {
            name: name.to_owned(),
            offset,
            datatype,
            count: 1,
        };
        let expected = PointCloud2 {
            header: header(0, 10, 20, "lidar"),
            height: 1,
            width: 2,
            fields: vec![
                field("x", 0, PointField::FLOAT32),
                field("intensity", 12, PointField::UINT8),
            ],
            is_bigendian: false,
            point_step: 16,
            row_step: 32,
            data,
            is_dense: true,
        };
        let root = TypeName::new("sensor_msgs", "PointCloud2");
        assert_eq!(transcode(&specs, &root, &cdr.0, 0), Ok(ros1(&expected)));
    }

    #[test]
    fn every_nested_header_gets_the_sequence() {
        let specs = specs(&[("tf2_msgs", "TFMessage")]);
        let mut cdr = Cdr::new();
        cdr.u32(2);
        for child in ["lidar", "camera"] {
            cdr.header(5, 6, "base_link").text(child);
            cdr.vector(1.0, 2.0, 3.0).f64(0.0).f64(0.0).f64(0.0).f64(1.0);
        }
        let transform = |child: &str| TransformStamped {
            header: header(3, 5, 6, "base_link"),
            child_frame_id: child.to_owned(),
            transform: Transform {
                translation: Vector3 { x: 1.0, y: 2.0, z: 3.0 },
                rotation: Quaternion { x: 0.0, y: 0.0, z: 0.0, w: 1.0 },
            },
        };
        let expected = TFMessage { transforms: vec![transform("lidar"), transform("camera")] };
        let root = TypeName::new("tf2_msgs", "TFMessage");
        assert_eq!(transcode(&specs, &root, &cdr.0, 3), Ok(ros1(&expected)));
    }

    #[test]
    fn a_string_loses_its_terminator_and_an_empty_one_may_have_none() {
        let specs = specs(&[("std_msgs", "String")]);
        let root = TypeName::new("std_msgs", "String");
        let mut cdr = Cdr::new();
        cdr.text("hello");
        let expected = ros1(&Text { data: "hello".to_owned() });
        assert_eq!(transcode(&specs, &root, &cdr.0, 0), Ok(expected));
        let mut empty = Cdr::new();
        empty.u32(0);
        assert_eq!(transcode(&specs, &root, &empty.0, 0), Ok(ros1(&Text { data: String::new() })));
    }

    #[test]
    fn a_duration_keeps_its_sign() {
        let specs = specs(&[("std_msgs", "Duration")]);
        let root = TypeName::new("std_msgs", "Duration");
        let mut cdr = Cdr::new();
        cdr.i32(-3).u32(500);
        let expected = ros1(&Duration { data: RosDuration { secs: -3, nsecs: 500 } });
        assert_eq!(transcode(&specs, &root, &cdr.0, 0), Ok(expected));
        let mut huge = Cdr::new();
        huge.i32(1).u32(u32::MAX);
        assert_eq!(
            transcode(&specs, &root, &huge.0, 0),
            Err(CdrError::DurationNanoseconds(u32::MAX))
        );
    }

    #[test]
    fn what_cannot_be_read_is_named() {
        let specs = specs(&[("std_msgs", "Header")]);
        let root = TypeName::new("std_msgs", "Header");
        let mut cdr = Cdr::new();
        cdr.header(1, 2, "frame");
        let big_endian = [&[0x00, 0x00, 0x00, 0x00][..], cdr.0.get(4..).expect("a body")].concat();
        assert_eq!(transcode(&specs, &root, &big_endian, 0), Err(CdrError::Encapsulation([0, 0])));
        let short = cdr.0.get(..cdr.0.len() - 2).expect("a prefix");
        assert!(matches!(transcode(&specs, &root, short, 0), Err(CdrError::Truncated { .. })));
        let mut negative = Cdr::new();
        negative.header(-1, 0, "frame");
        assert_eq!(transcode(&specs, &root, &negative.0, 0), Err(CdrError::NegativeTime(-1)));
        let mut trailing = cdr.0.clone();
        trailing.extend_from_slice(&[0; 12]);
        assert_eq!(transcode(&specs, &root, &trailing, 0), Err(CdrError::Trailing(12)));
        let unknown = TypeName::new("vendor_msgs", "Missing");
        assert_eq!(
            transcode(&specs, &unknown, &cdr.0, 0),
            Err(CdrError::MissingType("vendor_msgs/Missing".to_owned()))
        );
    }
}
