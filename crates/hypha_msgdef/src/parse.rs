//! Turns message definition text into a `MessageSpec`.
//!
//! One line is one field (`type name`) or constant (`type NAME=value`). A `#` starts a
//! comment to the end of the line, except inside the value of a `string` constant.

use crate::constant::{ConstantValueError, constant_value};
use crate::spec::{Constant, Field, FieldType, MessageSpec, Multiplicity, ServiceSpec, TypeName};

/// Why a ROS 1 service definition could not be split or parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceParseError {
    /// A service definition has exactly one line containing `---`.
    InvalidSeparatorCount {
        /// Separators found in the supplied text.
        count: usize,
    },
    /// The request definition is invalid.
    Request(ParseError),
    /// The response definition is invalid.
    Response(ParseError),
}

impl std::fmt::Display for ServiceParseError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSeparatorCount { count } => {
                write!(out, "service definition needs exactly one `---` separator, found {count}")
            }
            Self::Request(error) => write!(out, "invalid request: {error}"),
            Self::Response(error) => write!(out, "invalid response: {error}"),
        }
    }
}

impl std::error::Error for ServiceParseError {}

/// Parse a ROS 1 `.srv` definition into request and response message specs.
///
/// # Errors
/// Returns an error unless the text has exactly one separator and both halves
/// are valid message definitions.
pub fn parse_service(type_name: TypeName, text: &str) -> Result<ServiceSpec, ServiceParseError> {
    let mut request_lines = Vec::new();
    let mut response_lines = Vec::new();
    let mut separator_count = 0_usize;
    for line in text.lines() {
        if line.trim() == "---" {
            separator_count = separator_count.saturating_add(1);
            continue;
        }
        if separator_count == 0 {
            request_lines.push(line);
        } else {
            response_lines.push(line);
        }
    }
    if separator_count != 1 {
        return Err(ServiceParseError::InvalidSeparatorCount { count: separator_count });
    }
    let request_name = TypeName::new(&type_name.package, &format!("{}Request", type_name.name));
    let response_name = TypeName::new(&type_name.package, &format!("{}Response", type_name.name));
    let request = parse_message(request_name, &request_lines.join("\n"))
        .map_err(ServiceParseError::Request)?;
    let response = parse_message(response_name, &response_lines.join("\n"))
        .map_err(ServiceParseError::Response)?;
    Ok(ServiceSpec { type_name, request, response })
}

/// Why one line of a message definition could not be understood.
///
/// Every variant carries the 1-based line number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// The line names a type but gives no field or constant name.
    MissingName {
        /// The line number, counted from 1.
        line: usize,
        /// The line's text, with surrounding whitespace removed.
        text: String,
    },
    /// The type is empty, such as an array with no base type before `[`.
    InvalidType {
        /// The line number, counted from 1.
        line: usize,
        /// The type text that could not be understood.
        text: String,
    },
    /// The array size between `[` and `]` is not a plain whole number, or the
    /// closing `]` is missing.
    InvalidArraySize {
        /// The line number, counted from 1.
        line: usize,
        /// The type text that carries the bad array size.
        text: String,
    },
    /// The field or constant name, after the type, is empty.
    EmptyName {
        /// The line number, counted from 1.
        line: usize,
    },
    /// A nested type name splits into an empty package or an empty name.
    InvalidTypeName {
        /// The line number, counted from 1.
        line: usize,
        /// The type text that could not be split into package and name.
        text: String,
    },
    /// A constant names a type that is not built in.
    NonBuiltinConstant {
        /// The line number, counted from 1.
        line: usize,
        /// The full name of the type the constant named.
        text: String,
    },
    /// A constant's value is not a literal of its type.
    InvalidConstantValue {
        /// The line number, counted from 1.
        line: usize,
        /// The value as written.
        text: String,
        /// The constant's type, as written in definition text.
        constant_type: String,
    },
    /// A constant's value lies outside the range of its type.
    ConstantOutOfRange {
        /// The line number, counted from 1.
        line: usize,
        /// The value as written.
        text: String,
        /// The constant's type, as written in definition text.
        constant_type: String,
        /// The smallest value of the type.
        min: String,
        /// The largest value of the type.
        max: String,
    },
    /// A constant is declared with array brackets.
    ArrayConstant {
        /// The line number, counted from 1.
        line: usize,
        /// The type text, with its brackets.
        text: String,
    },
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingName { line, text } => {
                write!(out, "line {line}: type with no field or constant name: \"{text}\"")
            }
            Self::InvalidType { line, text } => {
                write!(out, "line {line}: empty or malformed type: \"{text}\"")
            }
            Self::InvalidArraySize { line, text } => {
                write!(out, "line {line}: bad array size in \"{text}\"")
            }
            Self::EmptyName { line } => {
                write!(out, "line {line}: empty field or constant name")
            }
            Self::InvalidTypeName { line, text } => {
                write!(
                    out,
                    "line {line}: type name has an empty package or an empty name: \"{text}\""
                )
            }
            Self::NonBuiltinConstant { line, text } => {
                write!(out, "line {line}: constant type is not built in: \"{text}\"")
            }
            Self::InvalidConstantValue { line, text, constant_type } => {
                write!(
                    out,
                    "line {line}: constant value \"{text}\" is not a {constant_type} literal"
                )
            }
            Self::ConstantOutOfRange { line, text, constant_type, min, max } => {
                write!(
                    out,
                    "line {line}: constant value {text} is out of range for {constant_type} ({min} to {max})"
                )
            }
            Self::ArrayConstant { line, text } => {
                write!(out, "line {line}: a constant cannot be an array: \"{text}\"")
            }
        }
    }
}

