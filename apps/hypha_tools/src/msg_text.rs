//! A message from text: YAML or JSON, encoded against a runtime schema, as
//! `rosservice call /set_enabled "data: true"` takes a request.
//!
//! The text is the YAML that a message field list needs: a block mapping
//! (`data: true`, one `key: value` per line, nested by indentation, `- item` lists), or a
//! flow mapping (`{data: true}`, `{pose: {x: 1, y: 2}}`, `[1, 2, 3]`), of which JSON is one
//! spelling. A field the text leaves out takes its zero value, as in ROS. A key that is no
//! field, a value that is not of its field's type, and a fixed array of another length are
//! refused, each naming the field.

use std::fmt::Write as _;

use crate::msg_patch::{ScalarValue, encode_scalar, parse_value};
use crate::msg_reflect::{MessageSchema, ReflectedField, is_builtin_type, is_byte_array};

/// One node of the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextValue {
    /// A mapping, in the order written.
    Map(Vec<(String, TextValue)>),
    /// A sequence.
    List(Vec<TextValue>),
    /// A scalar; `quoted` when it was written in quotes, so it is text whatever it reads.
    Scalar {
        /// The text, unescaped.
        text: String,
        /// Written in quotes.
        quoted: bool,
    },
    /// No value (`key:` with nothing after it, `~`, `null`): the field's zero value.
    Null,
}

/// Parses `input` as YAML (block or flow) or JSON. Empty text is an empty mapping.
///
/// # Errors
/// The reason, with the line, when the text is not one of the forms this reads.
pub fn parse_text(input: &str) -> Result<TextValue, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(TextValue::Map(Vec::new()));
    }
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        let mut flow = Flow { chars: trimmed.chars().collect(), at: 0 };
        let value = flow.value()?;
        flow.skip_space();
        if flow.at < flow.chars.len() {
            return Err(format!("unexpected text after the value: '{}'", flow.rest()));
        }
        return Ok(value);
    }
    let lines: Vec<Line> = input
        .lines()
        .enumerate()
        .filter_map(|(index, raw)| Line::of(index.saturating_add(1), raw))
        .collect();
    let mut block = Block { lines: &lines, at: 0 };
    let first = lines.first().map_or(0, |line| line.indent);
    let value = block.node(first)?;
    if let Some(line) = lines.get(block.at) {
        return Err(format!("line {}: unexpected indentation of '{}'", line.number, line.text));
    }
    Ok(value)
}

/// One line of block text: its number, indentation and content without a comment.
struct Line {
    number: usize,
    indent: usize,
    text: String,
}

impl Line {
    fn of(number: usize, raw: &str) -> Option<Self> {
        let text = strip_comment(raw);
        let content = text.trim_start();
        if content.trim().is_empty() {
            return None;
        }
        let indent = text.len().saturating_sub(content.len());
        Some(Self { number, indent, text: content.trim_end().to_owned() })
    }
}

/// `raw` without a `#` comment outside quotes.
fn strip_comment(raw: &str) -> &str {
    let mut quote = None;
    let mut previous = ' ';
    for (index, character) in raw.char_indices() {
        match (quote, character) {
            (None, '"' | '\'') => quote = Some(character),
            (Some(open), _) if character == open => quote = None,
            (None, '#') if previous.is_whitespace() => return raw.get(..index).unwrap_or(raw),
            _ => {}
        }
        previous = character;
    }
    raw
}

/// The block reader: lines consumed from `at`.
struct Block<'a> {
    lines: &'a [Line],
    at: usize,
}

