//! Dynamic ROS 1 wire format decoder driven by `.msg` definition text.

use std::collections::HashMap;
use std::fmt::Write as FmtWrite;
use std::ops::Range;
use std::sync::Arc;

use hypha_node::sdk::codec::{CodecError, Reader};
use hypha_node::sdk::msgdef::definition::parse_definition;
use hypha_node::sdk::msgdef::resolve::SpecSet;
use hypha_node::sdk::msgdef::spec::{Field, MAX_NESTING_DEPTH, Multiplicity, TypeName};

/// Builtin scalar and primitive types in ROS 1.
pub const BUILTIN_TYPES: &[&str] = &[
    "bool", "byte", "char", "int8", "uint8", "int16", "uint16", "int32", "uint32", "int64",
    "uint64", "float32", "float64", "string", "time", "duration",
];

/// Checks if a type string is a built-in primitive.
#[must_use]
pub fn is_builtin_type(t: &str) -> bool {
    BUILTIN_TYPES.contains(&t)
}

/// A reflected field of a message type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReflectedField {
    /// Builtin type or resolved `package/Type`.
    pub type_name: String,
    /// Field name.
    pub name: String,
    /// Whether this field is an array (fixed or dynamic).
    pub is_array: bool,
    /// Whether this is a fixed-length array `type[N]`.
    pub is_fixed_length: bool,
    /// Length of fixed array if `is_fixed_length` is true.
    pub array_length: usize,
}

/// Parsed message schema: every message of one definition, for nested resolution.
#[derive(Debug, Clone)]
pub struct MessageSchema {
    specs: SpecSet,
    cache: HashMap<String, Arc<[ReflectedField]>>,
}

impl MessageSchema {
    /// Builds a `MessageSchema` from concatenated definition text for a root type.
    ///
    /// # Errors
    ///
    /// Returns the reason when `root_type` is not `package/Type`, or when a
    /// message in the text is not a valid definition.
    pub fn from_definition(root_type: &str, definition_text: &str) -> Result<Self, String> {
        let (package, name) = root_type
            .split_once('/')
            .ok_or_else(|| format!("`{root_type}` is not a `package/Type` name"))?;
        let root = TypeName::new(package, name);
        let specs = parse_definition(&root, definition_text).map_err(|error| error.to_string())?;
        // The decoders walk the definition per message: bound that walk once here.
        hypha_node::sdk::msgdef::resolve::check_expansion(&specs, &root)
            .map_err(|error| error.to_string())?;
        Ok(Self { specs, cache: HashMap::new() })
    }

    /// Resolves fields for `type_name`.
    pub fn fields_for(&mut self, type_name: &str) -> Option<Arc<[ReflectedField]>> {
        if let Some(fields) = self.cache.get(type_name) {
            return Some(Arc::clone(fields));
        }
        let (package, name) = type_name.split_once('/')?;
        let spec = self.specs.get(&TypeName::new(package, name))?;
        let fields: Arc<[ReflectedField]> = spec.fields.iter().map(reflected_field).collect();
        self.cache.insert(type_name.to_string(), Arc::clone(&fields));
        Some(fields)
    }
}

/// Parsed schemas by `(topic, data_type, md5)` identity, shared by
/// `hypha topic` and `hypha top`.
pub struct SchemaCache {
    tool: &'static str,
    schemas: HashMap<(String, String, String), Result<MessageSchema, String>>,
}

impl SchemaCache {
    /// An empty cache whose reports name `tool`.
    #[must_use]
    pub fn new(tool: &'static str) -> Self {
        Self { tool, schemas: HashMap::new() }
    }

