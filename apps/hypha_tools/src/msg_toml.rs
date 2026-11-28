//! A message as TOML, against a runtime schema: the request that `hypha service call`
//! encodes, its template, and the response it prints.
//!
//! A request is a TOML document whose keys are the message's fields: `data = true`; a
//! nested message is an inline table or dotted keys (`goal = { x = 1.0, y = 2.0 }` or
//! `goal.x = 1.0`), an array a TOML array, an array of messages an array of inline tables,
//! a `time` or `duration` the table `{ secs = 1, nsecs = 500 }`, and a `uint8[]`
//! (`byte[]`, `char[]`) an array of integers or a string of its bytes. The `toml` crate
//! parses the text; a field the text leaves out takes its zero value, as in ROS. A key
//! that is no field, a value of another TOML type than its field's (text for a number, a
//! float for an integer), a number out of its field's range and a fixed array of another
//! length are refused, each naming the field. A response and a template are printed in
//! the same form, a nested message as dotted keys, so either is a request again.

use std::fmt::Write as _;
use std::ops::Range;

use hypha_node::sdk::msgdef::spec::MAX_NESTING_DEPTH;
use toml::{Table, Value};

use crate::msg_patch::{ScalarValue, encode_scalar, read_scalar};
use crate::msg_reflect::{
    MessageSchema, ReflectedField, WalkEvent, is_builtin_type, is_byte_array, walk,
};

/// Parses `input`, a TOML document; empty text is an empty table.
///
/// # Errors
/// The `toml` crate's reason, with the line and column, when `input` is not TOML.
pub fn parse_request(input: &str) -> Result<Table, String> {
    input.parse::<Table>().map_err(|cause| cause.to_string().trim_end().to_owned())
}

/// Encodes `table` as a `type_name` message of `schema`. A field left out takes its zero
/// value.
///
/// # Errors
/// The reason, naming the field, when the table does not fit the message.
pub fn encode_message(
    schema: &mut MessageSchema,
    type_name: &str,
    table: &Table,
) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    Encoder { schema }.message(type_name, Some(table), "", &mut out)?;
    Ok(out)
}

struct Encoder<'s> {
    schema: &'s mut MessageSchema,
}

impl Encoder<'_> {
    fn message(
        &mut self,
        type_name: &str,
        table: Option<&Table>,
        path: &str,
        out: &mut Vec<u8>,
    ) -> Result<(), String> {
        let fields = self
            .schema
            .fields_for(type_name)
            .ok_or_else(|| format!("the definition holds no type {type_name}"))?;
        if let Some(table) = table {
            for key in table.keys() {
                if !fields.iter().any(|field| &field.name == key) {
                    let names: Vec<&str> = fields.iter().map(|field| field.name.as_str()).collect();
                    let known = if names.is_empty() { "none".to_owned() } else { names.join(", ") };
                    return Err(format!(
                        "{type_name} has no field '{}'; its fields: {known}",
                        joined(path, key)
                    ));
                }
            }
        }
        for field in fields.iter() {
            let given = table.and_then(|table| table.get(&field.name));
            self.field(field, given, &joined(path, &field.name), out)?;
        }
        Ok(())
    }

    fn field(
        &mut self,
        field: &ReflectedField,
        value: Option<&Value>,
        path: &str,
        out: &mut Vec<u8>,
    ) -> Result<(), String> {
        if !field.is_array {
            return self.element(&field.type_name, value, path, out);
        }
        let given: Vec<Option<&Value>> = match value {
            Some(Value::Array(items)) => items.iter().map(Some).collect(),
            None if field.is_fixed_length => vec![None; field.array_length],
            None => Vec::new(),
            Some(Value::String(text)) if is_byte_array(field) => {
                return self.bytes(field, text.as_bytes(), path, out);
            }
            Some(other) => {
                return Err(format!(
                    "{path}: a {}[] is an array, as [1, 2], not {}",
                    field.type_name,
                    kind(other)
                ));
            }
        };
        self.length(field, given.len(), path, out)?;
        for (index, item) in given.into_iter().enumerate() {
            self.element(&field.type_name, item, &format!("{path}[{index}]"), out)?;
        }
        Ok(())
    }

    /// A byte array written as a string: its UTF-8 bytes.
    fn bytes(
        &mut self,
        field: &ReflectedField,
        bytes: &[u8],
        path: &str,
        out: &mut Vec<u8>,
    ) -> Result<(), String> {
        self.length(field, bytes.len(), path, out)?;
        out.extend_from_slice(bytes);
        Ok(())
    }

    /// Checks a fixed array's length, or writes a variable array's.
    #[allow(clippy::unused_self)]
    fn length(
        &self,
        field: &ReflectedField,
        count: usize,
        path: &str,
        out: &mut Vec<u8>,
    ) -> Result<(), String> {
        if field.is_fixed_length {
            if count != field.array_length {
                return Err(format!(
                    "{path}: a {}[{}] takes {} values, not {count}",
                    field.type_name, field.array_length, field.array_length
                ));
            }
            return Ok(());
        }
        let count = u32::try_from(count).map_err(|_| format!("{path}: too long"))?;
        out.extend_from_slice(&count.to_le_bytes());
        Ok(())
    }

    fn element(
        &mut self,
        type_name: &str,
        value: Option<&Value>,
        path: &str,
        out: &mut Vec<u8>,
    ) -> Result<(), String> {
        if !is_builtin_type(type_name) {
            let table = match value {
                None => None,
                Some(Value::Table(table)) => Some(table),
                Some(other) => {
                    return Err(format!(
                        "{}: a {type_name} is a table of its fields, as {{ x = 1.0 }}, not {}",
                        shown(path),
                        kind(other)
                    ));
                }
            };
            return self.message(type_name, table, path, out);
        }
        let scalar = scalar_of(type_name, value).map_err(|why| format!("{path}: {why}"))?;
        let wire = encode_scalar(type_name, &scalar).map_err(|why| format!("{path}: {why}"))?;
        out.extend_from_slice(&wire);
        Ok(())
    }
}