impl Block<'_> {
    /// The node whose lines start at `indent`.
    fn node(&mut self, indent: usize) -> Result<TextValue, String> {
        let Some(line) = self.lines.get(self.at) else {
            return Ok(TextValue::Null);
        };
        if line.text == "-" || line.text.starts_with("- ") {
            self.list(indent)
        } else {
            self.map(indent)
        }
    }

    fn map(&mut self, indent: usize) -> Result<TextValue, String> {
        let mut entries = Vec::new();
        while let Some(line) = self.lines.get(self.at) {
            if line.indent < indent {
                break;
            }
            if line.indent > indent {
                return Err(format!("line {}: unexpected indentation", line.number));
            }
            let (key, rest) = split_key(&line.text).ok_or_else(|| {
                format!("line {}: expected 'field: value', got '{}'", line.number, line.text)
            })?;
            let number = line.number;
            self.at = self.at.saturating_add(1);
            let value = if rest.is_empty() {
                match self.lines.get(self.at) {
                    Some(next) if next.indent > indent => self.node(next.indent)?,
                    // A list may sit at the key's own indentation, as YAML allows.
                    Some(next) if next.indent == indent && next.text.starts_with('-') => {
                        self.list(indent)?
                    }
                    _ => TextValue::Null,
                }
            } else {
                inline(rest).map_err(|why| format!("line {number}: {why}"))?
            };
            entries.push((key, value));
        }
        Ok(TextValue::Map(entries))
    }

    fn list(&mut self, indent: usize) -> Result<TextValue, String> {
        let mut items = Vec::new();
        while let Some(line) = self.lines.get(self.at) {
            let is_item = line.text == "-" || line.text.starts_with("- ");
            if line.indent != indent || !is_item {
                break;
            }
            let number = line.number;
            let rest = line.text.get(1..).unwrap_or_default().trim_start();
            self.at = self.at.saturating_add(1);
            if rest.is_empty() {
                let nested = match self.lines.get(self.at) {
                    Some(next) if next.indent > indent => self.node(next.indent)?,
                    _ => TextValue::Null,
                };
                items.push(nested);
            } else if let Some((key, value)) = split_key(rest) {
                // `- key: value` opens a mapping whose further keys sit under `key`.
                let inner = indent.saturating_add(line.text.len().saturating_sub(rest.len()));
                let first = if value.is_empty() {
                    match self.lines.get(self.at) {
                        Some(next) if next.indent > inner => self.node(next.indent)?,
                        _ => TextValue::Null,
                    }
                } else {
                    inline(value).map_err(|why| format!("line {number}: {why}"))?
                };
                let mut entries = vec![(key, first)];
                if let TextValue::Map(more) = self.map(inner)? {
                    entries.extend(more);
                }
                items.push(TextValue::Map(entries));
            } else {
                items.push(inline(rest).map_err(|why| format!("line {number}: {why}"))?);
            }
        }
        Ok(TextValue::List(items))
    }
}

/// `key: rest` of a block line, the key unquoted; `None` when the line has no key.
fn split_key(text: &str) -> Option<(String, &str)> {
    let (key, rest) = if let Some(quote) = text.chars().next().filter(|c| *c == '"' || *c == '\'') {
        let mut flow = Flow { chars: text.chars().collect(), at: 0 };
        let key = flow.quoted(quote).ok()?;
        let consumed: usize = text.chars().take(flow.at).map(char::len_utf8).sum();
        let rest = text.get(consumed..)?.trim_start().strip_prefix(':')?;
        (key, rest)
    } else {
        let colon = text.find(": ").or_else(|| text.strip_suffix(':').map(str::len))?;
        let key = text.get(..colon)?.trim();
        let rest = text.get(colon.saturating_add(1)..).unwrap_or_default();
        (key.to_owned(), rest)
    };
    if key.is_empty() {
        return None;
    }
    Some((key, rest.trim()))
}

/// The value after `key:` on one line: flow, quoted or plain.
fn inline(text: &str) -> Result<TextValue, String> {
    let mut flow = Flow { chars: text.chars().collect(), at: 0 };
    let value =
        if text.starts_with(['{', '[', '"', '\'']) { flow.value()? } else { plain(text.trim()) };
    if text.starts_with(['{', '[', '"', '\'']) {
        flow.skip_space();
        if flow.at < flow.chars.len() {
            return Err(format!("unexpected text after the value: '{}'", flow.rest()));
        }
    }
    Ok(value)
}

/// A plain scalar, `~` and `null` being no value.
fn plain(text: &str) -> TextValue {
    if matches!(text, "~" | "null" | "Null" | "NULL" | "") {
        return TextValue::Null;
    }
    TextValue::Scalar { text: text.to_owned(), quoted: false }
}

/// The flow reader: `{...}`, `[...]` and scalars, JSON included.
struct Flow {
    chars: Vec<char>,
    at: usize,
}

