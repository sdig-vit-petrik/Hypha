//! The player's replay message definitions match the generator's text.
#![allow(clippy::expect_used)]

use std::path::PathBuf;

use hypha_msgdef::parse::parse_message;
use hypha_msgdef::resolve::{SpecSet, definition_text, message_identity};
use hypha_msgdef::spec::TypeName;

fn spec_set() -> SpecSet {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../hypha_msgs/msg/hypha_msgs");
    let mut specs = SpecSet::new();
    let name = "ReplayProgress";
    let text = std::fs::read_to_string(root.join(format!("{name}.msg"))).expect("definition");
    let type_name = TypeName { package: "hypha_msgs".to_string(), name: name.to_string() };
    specs.insert(parse_message(type_name, &text).expect("parse"));
    specs
}

#[test]
fn the_replay_progress_definition_matches_the_generator() {
    let specs = spec_set();
    let root = TypeName { package: "hypha_msgs".to_string(), name: "ReplayProgress".to_string() };
    assert_eq!(
        definition_text(&specs, &root).expect("definition"),
        hypha_runtime::replay_progress::REPLAY_PROGRESS_DEFINITION
    );
    assert_eq!(
        message_identity(&specs, &root).expect("identity"),
        <hypha_msgs::hypha_msgs::ReplayProgress as hypha_core::message::MessageIdentity>::SCHEMA_HASH
    );
}