    /// The schema for one identity. `None` while its definition is not known;
    /// `Some(Err)` with the reason when the definition does not parse.
    pub fn get(
        &mut self,
        bus: &hypha_node::sdk::BusNode,
        topic: &str,
        data_type: &str,
        md5: &str,
    ) -> Option<Result<&mut MessageSchema, &str>> {
        let key = (topic.to_owned(), data_type.to_owned(), md5.to_owned());
        if !self.schemas.contains_key(&key) {
            let definition = match bus.lookup_definition(topic, data_type, md5) {
                Ok(Some(definition)) => definition,
                Ok(None) => return None,
                Err(cause) => {
                    eprintln!("{}: schema lookup failed for '{topic}': {cause}", self.tool);
                    return None;
                }
            };
            if definition.is_empty() {
                return None;
            }
            let schema = MessageSchema::from_definition(data_type, &definition);
            if let Err(cause) = &schema {
                eprintln!(
                    "{}: '{data_type}' on '{topic}' could not be decoded: {cause}",
                    self.tool
                );
            }
            self.schemas.insert(key.clone(), schema);
        }
        Some(self.schemas.get_mut(&key)?.as_mut().map_err(|cause| cause.as_str()))
    }
}

/// The reflected view of one parsed field.
fn reflected_field(field: &Field) -> ReflectedField {
    let (is_fixed_length, array_length) = match field.multiplicity {
        Multiplicity::Fixed(count) => (true, count),
        Multiplicity::Unit | Multiplicity::Variable => (false, 0),
    };
    ReflectedField {
        type_name: field.field_type.render(),
        name: field.name.clone(),
        is_array: field.multiplicity != Multiplicity::Unit,
        is_fixed_length,
        array_length,
    }
}

/// Cap on elements dumped in arrays.
pub const MAX_ARRAY_PRINT_ELEMENTS: usize = 16;

/// Decodes a ROS 1 message payload against `schema` and writes indented output into `out`.
///
/// Returns `false` when the type is unknown or the payload does not match the schema.
pub fn decode_message(
    out: &mut String,
    type_name: &str,
    schema: &mut MessageSchema,
    bytes: &[u8],
) -> bool {
    if schema.fields_for(type_name).is_none() {
        let _ = writeln!(out, "<unresolved type {type_name}>");
        return false;
    }
    let walked = {
        let mut printer = Printer { out: &mut *out, bytes, stack: Vec::new() };
        walk(schema, type_name, bytes, &mut |event| printer.on(event))
    };
    match walked {
        Ok(_) => true,
        Err(cause) => {
            let _ = writeln!(out, "<payload does not match the schema: {cause}>");
            false
        }
    }
}

/// One step of [`walk`] over a payload, in wire order.
///
/// Every field is bracketed by `Enter` and `Leave`. An array is bracketed by
/// `ArrayStart` and `ArrayEnd`, and each element starts with `Element`; a byte
/// array (`uint8[]`, `byte[]`, `char[]`) is one `Bytes` instead of elements. A
/// `span` is the full wire encoding: for a `string`, the 4-byte length and the
/// bytes. `prefix` is the 4-byte length of a variable array, empty for a fixed one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WalkEvent<'a> {
    /// A field starts.
    Enter {
        /// The field, with its type and multiplicity.
        field: &'a ReflectedField,
    },
    /// Element `index` of the innermost array starts.
    Element {
        /// Zero-based element index.
        index: usize,
    },
    /// The innermost field ends.
    Leave,
    /// A builtin value.
    Scalar {
        /// The builtin type.
        type_name: &'a str,
        /// Its bytes in the payload.
        span: Range<usize>,
    },
    /// The content of a byte array.
    Bytes {
        /// Byte count.
        count: usize,
        /// The bytes in the payload, without the length prefix.
        span: Range<usize>,
    },
    /// An array of `count` elements starts.
    ArrayStart {
        /// Element count.
        count: usize,
        /// True for `type[N]`.
        fixed: bool,
        /// The length prefix in the payload; empty for a fixed array.
        prefix: Range<usize>,
    },
    /// The innermost array ends.
    ArrayEnd,
    /// A message type the schema does not define. Nothing of it was read, so the
    /// offsets after it are not trustworthy.
    Unresolved {
        /// The undefined type.
        type_name: &'a str,
    },
}