/// The value of a builtin field; no value is its zero.
fn scalar_of(type_name: &str, value: Option<&Value>) -> Result<ScalarValue, String> {
    let Some(value) = value else {
        return Ok(zero(type_name));
    };
    let refused = || format!("expected {}, found {}", expected(type_name), kind(value));
    match (type_name, value) {
        ("bool", Value::Boolean(flag)) => Ok(ScalarValue::Bool(*flag)),
        ("string", Value::String(text)) => Ok(ScalarValue::Text(text.as_bytes().to_vec())),
        ("float32" | "float64", Value::Float(number)) => Ok(ScalarValue::Float(*number)),
        // An integer is an exact float here; beyond 2^53 it rounds, as in ROS.
        #[allow(clippy::cast_precision_loss)]
        ("float32" | "float64", Value::Integer(number)) => Ok(ScalarValue::Float(*number as f64)),
        ("time" | "duration", Value::Table(table)) => time_of(type_name, table),
        (_, Value::Integer(number))
            if !matches!(type_name, "bool" | "string" | "time" | "duration") =>
        {
            Ok(ScalarValue::Int(*number))
        }
        _ => Err(refused()),
    }
}

/// A `time` or `duration` from `{ secs = .., nsecs = .. }`; a part left out is 0.
fn time_of(type_name: &str, table: &Table) -> Result<ScalarValue, String> {
    if let Some(key) = table.keys().find(|key| !matches!(key.as_str(), "secs" | "nsecs")) {
        return Err(format!("a {type_name} has secs and nsecs, not '{key}'"));
    }
    let part = |name: &str| match table.get(name) {
        None => Ok(0_i64),
        Some(Value::Integer(number)) => Ok(*number),
        Some(other) => Err(format!("{name}: expected an integer, found {}", kind(other))),
    };
    let (secs, nsecs) = (part("secs")?, part("nsecs")?);
    let range = |name: &str| format!("{name} is out of range for a {type_name}");
    if type_name == "time" {
        Ok(ScalarValue::Time(
            u32::try_from(secs).map_err(|_| range("secs"))?,
            u32::try_from(nsecs).map_err(|_| range("nsecs"))?,
        ))
    } else {
        Ok(ScalarValue::Duration(
            i32::try_from(secs).map_err(|_| range("secs"))?,
            i32::try_from(nsecs).map_err(|_| range("nsecs"))?,
        ))
    }
}

fn zero(type_name: &str) -> ScalarValue {
    match type_name {
        "bool" => ScalarValue::Bool(false),
        "string" => ScalarValue::Text(Vec::new()),
        "float32" | "float64" => ScalarValue::Float(0.0),
        "time" => ScalarValue::Time(0, 0),
        "duration" => ScalarValue::Duration(0, 0),
        _ => ScalarValue::Int(0),
    }
}

fn expected(type_name: &str) -> String {
    match type_name {
        "bool" => "true or false".to_owned(),
        "string" => "a string in quotes".to_owned(),
        "float32" | "float64" => format!("a number ({type_name})"),
        "time" | "duration" => format!("{{ secs = 0, nsecs = 0 }} ({type_name})"),
        _ => format!("an integer ({type_name})"),
    }
}

