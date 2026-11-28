//! The port-table keys through the C ABI.

use super::*;

const KEYS_PARAMETERS: &str = r#"[ports.inputs.timed]
topic = "/timed"
silence_ms = 250

[ports.inputs.stalled]
topic = "/stalled"
stagnation_ms = 500

[ports.inputs.colour_port]
topic = "/colour"
colour = "red"

[ports.inputs.unlimited]
topic = "/unlimited"
"#;

const ALL_SILENCE_ONLY: Check = Check { stagnation: false, ..ALL };

#[test]
fn an_unknown_key_is_reported_by_name() {
    let node = Node::with_parameters(KEYS_PARAMETERS);
    let outcome = node.supervision(HYPHA_PORT_INPUT, "colour_port", ALL, 128);
    assert_eq!(outcome.status, HyphaStatus::InvalidArgument);
    assert_eq!(outcome.detail, "unknown key 'colour'");
}

#[test]
fn a_too_small_buffer_for_a_detail_reports_the_length() {
    let node = Node::with_parameters(KEYS_PARAMETERS);
    let outcome = node.supervision(HYPHA_PORT_INPUT, "colour_port", ALL, 3);
    assert_eq!((outcome.status, outcome.length), (HyphaStatus::TooLarge, 20));
}

#[test]
fn a_missing_silence_ms_is_not_available() {
    let node = Node::with_parameters(KEYS_PARAMETERS);
    let outcome = node.supervision(HYPHA_PORT_INPUT, "unlimited", ALL_SILENCE_ONLY, 128);
    assert_eq!(outcome.status, HyphaStatus::NotAvailable);
    assert_eq!(
        outcome.detail,
        "missing key 'silence_ms': an input needs a limit, 0 for none, or the tag hypha::unsupervised"
    );
}

#[test]
fn a_declared_limit_is_returned_in_nanoseconds() {
    let node = Node::with_parameters(KEYS_PARAMETERS);
    let outcome = node.supervision(HYPHA_PORT_INPUT, "timed", ALL_SILENCE_ONLY, 128);
    assert_eq!(outcome.status, HyphaStatus::Ok);
    assert_eq!(
        outcome.limits,
        HyphaSupervisionLimits { silence_nanos: 250 * MILLI, stagnation_nanos: 0 }
    );
    let outcome = node.supervision(HYPHA_PORT_INPUT, "stalled", ALL, 128);
    assert_eq!(outcome.status, HyphaStatus::NotAvailable);
    let stagnation_only = Check { silence: false, ..ALL };
    let outcome = node.supervision(HYPHA_PORT_INPUT, "stalled", stagnation_only, 128);
    assert_eq!(
        outcome.limits,
        HyphaSupervisionLimits { silence_nanos: 0, stagnation_nanos: 500 * MILLI }
    );
}

/// Call with every output filled with a value that a correct call must overwrite.
fn call_over_garbage(
    node: &Node,
    direction: u32,
    name: &str,
) -> (HyphaStatus, HyphaSupervisionLimits, u8, usize) {
    let outcome = node.supervision(direction, name, ALL, 128);
    (outcome.status, outcome.limits, outcome.ignored, outcome.length)
}

#[test]
fn an_empty_name_leaves_the_outputs_defined() {
    let node = Node::with_parameters(KEYS_PARAMETERS);
    let (status, limits, ignored, length) = call_over_garbage(&node, HYPHA_PORT_INPUT, "");
    assert_eq!(status, HyphaStatus::InvalidArgument);
    // The refusal names its reason, so the detail length is reported.
    let reason = "supervision needs a named port";
    assert_eq!((limits, ignored, length), (HyphaSupervisionLimits::default(), 0, reason.len()));
}

#[test]
fn a_refusal_after_the_checks_leaves_the_limits_and_flag_zero() {
    let node = Node::with_parameters(KEYS_PARAMETERS);
    let (status, limits, ignored, length) =
        call_over_garbage(&node, HYPHA_PORT_INPUT, "colour_port");
    assert_eq!(status, HyphaStatus::InvalidArgument);
    assert_eq!((limits, ignored, length), (HyphaSupervisionLimits::default(), 0, 20));
    let (status, limits, ignored, length) = call_over_garbage(&node, HYPHA_PORT_INPUT, "unlimited");
    assert_eq!(status, HyphaStatus::NotAvailable);
    assert_eq!((limits, ignored), (HyphaSupervisionLimits::default(), 0));
    assert!(length > 0);
}