impl std::error::Error for ParseError {}

/// One parsed line: either a field or a constant.
enum Declaration {
    /// A field declaration, `type name`.
    Field(Field),
    /// A constant declaration, `type NAME=value`.
    Constant(Constant),
}

/// Builds a `MessageSpec` from one message definition's text.
/// `type_name` names the message being parsed; an unqualified nested type takes its
/// package.
///
/// # Errors
///
/// Returns a [`ParseError`] naming the first line that does not follow the syntax.
pub fn parse_message(type_name: TypeName, text: &str) -> Result<MessageSpec, ParseError> {
    let mut fields = Vec::new();
    let mut constants = Vec::new();

    for (index, raw_line) in text.lines().enumerate() {
        let line_number = index.saturating_add(1);
        let trimmed = raw_line.trim();
        let skip_line = trimmed.is_empty() || trimmed.starts_with('#');
        if skip_line {
            continue;
        }

        match parse_declaration(line_number, trimmed, &type_name.package)? {
            Declaration::Field(field) => fields.push(field),
            Declaration::Constant(constant) => constants.push(constant),
        }
    }

    Ok(MessageSpec { type_name, constants, fields })
}

/// Parses one already-trimmed, non-comment line into a field or a constant.
fn parse_declaration(
    line: usize,
    trimmed: &str,
    own_package: &str,
) -> Result<Declaration, ParseError> {
    let missing_name = || ParseError::MissingName { line, text: String::from(trimmed) };
    let (type_token, rest) = trimmed.split_once(char::is_whitespace).ok_or_else(missing_name)?;
    let after_type = rest.trim_start();
    if after_type.is_empty() {
        return Err(missing_name());
    }

    let (base_type, multiplicity) = split_array_suffix(line, type_token)?;
    let field_type = resolve_type(line, base_type, own_package)?;

    // An `=` after a `#` is part of the comment.
    let comment_at = after_type.find('#');
    let equals_index = after_type.find('=').filter(|&at| comment_at.is_none_or(|end| at < end));

    if let Some(equals_index) = equals_index {
        if multiplicity != Multiplicity::Unit {
            return Err(ParseError::ArrayConstant { line, text: String::from(type_token) });
        }
        return build_constant(line, after_type, equals_index, field_type);
    }

    let name = strip_comment(after_type).trim().to_string();
    if name.is_empty() {
        return Err(ParseError::EmptyName { line });
    }
    Ok(Declaration::Field(Field { field_type, multiplicity, name }))
}