/// The TOML type of `value`, with the value, for a refusal.
fn kind(value: &Value) -> String {
    match value {
        Value::String(text) => format!("the string {}", quoted(text)),
        Value::Integer(number) => format!("the integer {number}"),
        Value::Float(number) => format!("the float {}", float_text(*number)),
        Value::Boolean(flag) => format!("the boolean {flag}"),
        Value::Datetime(date) => format!("the date-time {date}"),
        Value::Array(_) => "an array".to_owned(),
        Value::Table(_) => "a table".to_owned(),
    }
}

fn joined(path: &str, name: &str) -> String {
    if path.is_empty() { name.to_owned() } else { format!("{path}.{name}") }
}

fn shown(path: &str) -> &str {
    if path.is_empty() { "the message" } else { path }
}

/// A message as a tree, in field order, of TOML values already written out.
#[derive(Debug, Clone, PartialEq)]
enum Node {
    Value(String),
    Array(Vec<Node>),
    Message(Vec<(String, Node)>),
}

/// Writes `fields` as TOML lines: a nested message of fields as dotted keys, the rest as
/// `key = value`.
fn write_fields(out: &mut String, prefix: &str, fields: &[(String, Node)]) {
    for (name, node) in fields {
        let key = format!("{prefix}{name}");
        match node {
            Node::Message(inner) if !inner.is_empty() => {
                write_fields(out, &format!("{key}."), inner);
            }
            _ => {
                let _ = writeln!(out, "{key} = {}", inline(node));
            }
        }
    }
}

fn inline(node: &Node) -> String {
    match node {
        Node::Value(text) => text.clone(),
        Node::Array(items) => {
            let items: Vec<String> = items.iter().map(inline).collect();
            format!("[{}]", items.join(", "))
        }
        Node::Message(fields) if fields.is_empty() => "{}".to_owned(),
        Node::Message(fields) => {
            let fields: Vec<String> =
                fields.iter().map(|(name, node)| format!("{name} = {}", inline(node))).collect();
            format!("{{ {} }}", fields.join(", "))
        }
    }
}

/// `text` as a TOML basic string on one line: `"a\"b\n"`.
fn quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len().saturating_add(2));
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            control if control.is_control() => {
                let _ = write!(out, "\\u{:04X}", u32::from(control));
            }
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// `value` as TOML text: a float with its point (`1.0`), `nan`, `inf`.
fn float_text(value: f64) -> String {
    if value.is_nan() {
        "nan".to_owned()
    } else if value.is_infinite() {
        if value > 0.0 { "inf" } else { "-inf" }.to_owned()
    } else {
        format!("{value:?}")
    }
}

/// `value` of `type_name` as TOML text. A `float32` is shown at its own precision, a
/// string that is not UTF-8 with U+FFFD for its bad bytes.
fn scalar_text(type_name: &str, value: &ScalarValue) -> String {
    match value {
        ScalarValue::Bool(flag) => flag.to_string(),
        ScalarValue::Int(number) => number.to_string(),
        ScalarValue::UInt(number) => number.to_string(),
        // A float32 widened to f64 is exact; narrowing it back loses nothing.
        #[allow(clippy::cast_possible_truncation)]
        ScalarValue::Float(number) if type_name == "float32" => {
            let narrow = *number as f32;
            if narrow.is_finite() { format!("{narrow:?}") } else { float_text(*number) }
        }
        ScalarValue::Float(number) => float_text(*number),
        ScalarValue::Text(bytes) => quoted(&String::from_utf8_lossy(bytes)),
        ScalarValue::Time(secs, nsecs) => format!("{{ secs = {secs}, nsecs = {nsecs} }}"),
        ScalarValue::Duration(secs, nsecs) => format!("{{ secs = {secs}, nsecs = {nsecs} }}"),
    }
}

/// The TOML of a `type_name` message with every field at its zero value, as
/// `hypha service call <service>` prints it to start a request from; empty for a message
/// of no fields.
#[must_use]
pub fn template(schema: &mut MessageSchema, type_name: &str) -> String {
    let fields = zero_fields(schema, type_name, 0);
    let mut out = String::new();
    write_fields(&mut out, "", &fields);
    out
}

fn zero_fields(schema: &mut MessageSchema, type_name: &str, depth: usize) -> Vec<(String, Node)> {
    let Some(fields) = schema.fields_for(type_name) else {
        return Vec::new();
    };
    if depth >= MAX_NESTING_DEPTH {
        return Vec::new();
    }
    fields
        .iter()
        .map(|field| {
            let mut single = || {
                if is_builtin_type(&field.type_name) {
                    Node::Value(scalar_text(&field.type_name, &zero(&field.type_name)))
                } else {
                    Node::Message(zero_fields(schema, &field.type_name, depth.saturating_add(1)))
                }
            };
            let node = if !field.is_array {
                single()
            } else if field.is_fixed_length {
                let item = single();
                Node::Array(vec![item; field.array_length])
            } else {
                Node::Array(Vec::new())
            };
            (field.name.clone(), node)
        })
        .collect()
}

