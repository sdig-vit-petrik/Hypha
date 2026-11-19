//! Parameter reads across the C boundary.
//!
//! `hypha_config_value` reads a scalar and `hypha_config_list_value` an element of a
//! TOML list, each as it is stored, with a type tag.

use std::os::raw::c_char;
use std::ptr;

use hypha_core::parameters::{ListElement, ParamValue, Parameters};

use crate::config::HyphaConfig;

/// Result of reading a parameter.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HyphaParamResult {
    /// The parameter was present and was written to the output.
    Ok = 0,
    /// The parameter was not available.
    Absent = 1,
    /// The parameter was present but is not a value the read can return: a list
    /// read as a scalar, or a table element read as a scalar.
    Malformed = 2,
    /// An input pointer was null, or a key was not UTF-8.
    InvalidArgument = 3,
}

/// `HyphaConfigValue::kind` of a boolean.
pub const HYPHA_CONFIG_VALUE_BOOL: u32 = 0;
/// `HyphaConfigValue::kind` of a whole number.
pub const HYPHA_CONFIG_VALUE_INT: u32 = 1;
/// `HyphaConfigValue::kind` of a number with a fraction.
pub const HYPHA_CONFIG_VALUE_FLOAT: u32 = 2;
/// `HyphaConfigValue::kind` of text.
pub const HYPHA_CONFIG_VALUE_TEXT: u32 = 3;

/// A configured value as it is stored. Only the field named by `kind` is
/// meaningful; the others are zero or null. `text` is not NUL-terminated and
/// borrows the configuration until it is destroyed (`hypha_config_destroy`, or
/// `hypha_node_destroy` for a node's own configuration).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HyphaConfigValue {
    /// One of `HYPHA_CONFIG_VALUE_*`.
    pub kind: u32,
    /// 0 or 1 when `kind` is `HYPHA_CONFIG_VALUE_BOOL`, else 0.
    pub bool_value: u8,
    /// The whole number when `kind` is `HYPHA_CONFIG_VALUE_INT`, else 0.
    pub int_value: i64,
    /// The number when `kind` is `HYPHA_CONFIG_VALUE_FLOAT`, else 0.
    pub float_value: f64,
    /// The bytes when `kind` is `HYPHA_CONFIG_VALUE_TEXT`, else null.
    pub text: *const c_char,
    /// The byte length of `text`, else 0.
    pub text_len: usize,
}

impl HyphaConfigValue {
    /// A value of `kind` with every field zero or null.
    #[must_use]
    pub const fn empty(kind: u32) -> Self {
        Self { kind, bool_value: 0, int_value: 0, float_value: 0.0, text: ptr::null(), text_len: 0 }
    }
}

/// What a parameter read found.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParamRead<T> {
    /// The parameter, as it is stored.
    Found(T),
    /// No parameter of that name, index or field.
    Absent,
    /// A parameter of another shape: a list read as a scalar, a scalar read as a
    /// list, or a table element read as a scalar.
    Malformed,
}

/// The scalar parameter `name` of `config`.
#[must_use]
pub fn config_value<'a>(config: &'a HyphaConfig, name: &str) -> ParamRead<&'a ParamValue> {
    let Some(value) = config.parameters.value(name) else {
        let is_list = config.parameters.has_list(name);
        return if is_list { ParamRead::Malformed } else { ParamRead::Absent };
    };
    ParamRead::Found(value)
}

/// The number of elements of the list `name` of `config`.
#[must_use]
pub fn config_list_len(config: &HyphaConfig, name: &str) -> ParamRead<usize> {
    let Some(len) = config.parameters.list_len(name) else {
        let is_scalar = config.parameters.has(name);
        return if is_scalar { ParamRead::Malformed } else { ParamRead::Absent };
    };
    ParamRead::Found(len)
}

/// Element `index` of the list `name` of `config`, or its `field` when the element is
/// a table; an empty `field` names the element itself.
#[must_use]
pub fn config_list_value<'a>(
    config: &'a HyphaConfig,
    name: &str,
    index: usize,
    field: &str,
) -> ParamRead<&'a ParamValue> {
    match find_list_value(&config.parameters, name, index, field) {
        ListRead::Value(value) => ParamRead::Found(value),
        ListRead::Missing => ParamRead::Absent,
        ListRead::Table => ParamRead::Malformed,
    }
}

enum ListRead<'a> {
    /// A scalar: the element itself, or a field of a table element.
    Value(&'a ParamValue),
    /// The list, the index or the field does not exist.
    Missing,
    /// The element is a table, and the read asked for it as a scalar.
    Table,
}

/// Find the scalar that `list[index]` holds, or its `field` when it is a table.
fn find_list_value<'a>(
    params: &'a Parameters,
    list: &str,
    index: usize,
    field: &str,
) -> ListRead<'a> {
    let Some(element) = params.list_element(list, index) else {
        return ListRead::Missing;
    };
    match (element, field.is_empty()) {
        (ListElement::Scalar(value), true) => ListRead::Value(value),
        (ListElement::Scalar(_), false) => ListRead::Missing,
        (ListElement::Table(_), true) => ListRead::Table,
        (ListElement::Table(table), false) => {
            table.value(field).map_or(ListRead::Missing, ListRead::Value)
        }
    }
}

/// The scalar that `key` names: a dotted scalar name, `list[index]` or
/// `list[index].field`. Marks what it reads as read.
pub(crate) fn scalar_at_key<'a>(params: &'a Parameters, key: &str) -> Option<&'a ParamValue> {
    let Some(bracket) = key.find('[') else {
        return params.value(key);
    };
    let list = key.get(..bracket)?;
    let after_bracket = key.get(bracket.checked_add(1)?..)?;
    let close = after_bracket.find(']')?;
    let index_text = after_bracket.get(..close)?;
    let index_is_digits =
        !index_text.is_empty() && index_text.bytes().all(|byte| byte.is_ascii_digit());
    if !index_is_digits {
        return None;
    }
    let index: usize = index_text.parse().ok()?;
    let rest = after_bracket.get(close.checked_add(1)?..)?;
    let field = if rest.is_empty() { "" } else { rest.strip_prefix('.')? };
    let field_is_valid = rest.is_empty() || !field.is_empty();
    if !field_is_valid {
        return None;
    }
    match find_list_value(params, list, index, field) {
        ListRead::Value(value) => Some(value),
        ListRead::Missing | ListRead::Table => None,
    }
}