/// Builds a constant from the text after its type, given where `=` sits.
fn build_constant(
    line: usize,
    after_type: &str,
    equals_index: usize,
    field_type: FieldType,
) -> Result<Declaration, ParseError> {
    let is_time = matches!(field_type, FieldType::Time | FieldType::Duration);
    let can_be_constant = field_type.is_builtin() && !is_time;
    if !can_be_constant {
        return Err(ParseError::NonBuiltinConstant { line, text: field_type.render() });
    }

    let name = after_type.get(..equals_index).unwrap_or("").trim().to_string();
    if name.is_empty() {
        return Err(ParseError::EmptyName { line });
    }

    let value_start = equals_index.saturating_add(1);
    let raw_value = after_type.get(value_start..).unwrap_or("");
    let is_string_constant = matches!(field_type, FieldType::Text);
    let value = if is_string_constant {
        raw_value.trim().to_string()
    } else {
        strip_comment(raw_value).trim().to_string()
    };

    let constant = Constant { constant_type: field_type, name, value };
    match constant_value(&constant) {
        Ok(_) => Ok(Declaration::Constant(constant)),
        Err(ConstantValueError::NotALiteral) => Err(ParseError::InvalidConstantValue {
            line,
            text: constant.value,
            constant_type: constant.constant_type.render(),
        }),
        Err(ConstantValueError::OutOfRange { min, max }) => Err(ParseError::ConstantOutOfRange {
            line,
            text: constant.value,
            constant_type: constant.constant_type.render(),
            min,
            max,
        }),
        Err(ConstantValueError::NotAConstantType) => {
            Err(ParseError::NonBuiltinConstant { line, text: constant.constant_type.render() })
        }
    }
}

/// Cuts `text` at its first comment marker, if it has one.
fn strip_comment(text: &str) -> &str {
    text.split_once('#').map_or(text, |(before, _)| before)
}

/// Splits a type token into its base name and its array suffix.
/// A `[` with no matching `]`, or a size that is not a plain whole number, is an error.
fn split_array_suffix(line: usize, type_token: &str) -> Result<(&str, Multiplicity), ParseError> {
    let Some(bracket_at) = type_token.find('[') else {
        if type_token.is_empty() {
            return Err(ParseError::InvalidType { line, text: String::from(type_token) });
        }
        return Ok((type_token, Multiplicity::Unit));
    };

    if !type_token.ends_with(']') {
        return Err(ParseError::InvalidArraySize { line, text: String::from(type_token) });
    }

    let base_type = type_token.get(..bracket_at).unwrap_or("");
    if base_type.is_empty() {
        return Err(ParseError::InvalidType { line, text: String::from(type_token) });
    }

    let inside_start = bracket_at.saturating_add(1);
    let inside_end = type_token.len().saturating_sub(1);
    let inside = type_token.get(inside_start..inside_end).unwrap_or("");
    if inside.is_empty() {
        return Ok((base_type, Multiplicity::Variable));
    }

    inside
        .parse::<usize>()
        .map(|count| (base_type, Multiplicity::Fixed(count)))
        .map_err(|_| ParseError::InvalidArraySize { line, text: String::from(type_token) })
}

/// Resolves a base type name (with no array suffix) to a `FieldType`.
/// `Header` becomes `std_msgs/Header`; a name without `/` takes `own_package`.
fn resolve_type(line: usize, base_type: &str, own_package: &str) -> Result<FieldType, ParseError> {
    let field_type = match base_type {
        "bool" => FieldType::Bool,
        "byte" => FieldType::Byte,
        "char" => FieldType::Char,
        "int8" => FieldType::Int8,
        "uint8" => FieldType::UInt8,
        "int16" => FieldType::Int16,
        "uint16" => FieldType::UInt16,
        "int32" => FieldType::Int32,
        "uint32" => FieldType::UInt32,
        "int64" => FieldType::Int64,
        "uint64" => FieldType::UInt64,
        "float32" => FieldType::Float32,
        "float64" => FieldType::Float64,
        "string" => FieldType::Text,
        "time" => FieldType::Time,
        "duration" => FieldType::Duration,
        "Header" => FieldType::Message(TypeName::new("std_msgs", "Header")),
        _ => match base_type.split_once('/') {
            Some((package, name)) if package.is_empty() || name.is_empty() => {
                return Err(ParseError::InvalidTypeName { line, text: String::from(base_type) });
            }
            Some((package, name)) => FieldType::Message(TypeName::new(package, name)),
            None => FieldType::Message(TypeName::new(own_package, base_type)),
        },
    };
    Ok(field_type)
}