/// The `type_name` message in `bytes` as TOML, in the form [`template`] prints; empty for
/// a message of no fields.
///
/// # Errors
/// The reason when `bytes` is not a `type_name` message of `schema`.
pub fn decode_message(
    schema: &mut MessageSchema,
    type_name: &str,
    bytes: &[u8],
) -> Result<String, String> {
    if schema.fields_for(type_name).is_none() {
        return Err(format!("the definition holds no type {type_name}"));
    }
    let mut builder = Builder { bytes, stack: vec![Frame::Message(Vec::new())], failure: None };
    let walked = walk(schema, type_name, bytes, &mut |event| builder.on(event));
    walked.map_err(|cause| cause.to_string())?;
    if let Some(failure) = builder.failure {
        return Err(failure);
    }
    let Some(Frame::Message(fields)) = builder.stack.pop() else {
        return Err("the payload does not match the schema".to_owned());
    };
    let mut out = String::new();
    write_fields(&mut out, "", &fields);
    Ok(out)
}

/// One open part of the tree that [`decode_message`] builds.
enum Frame {
    /// A field; `None` until its value is complete.
    Field(String, Option<Node>),
    /// A message: the root, a nested field's or an array element's.
    Message(Vec<(String, Node)>),
    /// An array; true when its elements are messages.
    Array(Vec<Node>, bool),
}

struct Builder<'b> {
    bytes: &'b [u8],
    stack: Vec<Frame>,
    failure: Option<String>,
}

impl Builder<'_> {
    fn fail(&mut self, reason: String) {
        self.failure.get_or_insert(reason);
    }

    fn wire(&self, span: Range<usize>) -> &[u8] {
        self.bytes.get(span).unwrap_or_default()
    }

    /// Puts a finished `node` into the open frame below it.
    fn place(&mut self, node: Node) {
        match self.stack.last_mut() {
            Some(Frame::Field(_, value)) => *value = Some(node),
            Some(Frame::Array(items, _)) => items.push(node),
            _ => self.fail("the payload does not match the schema".to_owned()),
        }
    }

    /// Closes an array element that is a message, if one is open.
    fn close_element(&mut self) {
        if let Some(Frame::Message(_)) = self.stack.last()
            && matches!(self.stack.iter().rev().nth(1), Some(Frame::Array(_, true)))
            && let Some(Frame::Message(fields)) = self.stack.pop()
        {
            self.place(Node::Message(fields));
        }
    }

    fn on(&mut self, event: WalkEvent<'_>) {
        match event {
            WalkEvent::Enter { field } => {
                self.stack.push(Frame::Field(field.name.clone(), None));
                if !field.is_array && !is_builtin_type(&field.type_name) {
                    self.stack.push(Frame::Message(Vec::new()));
                }
                if field.is_array {
                    let messages = !is_builtin_type(&field.type_name);
                    self.stack.push(Frame::Array(Vec::new(), messages));
                }
            }
            WalkEvent::Element { .. } => {
                self.close_element();
                if let Some(Frame::Array(_, true)) = self.stack.last() {
                    self.stack.push(Frame::Message(Vec::new()));
                }
            }
            WalkEvent::Scalar { type_name, span } => {
                match read_scalar(type_name, self.wire(span)) {
                    Ok(value) => self.place(Node::Value(scalar_text(type_name, &value))),
                    Err(cause) => self.fail(cause),
                }
            }
            WalkEvent::Bytes { span, .. } => {
                let items: Vec<Node> =
                    self.wire(span).iter().map(|byte| Node::Value(byte.to_string())).collect();
                if let Some(Frame::Array(held, _)) = self.stack.last_mut() {
                    held.extend(items);
                }
            }
            WalkEvent::ArrayStart { .. } => {}
            WalkEvent::ArrayEnd => {
                self.close_element();
                if let Some(Frame::Array(items, _)) = self.stack.pop() {
                    self.place(Node::Array(items));
                }
            }
            WalkEvent::Leave => {
                if let Some(Frame::Message(_)) = self.stack.last()
                    && let Some(Frame::Message(fields)) = self.stack.pop()
                {
                    self.place(Node::Message(fields));
                }
                match self.stack.pop() {
                    Some(Frame::Field(name, Some(node))) => match self.stack.last_mut() {
                        Some(Frame::Message(fields)) => fields.push((name, node)),
                        _ => self.fail("the payload does not match the schema".to_owned()),
                    },
                    _ => self.fail("the payload does not match the schema".to_owned()),
                }
            }
            WalkEvent::Unresolved { type_name } => {
                self.fail(format!("the definition holds no type {type_name}"));
            }
        }
    }
}