/// Walks `bytes` as a `root` message and reports each step to `visit`.
///
/// Returns the number of bytes the message occupies.
///
/// # Errors
/// Returns the codec error at the first read the payload cannot satisfy. The
pub fn walk(
    schema: &mut MessageSchema,
    root: &str,
    bytes: &[u8],
    visit: &mut dyn FnMut(WalkEvent<'_>),
) -> Result<usize, CodecError> {
    let mut reader = Reader::new(bytes);
    walk_message(root, &mut reader, schema, visit, 0)?;
    Ok(reader.position())
}

/// `depth` counts the messages that enclose this one.
fn walk_message(
    type_name: &str,
    reader: &mut Reader<'_>,
    schema: &mut MessageSchema,
    visit: &mut dyn FnMut(WalkEvent<'_>),
    depth: usize,
) -> Result<(), CodecError> {
    if depth >= MAX_NESTING_DEPTH {
        return Err(CodecError::NestingTooDeep { limit: MAX_NESTING_DEPTH });
    }
    let Some(fields) = schema.fields_for(type_name) else {
        visit(WalkEvent::Unresolved { type_name });
        return Ok(());
    };
    let inner = depth.saturating_add(1);
    for field in fields.iter() {
        walk_field(field, reader, schema, visit, inner)?;
    }
    Ok(())
}

fn walk_field(
    field: &ReflectedField,
    reader: &mut Reader<'_>,
    schema: &mut MessageSchema,
    visit: &mut dyn FnMut(WalkEvent<'_>),
    depth: usize,
) -> Result<(), CodecError> {
    visit(WalkEvent::Enter { field });
    let builtin = is_builtin_type(&field.type_name);
    if !field.is_array {
        if builtin {
            let span = scalar_span(&field.type_name, reader)?;
            visit(WalkEvent::Scalar { type_name: &field.type_name, span });
        } else {
            walk_message(&field.type_name, reader, schema, visit, depth)?;
        }
        visit(WalkEvent::Leave);
        return Ok(());
    }

    let start = reader.position();
    let count = if field.is_fixed_length {
        field.array_length
    } else {
        reader.read_array_len_bounded(1)?
    };
    let prefix = start..reader.position();
    visit(WalkEvent::ArrayStart { count, fixed: field.is_fixed_length, prefix });
    if is_byte_array(field) {
        let start = reader.position();
        reader.read_bytes(count)?;
        visit(WalkEvent::Bytes { count, span: start..reader.position() });
    } else if builtin {
        for index in 0..count {
            visit(WalkEvent::Element { index });
            let span = scalar_span(&field.type_name, reader)?;
            visit(WalkEvent::Scalar { type_name: &field.type_name, span });
        }
    } else if schema.fields_for(&field.type_name).is_none() {
        visit(WalkEvent::Unresolved { type_name: &field.type_name });
    } else {
        for index in 0..count {
            visit(WalkEvent::Element { index });
            walk_message(&field.type_name, reader, schema, visit, depth)?;
        }
    }
    visit(WalkEvent::ArrayEnd);
    visit(WalkEvent::Leave);
    Ok(())
}

/// True for `uint8[]`, `byte[]` and `char[]`, which are shown as a size.
#[must_use]
pub fn is_byte_array(field: &ReflectedField) -> bool {
    field.is_array && matches!(field.type_name.as_str(), "uint8" | "byte" | "char")
}

/// Reads one builtin value and returns its bytes in the payload.
fn scalar_span(type_name: &str, reader: &mut Reader<'_>) -> Result<Range<usize>, CodecError> {
    let start = reader.position();
    match type_name {
        "bool" | "byte" | "int8" | "char" | "uint8" => {
            reader.read_u8()?;
        }
        "int16" | "uint16" => {
            reader.read_u16()?;
        }
        "int32" | "uint32" | "float32" => {
            reader.read_u32()?;
        }
        "int64" | "uint64" | "float64" => {
            reader.read_u64()?;
        }
        "string" => {
            reader.read_string()?;
        }
        "time" => {
            reader.read_time()?;
        }
        "duration" => {
            reader.read_duration()?;
        }
        _ => {}
    }
    Ok(start..reader.position())
}

/// Formats `value` the way a stream with 6 significant digits and default
/// (neither fixed nor scientific) float formatting would: whichever of
/// fixed-point or scientific notation is shorter for the magnitude, trailing
/// zeros dropped, and a signed two-digit exponent in scientific form.
#[must_use]
pub fn format_significant(value: f64) -> String {
    let significant_digits: usize = 6;

    if value.is_nan() {
        return "nan".to_string();
    }
    if value.is_infinite() {
        return if value.is_sign_negative() { "-inf" } else { "inf" }.to_string();
    }
    if value == 0.0 {
        return if value.is_sign_negative() { "-0" } else { "0" }.to_string();
    }

    let fractional_digits = significant_digits.saturating_sub(1);
    let scientific = format!("{value:.fractional_digits$e}");
    let Some((mantissa, exponent_text)) = scientific.split_once('e') else {
        return scientific;
    };
    let exponent: i32 = exponent_text.parse().unwrap_or(0);

    let use_fixed_point =
        exponent >= -4 && exponent < i32::try_from(significant_digits).unwrap_or(i32::MAX);
    if use_fixed_point {
        let decimals_wanted =
            i32::try_from(fractional_digits).unwrap_or(0).saturating_sub(exponent);
        let decimals = usize::try_from(decimals_wanted.max(0)).unwrap_or(0);
        strip_trailing_zeros(&format!("{value:.decimals$}"))
    } else {
        let sign = if exponent < 0 { '-' } else { '+' };
        format!("{}e{sign}{:02}", strip_trailing_zeros(mantissa), exponent.abs())
    }
}

fn strip_trailing_zeros(text: &str) -> String {
    if !text.contains('.') {
        return text.to_string();
    }
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn print_scalar(
    out: &mut String,
    type_name: &str,
    reader: &mut Reader<'_>,
) -> Result<(), CodecError> {
    match type_name {
        "bool" => out.push_str(if reader.read_u8()? != 0 { "true" } else { "false" }),
        "byte" | "int8" => out.push_str(&reader.read_i8()?.to_string()),
        "char" | "uint8" => out.push_str(&reader.read_u8()?.to_string()),
        "int16" => out.push_str(&reader.read_i16()?.to_string()),
        "uint16" => out.push_str(&reader.read_u16()?.to_string()),
        "int32" => out.push_str(&reader.read_i32()?.to_string()),
        "uint32" => out.push_str(&reader.read_u32()?.to_string()),
        "int64" => out.push_str(&reader.read_i64()?.to_string()),
        "uint64" => out.push_str(&reader.read_u64()?.to_string()),
        "float32" => out.push_str(&format_significant(f64::from(reader.read_f32()?))),
        "float64" => out.push_str(&format_significant(reader.read_f64()?)),
        "string" => {
            out.push('"');
            out.push_str(&String::from_utf8_lossy(reader.read_string()?));
            out.push('"');
        }
        "time" => {
            let (sec, nsec) = reader.read_time()?;
            let _ = write!(out, "{sec}.{nsec}");
        }
        "duration" => {
            let (sec, nsec) = reader.read_duration()?;
            let _ = write!(out, "{sec}.{nsec}");
        }
        _ => {
            let _ = write!(out, "<unresolved type {type_name}>");
        }
    }
    Ok(())
}

/// What a printer frame is the output of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameKind {
    Scalar,
    Message,
    ScalarArray,
    ByteArray,
    MessageArray,
}

/// One open field of the printer.
struct Frame {
    indent: String,
    kind: FrameKind,
    count: usize,
    index: usize,
}

/// Turns [`walk`] events into the indented dump. Elements past
/// [`MAX_ARRAY_PRINT_ELEMENTS`] are walked, so later offsets stay right, and not shown.
struct Printer<'o, 'b> {
    out: &'o mut String,
    bytes: &'b [u8],
    stack: Vec<Frame>,
}

impl Printer<'_, '_> {
    /// True when a message-array element in `stack[..depth]` is past the cap.
    fn hidden(&self, depth: usize) -> bool {
        self.stack.iter().take(depth).any(|frame| {
            frame.kind == FrameKind::MessageArray && frame.index >= MAX_ARRAY_PRINT_ELEMENTS
        })
    }

    fn child_indent(&self) -> String {
        match self.stack.last() {
            None => String::new(),
            Some(frame) if frame.kind == FrameKind::MessageArray => format!("{}    ", frame.indent),
            Some(frame) => format!("{}  ", frame.indent),
        }
    }

    fn on(&mut self, event: WalkEvent<'_>) {
        let depth = self.stack.len();
        let below = depth.saturating_sub(1);
        match event {
            WalkEvent::Enter { field } => self.enter(field),
            WalkEvent::Leave => {
                if let Some(frame) = self.stack.pop()
                    && frame.kind == FrameKind::Scalar
                    && !self.hidden(self.stack.len())
                {
                    self.out.push('\n');
                }
            }
            WalkEvent::ArrayStart { count, .. } => {
                let hidden = self.hidden(depth);
                let Some(frame) = self.stack.last_mut() else { return };
                frame.count = count;
                if hidden {
                    return;
                }
                match frame.kind {
                    FrameKind::ByteArray => {
                        let _ = writeln!(self.out, " <{count} bytes>");
                    }
                    FrameKind::ScalarArray => self.out.push_str(" ["),
                    FrameKind::MessageArray => self.out.push('\n'),
                    FrameKind::Scalar | FrameKind::Message => {}
                }
            }
            WalkEvent::Element { index } => {
                let hidden = self.hidden(below);
                let Some(frame) = self.stack.last_mut() else { return };
                frame.index = index;
                if hidden || index >= MAX_ARRAY_PRINT_ELEMENTS {
                    return;
                }
                match frame.kind {
                    FrameKind::ScalarArray if index > 0 => self.out.push_str(", "),
                    FrameKind::MessageArray => {
                        let _ = writeln!(self.out, "{}  -", frame.indent);
                    }
                    _ => {}
                }
            }
            WalkEvent::Scalar { type_name, span } => {
                let Some(frame) = self.stack.last() else { return };
                let shown =
                    frame.kind == FrameKind::Scalar || frame.index < MAX_ARRAY_PRINT_ELEMENTS;
                if shown && !self.hidden(below) {
                    let wire = self.bytes.get(span).unwrap_or_default();
                    let _ = print_scalar(self.out, type_name, &mut Reader::new(wire));
                }
            }
            WalkEvent::Unresolved { type_name } => {
                if self.hidden(depth) {
                    return;
                }
                if let Some(frame) = self.stack.last() {
                    let _ = writeln!(self.out, "{}  <unresolved type {type_name}>", frame.indent);
                }
            }
            WalkEvent::ArrayEnd => {
                if self.hidden(below) {
                    return;
                }
                let Some(frame) = self.stack.last() else { return };
                let count = frame.count;
                let over = count > MAX_ARRAY_PRINT_ELEMENTS;
                match frame.kind {
                    FrameKind::ScalarArray => {
                        if over {
                            let _ = write!(self.out, ", ... ({count} total)");
                        }
                        self.out.push_str("]\n");
                    }
                    FrameKind::MessageArray if over => {
                        let _ = writeln!(self.out, "{}  ... ({count} total)", frame.indent);
                    }
                    _ => {}
                }
            }
            WalkEvent::Bytes { .. } => {}
        }
    }

    fn enter(&mut self, field: &ReflectedField) {
        let indent = self.child_indent();
        let builtin = is_builtin_type(&field.type_name);
        let kind = match (field.is_array, builtin) {
            (false, true) => FrameKind::Scalar,
            (false, false) => FrameKind::Message,
            (true, _) if is_byte_array(field) => FrameKind::ByteArray,
            (true, true) => FrameKind::ScalarArray,
            (true, false) => FrameKind::MessageArray,
        };
        if !self.hidden(self.stack.len()) {
            let _ = match kind {
                FrameKind::Scalar => write!(self.out, "{indent}{}: ", field.name),
                FrameKind::Message => writeln!(self.out, "{indent}{}:", field.name),
                _ => write!(self.out, "{indent}{}:", field.name),
            };
        }
        self.stack.push(Frame { indent, kind, count: 0, index: 0 });
    }
}
