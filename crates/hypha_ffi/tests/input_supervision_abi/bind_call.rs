//! The bind call of input supervision checks the port, reads its limits and attaches them.

use super::*;
use crate::common::supervision::bind;

/// A node whose parameter file holds one input table `[ports.inputs.speed]`.
fn node_with_speed_table(label: &str, table: &str) -> (SimulatedNode, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(unique("bind"));
    std::fs::create_dir_all(&dir).expect("create fixture directory");
    let params = dir.join("params.toml");
    let text = format!("[ports.inputs.speed]\n{table}\n");
    std::fs::write(&params, text).expect("write parameter fixture");
    (SimulatedNode::with_parameters(label, 1_000_000_000, Some(&params)), params)
}

#[test]
fn an_unnamed_port_with_a_declared_check_is_refused_with_its_reason() {
    let (sim, _params) = node_with_speed_table("bind_unnamed", "topic = \"/speed\"");
    let waitset = unsafe { create_waitset(sim.node) };
    let (_pub, sub) = unsafe { create_pubsub(sim.node, &format!("/{}", unique("bind_unnamed"))) };
    let mut source = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_input_source_create(waitset, sub, &raw mut source) },
        HyphaStatus::Ok
    );
    let context = Box::new(Context::default());
    let cycle = unsafe {
        create_supervised_cycle(waitset, "bind_unnamed", (&raw const *context).cast_mut().cast())
    };
    let options = bind_options();
    let mut input_index = u32::MAX;
    assert_eq!(
        unsafe { hypha_cycle_bind_input(cycle, source, &raw const options, &raw mut input_index) },
        HyphaStatus::Ok
    );

    let refused = unsafe { bind(cycle, sim.node, 0, "", (true, true, true)) };
    assert_eq!(refused.status, HyphaStatus::InvalidArgument);
    assert_eq!(refused.detail, "supervision needs a named port");
    assert_eq!(refused.attached, 0);
    // A wired port without a table, advisory or not, reads no limit: it needs the tag.
    let untagged = unsafe { bind(cycle, sim.node, 0, "", (true, false, true)) };
    assert_eq!(untagged.status, HyphaStatus::InvalidArgument);
    assert_eq!(
        untagged.detail,
        "an input without a table has no time limit and needs the tag hypha::unsupervised"
    );
    let tagged = unsafe { bind(cycle, sim.node, 0, "", (false, false, true)) };
    assert_eq!((tagged.status, tagged.attached), (HyphaStatus::Ok, 0));
}

#[test]
fn a_named_port_with_a_declared_silence_check_attaches_its_limit() {
    let (sim, _params) =
        node_with_speed_table("bind_named", "topic = \"/speed\"\nsilence_ms = 250");
    let waitset = unsafe { create_waitset(sim.node) };
    let (_pub, sub) = unsafe { create_pubsub(sim.node, &format!("/{}", unique("bind_named"))) };
    let mut source = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_input_source_create(waitset, sub, &raw mut source) },
        HyphaStatus::Ok
    );
    let context = Box::new(Context::default());
    let cycle = unsafe {
        create_supervised_cycle(waitset, "bind_named", (&raw const *context).cast_mut().cast())
    };
    let options = bind_options();
    let mut input_index = u32::MAX;
    assert_eq!(
        unsafe { hypha_cycle_bind_input(cycle, source, &raw const options, &raw mut input_index) },
        HyphaStatus::Ok
    );

    let bound = unsafe { bind(cycle, sim.node, 0, "speed", (true, false, true)) };
    assert_eq!(bound.status, HyphaStatus::Ok, "detail: {}", bound.detail);
    assert_eq!(bound.attached, 1);
    assert_eq!(bound.ignored, 0);
}

#[test]
fn a_named_unsupervised_port_without_silence_ms_attaches_nothing() {
    let (sim, _params) = node_with_speed_table("bind_none", "topic = \"/speed\"");
    let waitset = unsafe { create_waitset(sim.node) };
    let (_pub, sub) = unsafe { create_pubsub(sim.node, &format!("/{}", unique("bind_none"))) };
    let mut source = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_input_source_create(waitset, sub, &raw mut source) },
        HyphaStatus::Ok
    );
    let context = Box::new(Context::default());
    let cycle = unsafe {
        create_supervised_cycle(waitset, "bind_none", (&raw const *context).cast_mut().cast())
    };
    let options = bind_options();
    let mut input_index = u32::MAX;
    assert_eq!(
        unsafe { hypha_cycle_bind_input(cycle, source, &raw const options, &raw mut input_index) },
        HyphaStatus::Ok
    );

    // Not requiring the limit is the tag unsupervised.
    let bound = unsafe { bind(cycle, sim.node, 0, "speed", (false, false, true)) };
    assert_eq!(bound.status, HyphaStatus::Ok, "detail: {}", bound.detail);
    assert_eq!(bound.attached, 0);
}

#[test]
fn an_undeclared_port_with_silence_ms_in_its_table_is_supervised() {
    let table = "topic = \"/speed\"\nsilence_ms = 250";
    let (sim, _params) = node_with_speed_table("bind_optional", table);
    let waitset = unsafe { create_waitset(sim.node) };
    let (_pub, sub) = unsafe { create_pubsub(sim.node, &format!("/{}", unique("bind_opt"))) };
    let mut source = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_input_source_create(waitset, sub, &raw mut source) },
        HyphaStatus::Ok
    );
    let context = Box::new(Context::default());
    let cycle = unsafe {
        create_supervised_cycle(waitset, "bind_optional", (&raw const *context).cast_mut().cast())
    };
    let options = bind_options();
    let mut input_index = u32::MAX;
    assert_eq!(
        unsafe { hypha_cycle_bind_input(cycle, source, &raw const options, &raw mut input_index) },
        HyphaStatus::Ok
    );

    let bound = unsafe { bind(cycle, sim.node, 0, "speed", (true, false, true)) };
    assert_eq!(bound.status, HyphaStatus::Ok, "detail: {}", bound.detail);
    assert_eq!(bound.attached, 1, "a port with a silence limit is supervised");
}

#[test]
fn a_declared_silence_check_without_its_limit_key_is_refused_naming_the_key() {
    let (sim, _params) = node_with_speed_table("bind_missing", "topic = \"/speed\"");
    let waitset = unsafe { create_waitset(sim.node) };
    let (_pub, sub) = unsafe { create_pubsub(sim.node, &format!("/{}", unique("bind_missing"))) };
    let mut source = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_input_source_create(waitset, sub, &raw mut source) },
        HyphaStatus::Ok
    );
    let context = Box::new(Context::default());
    let cycle = unsafe {
        create_supervised_cycle(waitset, "bind_missing", (&raw const *context).cast_mut().cast())
    };
    let options = bind_options();
    let mut input_index = u32::MAX;
    assert_eq!(
        unsafe { hypha_cycle_bind_input(cycle, source, &raw const options, &raw mut input_index) },
        HyphaStatus::Ok
    );

    let refused = unsafe { bind(cycle, sim.node, 0, "speed", (true, false, true)) };
    assert_ne!(refused.status, HyphaStatus::Ok);
    assert!(refused.detail.contains("silence_ms"), "detail: {}", refused.detail);
    assert_eq!(refused.attached, 0);
}
