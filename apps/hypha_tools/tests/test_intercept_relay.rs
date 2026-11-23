//! The bus-independent relay of one interception.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]

use hypha_node::sdk::ResolvedSchema;
use hypha_tools::intercept_doc::{InterceptionSpec, OverrideSpec, RelayState};
use hypha_tools::intercept_relay::{Forward, Relay};

const HASH_A: u64 = 0xA;
const HASH_B: u64 = 0xB;

fn schema() -> ResolvedSchema {
    ResolvedSchema {
        data_type: "test_msgs/Battery".to_owned(),
        md5: "d9d213e8c146065ec958333c617eb1a6".to_owned(),
        definition: "uint16 status\nstring serial_number\n".to_owned(),
    }
}

fn frame(status: u16, serial: &str) -> Vec<u8> {
    let mut wire = status.to_le_bytes().to_vec();
    wire.extend_from_slice(&u32::try_from(serial.len()).unwrap().to_le_bytes());
    wire.extend_from_slice(serial.as_bytes());
    wire
}

fn spec(armed: bool, overrides: &[(&str, OverrideSpec)]) -> InterceptionSpec {
    InterceptionSpec {
        name: "battery".to_owned(),
        input: "/battery/raw".to_owned(),
        output: "/battery".to_owned(),
        armed,
        overrides: overrides
            .iter()
            .map(|(path, spec)| ((*path).to_owned(), spec.clone()))
            .collect(),
    }
}

fn set_bit_one() -> OverrideSpec {
    OverrideSpec::Bits { set: "0x2".to_owned(), clear: "0".to_owned() }
}

fn live(spec: &InterceptionSpec) -> Relay {
    let mut relay = Relay::new(spec).expect("relay");
    let mut scratch = Vec::new();
    assert_eq!(relay.on_frame(HASH_A, &frame(0, "AB"), &mut scratch), Forward::Drop);
    assert_eq!(relay.pending_schema(), Some(HASH_A));
    relay.resolved(HASH_A, &schema()).expect("resolves");
    relay
}

fn published(forward: Forward<'_>) -> Vec<u8> {
    match forward {
        Forward::Publish(bytes) => bytes.to_vec(),
        Forward::Drop => panic!("dropped"),
    }
}

#[test]
fn frames_before_resolution_are_counted_not_forwarded() {
    let mut relay = Relay::new(&spec(true, &[])).expect("relay");
    let mut scratch = Vec::new();
    for _ in 0..3 {
        assert_eq!(relay.on_frame(HASH_A, &frame(1, "x"), &mut scratch), Forward::Drop);
    }
    let state = relay.snapshot();
    assert_eq!(state.state, RelayState::Resolving);
    assert_eq!((state.received, state.unresolved_dropped, state.published), (3, 3, 0));
}

#[test]
fn bypass_forwards_bytes_unchanged() {
    let mut relay = live(&spec(false, &[("status", set_bit_one())]));
    let input = frame(1, "AB");
    let mut scratch = Vec::new();
    assert_eq!(published(relay.on_frame(HASH_A, &input, &mut scratch)), input);
    assert_eq!(relay.snapshot().modified, 0);
}

#[test]
fn armed_relay_patches() {
    let mut relay = live(&spec(true, &[("status", set_bit_one())]));
    let mut scratch = Vec::new();
    let out = published(relay.on_frame(HASH_A, &frame(1, "AB"), &mut scratch));
    assert_eq!(out, frame(3, "AB"));
    relay.count_published();
    let state = relay.snapshot();
    assert_eq!(state.state, RelayState::Live);
    assert_eq!(state.data_type, "test_msgs/Battery");
    assert_eq!((state.received, state.modified, state.published), (2, 1, 1));
}

#[test]
fn schema_change_stops_the_relay() {
    let mut relay = live(&spec(true, &[("status", set_bit_one())]));
    let mut scratch = Vec::new();
    assert_eq!(relay.on_frame(HASH_B, &frame(1, "AB"), &mut scratch), Forward::Drop);
    let state = relay.snapshot();
    assert_eq!(state.state, RelayState::SchemaChanged);
    assert!(state.last_error.contains("schema changed"), "{}", state.last_error);
    assert_eq!(relay.on_frame(HASH_A, &frame(1, "AB"), &mut scratch), Forward::Drop);
}

#[test]
fn undecodable_payload_is_forwarded_and_counted() {
    let mut relay = live(&spec(true, &[("status", set_bit_one())]));
    let truncated = vec![1, 0, 9];
    let mut scratch = Vec::new();
    assert_eq!(published(relay.on_frame(HASH_A, &truncated, &mut scratch)), truncated);
    let state = relay.snapshot();
    assert_eq!(state.patch_failed, 1);
    assert!(!state.last_error.is_empty(), "the relay reports no error");
}

#[test]
fn a_failed_path_is_named_and_the_rest_applies() {
    let mut relay = live(&spec(
        true,
        &[("status", set_bit_one()), ("missing", OverrideSpec::Value { value: "1".to_owned() })],
    ));
    let mut scratch = Vec::new();
    assert_eq!(published(relay.on_frame(HASH_A, &frame(1, "AB"), &mut scratch)), frame(3, "AB"));
    let state = relay.snapshot();
    assert_eq!((state.modified, state.patch_failed), (1, 1));
    assert!(state.last_error.starts_with("missing: "), "{}", state.last_error);
}

#[test]
fn update_keeps_counters_and_schema() {
    let mut relay = live(&spec(true, &[]));
    relay.update(&spec(true, &[("status", set_bit_one())])).expect("update");
    let mut scratch = Vec::new();
    assert_eq!(published(relay.on_frame(HASH_A, &frame(0, "AB"), &mut scratch)), frame(2, "AB"));
    assert_eq!(relay.snapshot().received, 2);
}

#[test]
fn bad_override_refuses_the_spec() {
    let bad = OverrideSpec::Bits { set: "zz".to_owned(), clear: "0".to_owned() };
    assert!(Relay::new(&spec(true, &[("status", bad.clone())])).is_err());
    let mut relay = live(&spec(true, &[("status", set_bit_one())]));
    assert!(relay.update(&spec(true, &[("status", bad)])).is_err());
    let mut scratch = Vec::new();
    assert_eq!(published(relay.on_frame(HASH_A, &frame(0, "AB"), &mut scratch)), frame(2, "AB"));
}

#[test]
fn a_definition_that_does_not_parse_is_an_error_state() {
    let mut relay = Relay::new(&spec(true, &[])).expect("relay");
    let mut scratch = Vec::new();
    let _ = relay.on_frame(HASH_A, &frame(0, "AB"), &mut scratch);
    let broken = ResolvedSchema { definition: "int32[ data\n".to_owned(), ..schema() };
    assert!(relay.resolved(HASH_A, &broken).is_err());
    assert_eq!(relay.snapshot().state, RelayState::Error);
    assert_eq!(relay.on_frame(HASH_A, &frame(0, "AB"), &mut scratch), Forward::Drop);
}