impl Flow {
    fn rest(&self) -> String {
        self.chars.iter().skip(self.at).collect()
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.at).copied()
    }

    fn bump(&mut self) {
        self.at = self.at.saturating_add(1);
    }

    fn skip_space(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.bump();
        }
    }

    fn expect(&mut self, wanted: char) -> Result<(), String> {
        self.skip_space();
        if self.peek() == Some(wanted) {
            self.bump();
            Ok(())
        } else {
            Err(format!("expected '{wanted}' at '{}'", self.rest()))
        }
    }

    fn value(&mut self) -> Result<TextValue, String> {
        self.skip_space();
        match self.peek() {
            Some('{') => self.map(),
            Some('[') => self.list(),
            Some(quote @ ('"' | '\'')) => {
                Ok(TextValue::Scalar { text: self.quoted(quote)?, quoted: true })
            }
            Some(_) => Ok(plain(&self.plain_text(&[',', '}', ']']))),
            None => Err("a value is missing".to_owned()),
        }
    }

    fn map(&mut self) -> Result<TextValue, String> {
        self.expect('{')?;
        let mut entries = Vec::new();
        loop {
            self.skip_space();
            if self.peek() == Some('}') {
                self.bump();
                return Ok(TextValue::Map(entries));
            }
            let key = match self.peek() {
                Some(quote @ ('"' | '\'')) => self.quoted(quote)?,
                _ => self.plain_text(&[':', ',', '}']),
            };
            if key.is_empty() {
                return Err(format!("a key is missing at '{}'", self.rest()));
            }
            self.expect(':')?;
            self.skip_space();
            let value = match self.peek() {
                Some(',' | '}') => TextValue::Null,
                _ => self.value()?,
            };
            entries.push((key, value));
            self.skip_space();
            match self.peek() {
                Some(',') => self.bump(),
                Some('}') => {}
                _ => return Err(format!("expected ',' or '}}' at '{}'", self.rest())),
            }
        }
    }

    fn list(&mut self) -> Result<TextValue, String> {
        self.expect('[')?;
        let mut items = Vec::new();
        loop {
            self.skip_space();
            if self.peek() == Some(']') {
                self.bump();
                return Ok(TextValue::List(items));
            }
            items.push(self.value()?);
            self.skip_space();
            match self.peek() {
                Some(',') => self.bump(),
                Some(']') => {}
                _ => return Err(format!("expected ',' or ']' at '{}'", self.rest())),
            }
        }
    }

    /// Plain text up to one of `stops`, trimmed.
    fn plain_text(&mut self, stops: &[char]) -> String {
        let mut text = String::new();
        while let Some(next) = self.peek() {
            if stops.contains(&next) {
                break;
            }
            text.push(next);
            self.bump();
        }
        text.trim().to_owned()
    }

    /// A quoted scalar: `"..."` with JSON escapes, `'...'` with `''` for a quote.
    fn quoted(&mut self, quote: char) -> Result<String, String> {
        self.bump();
        let mut text = String::new();
        loop {
            let Some(next) = self.peek() else {
                return Err(format!("an unterminated {quote}quoted{quote} text"));
            };
            self.bump();
            match (quote, next) {
                ('\'', '\'') if self.peek() == Some('\'') => {
                    text.push('\'');
                    self.bump();
                }
                (open, close) if open == close => return Ok(text),
                ('"', '\\') => text.push(self.escape()?),
                _ => text.push(next),
            }
        }
    }

    fn escape(&mut self) -> Result<char, String> {
        let Some(next) = self.peek() else {
            return Err("an escape at the end of the text".to_owned());
        };
        self.bump();
        Ok(match next {
            'n' => '\n',
            't' => '\t',
            'r' => '\r',
            '0' => '\0',
            'b' => '\u{8}',
            'f' => '\u{c}',
            'u' => {
                let digits: String = self.chars.iter().skip(self.at).take(4).collect();
                self.at = self.at.saturating_add(4);
                u32::from_str_radix(&digits, 16)
                    .ok()
                    .and_then(char::from_u32)
                    .ok_or_else(|| format!("'\\u{digits}' is not a character"))?
            }
            other => other,
        })
    }
}

