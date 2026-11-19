//! The C ABI wrappers of `hypha_node::parameter_parse`.

use crate::boundary::{borrow_str, handle_ref, require_non_null, write_out};
use hypha_core::parameters::ParamValue;
use hypha_node::safe::{HyphaConfig, ParamRead, config_list_len, config_list_value, config_value};
use hypha_node::status::HyphaStatus;
use hypha_node::{
    HYPHA_CONFIG_VALUE_BOOL, HYPHA_CONFIG_VALUE_FLOAT, HYPHA_CONFIG_VALUE_INT,
    HYPHA_CONFIG_VALUE_TEXT, HyphaConfigValue, HyphaParamResult,
};
use std::os::raw::c_char;

/// Read the scalar parameter `name` of `config` into `out`, unconverted.
///
/// `Absent`: no scalar has that name. `Malformed`: `name` is a list. `InvalidArgument`:
/// a null pointer or non-UTF-8 `name`. `out` is written only on `Ok`; a text value
/// borrows `config`.
///
/// # Safety
/// `config` is live. `name` holds `name_len` bytes. `out` points to one writable
/// `HyphaConfigValue`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_config_value(
    config: *const HyphaConfig,
    name: *const c_char,
    name_len: usize,
    out: *mut HyphaConfigValue,
) -> HyphaParamResult {
    read_parameter(|| {
        let handle = unsafe { handle_ref(config) }?;
        let name = unsafe { borrow_str(name, name_len) }?;
        require_non_null(out)?;
        let value = match config_value(handle, name) {
            ParamRead::Found(value) => value,
            ParamRead::Absent => return Ok(HyphaParamResult::Absent),
            ParamRead::Malformed => return Ok(HyphaParamResult::Malformed),
        };
        unsafe { write_out(out, config_value_of(value)) }?;
        Ok(HyphaParamResult::Ok)
    })
}

/// Write the number of elements of the list `name` in `config` to `out_len`.
/// `Absent`: no list has that name. `Malformed`: a scalar. `InvalidArgument`: a null
/// pointer or non-UTF-8 `name`. `out_len` is written only on `Ok`.
///
/// # Safety
/// `config` is live. `name` holds `name_len` bytes. `out_len` points to a writable `usize`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_config_list_len(
    config: *const HyphaConfig,
    name: *const c_char,
    name_len: usize,
    out_len: *mut usize,
) -> HyphaParamResult {
    read_parameter(|| {
        let handle = unsafe { handle_ref(config) }?;
        let list = unsafe { borrow_str(name, name_len) }?;
        require_non_null(out_len)?;
        let len = match config_list_len(handle, list) {
            ParamRead::Found(len) => len,
            ParamRead::Absent => return Ok(HyphaParamResult::Absent),
            ParamRead::Malformed => return Ok(HyphaParamResult::Malformed),
        };
        unsafe { write_out(out_len, len) }?;
        Ok(HyphaParamResult::Ok)
    })
}

/// Read element `index` of the list `name` in `config`, or its `field` when the element
/// is a table (empty `field`: the element itself), into `out`. `Absent`: no such list,
/// index or field. `Malformed`: a table with an empty `field`. `InvalidArgument`: a null
/// pointer or a non-UTF-8 name. `out` is written only on `Ok`; a text value borrows `config`.
///
/// # Safety
/// `config` is live. `name` and `field` hold `name_len` and `field_len` bytes. `out`
/// points to one writable `HyphaConfigValue`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_config_list_value(
    config: *const HyphaConfig,
    name: *const c_char,
    name_len: usize,
    index: usize,
    field: *const c_char,
    field_len: usize,
    out: *mut HyphaConfigValue,
) -> HyphaParamResult {
    read_parameter(|| {
        let handle = unsafe { handle_ref(config) }?;
        let list = unsafe { borrow_str(name, name_len) }?;
        let field = unsafe { borrow_str(field, field_len) }?;
        require_non_null(out)?;
        let value = match config_list_value(handle, list, index, field) {
            ParamRead::Found(value) => value,
            ParamRead::Absent => return Ok(HyphaParamResult::Absent),
            ParamRead::Malformed => return Ok(HyphaParamResult::Malformed),
        };
        unsafe { write_out(out, config_value_of(value)) }?;
        Ok(HyphaParamResult::Ok)
    })
}

fn config_value_of(value: &ParamValue) -> HyphaConfigValue {
    match value {
        ParamValue::Bool(flag) => HyphaConfigValue {
            bool_value: u8::from(*flag),
            ..HyphaConfigValue::empty(HYPHA_CONFIG_VALUE_BOOL)
        },
        ParamValue::Int(number) => HyphaConfigValue {
            int_value: *number,
            ..HyphaConfigValue::empty(HYPHA_CONFIG_VALUE_INT)
        },
        ParamValue::Float(number) => HyphaConfigValue {
            float_value: *number,
            ..HyphaConfigValue::empty(HYPHA_CONFIG_VALUE_FLOAT)
        },
        ParamValue::Text(text) => HyphaConfigValue {
            text: text.as_ptr().cast::<c_char>(),
            text_len: text.len(),
            ..HyphaConfigValue::empty(HYPHA_CONFIG_VALUE_TEXT)
        },
    }
}

fn read_parameter<F: FnOnce() -> Result<HyphaParamResult, HyphaStatus>>(
    body: F,
) -> HyphaParamResult {
    body().unwrap_or(HyphaParamResult::InvalidArgument)
}
