//! The encoded size a definition gives its messages.
//!
//! A message with no `string`, no `T[]` and only fixed-size nested messages encodes to
//! the same number of bytes every time, as ROS 1 `gencpp` computes `IsFixedSize`.

use std::collections::HashMap;

use crate::definition::parse_definition;
use crate::resolve::{ROOT_LEVEL, ResolveError, SpecSet, message_identity};
use crate::spec::{FieldType, MAX_NESTING_DEPTH, MessageSpec, Multiplicity, TypeName};

/// What a verified definition says about the size of its messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodedSize {
    /// Every message is exactly this many bytes.
    Fixed(usize),
    /// The size depends on the message.
    Variable,
}

/// Why a definition cannot be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SizeError {
    /// `type_name` is not `package/Name`, or the text does not fully describe it.
    Unreadable,
    /// The definition hashes to `computed`, not to the MD5 it came with.
    Md5Mismatch {
        /// The MD5 the definition text gives.
        computed: String,
    },
    /// The definition nests messages deeper than the shared limit.
    NestingTooDeep {
        /// The deepest nesting allowed, [`MAX_NESTING_DEPTH`](crate::spec::MAX_NESTING_DEPTH).
        limit: usize,
    },
    /// The definition expands past
    /// [`MAX_EXPANDED_MESSAGES`](crate::spec::MAX_EXPANDED_MESSAGES), or a fixed-size
    /// message encodes to more bytes than `usize` holds.
    TooLarge,
}

/// The encoded size of `type_name` messages, after checking that
/// `definition_text` hashes to `md5`.
///
/// # Errors
/// [`SizeError::Unreadable`] when the text does not describe the type,
/// [`SizeError::Md5Mismatch`] when it describes another one,
/// [`SizeError::NestingTooDeep`] when it nests past
/// [`MAX_NESTING_DEPTH`](crate::spec::MAX_NESTING_DEPTH), and [`SizeError::TooLarge`]
/// when it expands too far or a fixed size overflows `usize`.
pub fn verified_encoded_size(
    type_name: &str,
    md5: &str,
    definition_text: &str,
) -> Result<EncodedSize, SizeError> {
    let root = TypeName::parse(type_name).ok_or(SizeError::Unreadable)?;
    let specs = parse_definition(&root, definition_text).map_err(|_| SizeError::Unreadable)?;
    let computed = match message_identity(&specs, &root) {
        Ok(computed) => computed,
        Err(ResolveError::NestingTooDeep { limit, .. }) => {
            return Err(SizeError::NestingTooDeep { limit });
        }
        Err(ResolveError::MissingType { .. } | ResolveError::Cycle { .. }) => {
            return Err(SizeError::Unreadable);
        }
        Err(ResolveError::TooLarge { .. }) => return Err(SizeError::TooLarge),
    };
    if !computed.eq_ignore_ascii_case(md5) {
        return Err(SizeError::Md5Mismatch { computed });
    }
    let fixed = encoded_size(&specs, &root, ROOT_LEVEL, &mut HashMap::new())?;
    Ok(fixed.map_or(EncodedSize::Variable, EncodedSize::Fixed))
}

/// The fixed encoded size of `root`, or `None` when it varies. Memoised per type in
/// `known`. `level` is the depth of `root`, the root message being level 1.
fn encoded_size(
    specs: &SpecSet,
    root: &TypeName,
    level: usize,
    known: &mut HashMap<TypeName, Option<usize>>,
) -> Result<Option<usize>, SizeError> {
    if let Some(size) = known.get(root) {
        return Ok(*size);
    }
    let nests_too_deep = level > MAX_NESTING_DEPTH;
    if nests_too_deep {
        return Err(SizeError::NestingTooDeep { limit: MAX_NESTING_DEPTH });
    }
    let spec = specs.get(root).ok_or(SizeError::Unreadable)?;
    let total = fields_size(specs, spec, level, known)?;
    known.insert(root.clone(), total);
    Ok(total)
}

/// The fixed encoded size of the fields of `spec`, or `None` at the first field
/// that varies. `level` is the depth of `spec`, as for [`encoded_size`].
fn fields_size(
    specs: &SpecSet,
    spec: &MessageSpec,
    level: usize,
    known: &mut HashMap<TypeName, Option<usize>>,
) -> Result<Option<usize>, SizeError> {
    let mut total = 0_usize;
    for field in &spec.fields {
        let count = match field.multiplicity {
            Multiplicity::Unit => 1,
            Multiplicity::Fixed(count) => count,
            Multiplicity::Variable => return Ok(None),
        };
        let width = match &field.field_type {
            FieldType::Bool
            | FieldType::Byte
            | FieldType::Char
            | FieldType::Int8
            | FieldType::UInt8 => 1,
            FieldType::Int16 | FieldType::UInt16 => 2,
            FieldType::Int32 | FieldType::UInt32 | FieldType::Float32 => 4,
            FieldType::Int64
            | FieldType::UInt64
            | FieldType::Float64
            | FieldType::Time
            | FieldType::Duration => 8,
            FieldType::Text => return Ok(None),
            FieldType::Message(nested) => {
                let nested_level = level.saturating_add(1);
                let Some(nested_width) = encoded_size(specs, nested, nested_level, known)? else {
                    return Ok(None);
                };
                nested_width
            }
        };
        let field_bytes = width.checked_mul(count).ok_or(SizeError::TooLarge)?;
        total = total.checked_add(field_bytes).ok_or(SizeError::TooLarge)?;
    }
    Ok(Some(total))
}