/// Encodes `value` as a `type_name` message of `schema`. A field left out takes its zero
/// value.
///
/// # Errors
/// The reason, naming the field, when the text does not fit the message.
pub fn encode_message(
    schema: &mut MessageSchema,
    type_name: &str,
    value: &TextValue,
) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    Encoder { schema }.message(type_name, value, "", &mut out)?;
    Ok(out)
}

struct Encoder<'s> {
    schema: &'s mut MessageSchema,
}

impl Encoder<'_> {
    fn message(
        &mut self,
        type_name: &str,
        value: &TextValue,
        path: &str,
        out: &mut Vec<u8>,
    ) -> Result<(), String> {
        let fields = self
            .schema
            .fields_for(type_name)
            .ok_or_else(|| format!("the definition holds no type {type_name}"))?;
        let entries: &[(String, TextValue)] = match value {
            TextValue::Map(entries) => entries,
            TextValue::Null => &[],
            _ => return Err(format!("{}: a {type_name} is a mapping of its fields", shown(path))),
        };
        for (key, _) in entries {
            if !fields.iter().any(|field| &field.name == key) {
                let names: Vec<&str> = fields.iter().map(|field| field.name.as_str()).collect();
                let known = if names.is_empty() { "none".to_owned() } else { names.join(", ") };
                return Err(format!(
                    "{type_name} has no field '{}'; its fields: {known}",
                    joined(path, key)
                ));
            }
        }
        for field in fields.iter() {
            let given = entries.iter().find(|(key, _)| key == &field.name).map(|(_, value)| value);
            self.field(field, given.unwrap_or(&TextValue::Null), &joined(path, &field.name), out)?;
        }
        Ok(())
    }

    fn field(
        &mut self,
        field: &ReflectedField,
        value: &TextValue,
        path: &str,
        out: &mut Vec<u8>,
    ) -> Result<(), String> {
        if !field.is_array {
            return self.element(&field.type_name, value, path, out);
        }
        let items: Vec<TextValue> = match value {
            TextValue::List(items) => items.clone(),
            TextValue::Null if field.is_fixed_length => {
                vec![TextValue::Null; field.array_length]
            }
            TextValue::Null => Vec::new(),
            TextValue::Scalar { text, quoted: true } if is_byte_array(field) => text
                .bytes()
                .map(|byte| TextValue::Scalar { text: byte.to_string(), quoted: false })
                .collect(),
            _ => return Err(format!("{path}: a {}[] is a list, as [1, 2]", field.type_name)),
        };
        if field.is_fixed_length {
            if items.len() != field.array_length {
                return Err(format!(
                    "{path}: a {}[{}] takes {} values, not {}",
                    field.type_name,
                    field.array_length,
                    field.array_length,
                    items.len()
                ));
            }
        } else {
            let count = u32::try_from(items.len()).map_err(|_| format!("{path}: too long"))?;
            out.extend_from_slice(&count.to_le_bytes());
        }
        for (index, item) in items.iter().enumerate() {
            self.element(&field.type_name, item, &format!("{path}[{index}]"), out)?;
        }
        Ok(())
    }

    fn element(
        &mut self,
        type_name: &str,
        value: &TextValue,
        path: &str,
        out: &mut Vec<u8>,
    ) -> Result<(), String> {
        if !is_builtin_type(type_name) {
            return self.message(type_name, value, path, out);
        }
        let scalar = scalar_of(type_name, value).map_err(|why| format!("{path}: {why}"))?;
        let wire = encode_scalar(type_name, &scalar).map_err(|why| format!("{path}: {why}"))?;
        out.extend_from_slice(&wire);
        Ok(())
    }
}

