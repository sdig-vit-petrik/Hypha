//! A parameter value prints as TOML writes it.

use hypha_core::parameters::ParamValue;

#[test]
fn a_whole_float_keeps_its_decimal_point() {
    assert_eq!(ParamValue::Float(1.0).to_string(), "1.0");
    assert_eq!(ParamValue::Float(-2.0).to_string(), "-2.0");
    assert_eq!(ParamValue::Int(1).to_string(), "1");
}

#[test]
fn a_fractional_float_prints_its_shortest_form() {
    assert_eq!(ParamValue::Float(0.5).to_string(), "0.5");
    assert_eq!(ParamValue::Float(45.5).to_string(), "45.5");
}

#[test]
fn a_very_large_or_small_float_prints_with_an_exponent() {
    assert_eq!(ParamValue::Float(1e300).to_string(), "1e300");
    assert_eq!(ParamValue::Float(1e-300).to_string(), "1e-300");
}

#[test]
fn a_special_float_prints_as_toml_spells_it() {
    assert_eq!(ParamValue::Float(f64::NAN).to_string(), "nan");
    assert_eq!(ParamValue::Float(f64::INFINITY).to_string(), "inf");
    assert_eq!(ParamValue::Float(f64::NEG_INFINITY).to_string(), "-inf");
}

#[test]
fn text_and_bool_print_as_toml_values() {
    assert_eq!(ParamValue::Text("safe".to_string()).to_string(), "\"safe\"");
    assert_eq!(ParamValue::Bool(true).to_string(), "true");
}
