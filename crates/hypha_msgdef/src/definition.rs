//! Splits concatenated definition text into one parsed message per section.

use std::collections::BTreeMap;
use std::fmt;

use crate::parse::parse_message;
use crate::resolve::SpecSet;
use crate::spec::TypeName;

/// Why a definition text could not be read into specs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefinitionError {
    /// One section's name or body is not a valid message.
    Section {
        /// The section's `package/Type` name, as written in the text.
        type_name: String,
        /// What is wrong with the section.
        reason: String,
    },
}

impl fmt::Display for DefinitionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Section { type_name, reason } => {
                write!(formatter, "section `{type_name}` is not a valid message: {reason}")
            }
        }
    }
}

impl std::error::Error for DefinitionError {}

/// Splits concatenated definition text into one body string per fully qualified type
/// name. The first section belongs to `type_name`; each later one starts at a
/// `MSG: package/Type` header after a line of only `=`. Stray lines between are discarded.
#[must_use]
pub fn split_sections(type_name: &str, text: &str) -> BTreeMap<String, String> {
    let mut sections: BTreeMap<String, String> = BTreeMap::new();
    let mut current_name = String::from(type_name);
    let mut current_lines: Vec<&str> = Vec::new();
    let mut seeking_header = false;

    for raw_line in text.lines() {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if is_separator_line(line) {
            sections.insert(current_name.clone(), current_lines.join("\n"));
            current_lines.clear();
            seeking_header = true;
        } else if seeking_header {
            if let Some(name) = parse_msg_header(line) {
                current_name = name;
                seeking_header = false;
            }
        } else {
            current_lines.push(line);
        }
    }
    sections.insert(current_name, current_lines.join("\n"));
    sections
}

/// Parses every section of `text` into a [`SpecSet`]. The first section is
/// `root`; the others take the names of their `MSG:` headers.
///
/// # Errors
///
/// Returns [`DefinitionError::Section`] when a section's name is not
/// `package/Type`, or its body is not a valid message.
pub fn parse_definition(root: &TypeName, text: &str) -> Result<SpecSet, DefinitionError> {
    let mut specs = SpecSet::new();
    for (full_name, body) in split_sections(&root.full(), text) {
        let type_name = TypeName::parse(&full_name).ok_or_else(|| DefinitionError::Section {
            type_name: full_name.clone(),
            reason: String::from("the name is not `package/Type`"),
        })?;
        let spec = parse_message(type_name, &body).map_err(|error| DefinitionError::Section {
            type_name: full_name.clone(),
            reason: error.to_string(),
        })?;
        specs.insert(spec);
    }
    Ok(specs)
}

/// True when `line` is one or more `=` characters and nothing else.
fn is_separator_line(line: &str) -> bool {
    !line.is_empty() && line.chars().all(|c| c == '=')
}

/// Parses a `MSG: package/Type` header line, if `line` is one.
fn parse_msg_header(line: &str) -> Option<String> {
    let rest = line.trim().strip_prefix("MSG:")?;
    Some(String::from(rest.trim()))
}