/// The value of a builtin field from its text; no value is its zero.
fn scalar_of(type_name: &str, value: &TextValue) -> Result<ScalarValue, String> {
    match value {
        TextValue::Null => parse_value(type_name, zero_text(type_name)),
        TextValue::Scalar { text, quoted: true } if type_name == "string" => {
            Ok(ScalarValue::Text(text.as_bytes().to_vec()))
        }
        TextValue::Scalar { text, .. } => match type_name {
            "string" => Ok(ScalarValue::Text(text.as_bytes().to_vec())),
            "bool" => match text.as_str() {
                "true" | "True" | "TRUE" | "1" => Ok(ScalarValue::Bool(true)),
                "false" | "False" | "FALSE" | "0" => Ok(ScalarValue::Bool(false)),
                _ => Err(format!("'{text}' is not a bool: expected true or false")),
            },
            "float32" | "float64" => match text.as_str() {
                ".inf" | ".Inf" | "inf" => Ok(ScalarValue::Float(f64::INFINITY)),
                "-.inf" | "-.Inf" | "-inf" => Ok(ScalarValue::Float(f64::NEG_INFINITY)),
                ".nan" | ".NaN" | "nan" => Ok(ScalarValue::Float(f64::NAN)),
                _ => parse_value(type_name, text),
            },
            _ => parse_value(type_name, text),
        },
        TextValue::Map(entries) if matches!(type_name, "time" | "duration") => {
            let part = |names: [&str; 2]| {
                entries.iter().find(|(key, _)| names.contains(&key.as_str())).map_or(
                    Ok(0_i64),
                    |(key, value)| match value {
                        TextValue::Scalar { text, .. } => text
                            .parse::<i64>()
                            .map_err(|_| format!("{key}: '{text}' is not an integer")),
                        _ => Err(format!("{key}: expected an integer")),
                    },
                )
            };
            let secs = part(["secs", "sec"])?;
            let nsecs = part(["nsecs", "nsec"])?;
            if type_name == "time" {
                let secs = u32::try_from(secs).map_err(|_| "secs out of range".to_owned())?;
                let nsecs = u32::try_from(nsecs).map_err(|_| "nsecs out of range".to_owned())?;
                Ok(ScalarValue::Time(secs, nsecs))
            } else {
                let secs = i32::try_from(secs).map_err(|_| "secs out of range".to_owned())?;
                let nsecs = i32::try_from(nsecs).map_err(|_| "nsecs out of range".to_owned())?;
                Ok(ScalarValue::Duration(secs, nsecs))
            }
        }
        _ => Err(format!("a {type_name} is one value, not a mapping or a list")),
    }
}

/// The text of a builtin type's zero value, as [`parse_value`] reads it.
fn zero_text(type_name: &str) -> &'static str {
    match type_name {
        "bool" => "false",
        "string" => "",
        _ => "0",
    }
}

fn joined(path: &str, name: &str) -> String {
    if path.is_empty() { name.to_owned() } else { format!("{path}.{name}") }
}

fn shown(path: &str) -> &str {
    if path.is_empty() { "the message" } else { path }
}

/// The text of a `type_name` message with every field at its zero value, as
/// `hypha service call <service>` prints it to start a request from; `{}` for a message of
/// no fields.
#[must_use]
pub fn template(schema: &mut MessageSchema, type_name: &str) -> String {
    let mut out = String::new();
    write_template(schema, type_name, "", &mut out, 0);
    if out.is_empty() { "{}\n".to_owned() } else { out }
}

fn write_template(
    schema: &mut MessageSchema,
    type_name: &str,
    indent: &str,
    out: &mut String,
    depth: usize,
) {
    let Some(fields) = schema.fields_for(type_name) else {
        return;
    };
    if depth >= hypha_node::sdk::msgdef::spec::MAX_NESTING_DEPTH {
        return;
    }
    for field in fields.iter() {
        let builtin = is_builtin_type(&field.type_name);
        if field.is_array {
            let zero = if builtin { zero_shown(&field.type_name) } else { "{}" };
            let items = vec![zero; field.array_length].join(", ");
            let _ = writeln!(out, "{indent}{}: [{items}]", field.name);
        } else if builtin {
            let _ = writeln!(out, "{indent}{}: {}", field.name, zero_shown(&field.type_name));
        } else {
            let _ = writeln!(out, "{indent}{}:", field.name);
            let inner = format!("{indent}  ");
            write_template(schema, &field.type_name, &inner, out, depth.saturating_add(1));
        }
    }
}

fn zero_shown(type_name: &str) -> &'static str {
    match type_name {
        "bool" => "false",
        "string" => "\"\"",
        "float32" | "float64" => "0.0",
        "time" | "duration" => "{secs: 0, nsecs: 0}",
        _ => "0",
    }
}
