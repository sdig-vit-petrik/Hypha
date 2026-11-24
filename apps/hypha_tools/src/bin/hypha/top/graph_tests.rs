//! Tests of the node graph: edges, loop grading, filters and the DOT export.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use super::*;
use hypha_node::sdk::msgs::hypha_msgs::{
    IntrospectedCycle, IntrospectedEndpoint, IntrospectedInput,
};

fn endpoint(id: u64, topic: &str, output_cycle: &str) -> IntrospectedEndpoint {
    IntrospectedEndpoint {
        id,
        topic: topic.to_string(),
        output_cycle: output_cycle.to_string(),
        ..IntrospectedEndpoint::default()
    }
}

fn rate_cycle(name: &str) -> IntrospectedCycle {
    IntrospectedCycle {
        name: name.to_string(),
        has_period: true,
        period_nanos: 10_000_000,
        ..IntrospectedCycle::default()
    }
}

fn triggered_cycle(name: &str, trigger: &str) -> IntrospectedCycle {
    IntrospectedCycle {
        name: name.to_string(),
        trigger: trigger.to_string(),
        ..IntrospectedCycle::default()
    }
}

fn binding(cycle: &str, subscriber_id: u64) -> IntrospectedInput {
    IntrospectedInput { cycle: cycle.to_string(), subscriber_id, ..IntrospectedInput::default() }
}

/// A node with one cycle that reads `input` and writes `output`.
fn one_cycle(name: &str, input: &str, output: &str, cycle: IntrospectedCycle) -> NodeIntrospection {
    NodeIntrospection {
        node: name.to_string(),
        subscribers: vec![endpoint(1, input, "")],
        publishers: vec![endpoint(2, output, &cycle.name)],
        input_bindings: vec![binding(&cycle.name, 1)],
        cycles: vec![cycle],
        ..NodeIntrospection::default()
    }
}

fn facts(manifests: &[NodeIntrospection]) -> Vec<NodeFacts<'_>> {
    manifests
        .iter()
        .map(|manifest| NodeFacts {
            name: manifest.node.as_str(),
            health: Health::Ok,
            manifest: Some(manifest),
        })
        .collect()
}

fn options(view: View) -> Options<'static> {
    Options { view, infrastructure: false, filter: "" }
}

#[test]
fn a_chain_has_edges_and_no_loop() {
    let manifests = [
        one_cycle("a", "/in", "/x", rate_cycle("run")),
        one_cycle("b", "/x", "/out", rate_cycle("run")),
    ];
    let graph = build(&facts(&manifests), std::iter::empty(), &options(View::Nodes));
    assert_eq!(graph.edges.len(), 1);
    assert_eq!((graph.edges[0].from, graph.edges[0].to), (0, 1));
    assert_eq!(graph.edges[0].label, "/x");
    assert!(graph.loops.is_empty(), "{:?}", graph.loops);
    assert!(graph.vertices.iter().all(|vertex| vertex.level == LoopLevel::None));
}

#[test]
fn a_loop_through_a_rate_cycle_is_feedback() {
    let manifests = [
        one_cycle("controller", "/state", "/cmd", rate_cycle("control")),
        one_cycle("plant", "/cmd", "/state", triggered_cycle("step", "/cmd")),
    ];
    let graph = build(&facts(&manifests), std::iter::empty(), &options(View::Nodes));
    assert_eq!(graph.loops.len(), 1);
    assert_eq!(graph.loops[0].nodes, ["controller", "plant"]);
    assert_eq!(graph.loops[0].level, LoopLevel::Feedback);
    assert!(graph.edges.iter().all(|edge| edge.level == LoopLevel::Feedback));
}

#[test]
fn a_loop_of_triggered_cycles_is_triggered() {
    let manifests = [
        one_cycle("ping", "/pong", "/ping", triggered_cycle("on_pong", "/pong")),
        one_cycle("pong", "/ping", "/pong", triggered_cycle("on_ping", "/ping")),
        one_cycle("logger", "/ping", "/log", rate_cycle("flush")),
    ];
    let graph = build(&facts(&manifests), std::iter::empty(), &options(View::Topics));
    assert_eq!(graph.loops.len(), 1);
    assert_eq!(graph.loops[0].level, LoopLevel::Triggered);
    let logger = graph.vertices.iter().find(|vertex| vertex.name == "logger").unwrap();
    assert_eq!(logger.level, LoopLevel::None, "a reader outside the loop is not in it");
    let ping_topic = graph.vertices.iter().find(|vertex| vertex.name == "/ping").unwrap();
    assert_eq!(ping_topic.level, LoopLevel::Triggered);
}

#[test]
fn a_callback_publisher_is_fed_by_every_unit_of_its_node() {
    // b publishes /y from a plain callback, not from its cycle. The loop is still
    // found, and the rate cycle of b that reads /x bounds it.
    let mut b = one_cycle("b", "/x", "/y", rate_cycle("run"));
    b.publishers[0].output_cycle.clear();
    let manifests = [one_cycle("a", "/y", "/x", triggered_cycle("t", "/y")), b];
    let graph = build(&facts(&manifests), std::iter::empty(), &options(View::Nodes));
    assert_eq!(graph.loops.len(), 1);
    assert_eq!(graph.loops[0].level, LoopLevel::Feedback);
}

#[test]
fn a_node_reading_its_own_output_is_a_self_loop() {
    let manifests = [one_cycle("echo", "/x", "/x", triggered_cycle("t", "/x"))];
    let graph = build(&facts(&manifests), std::iter::empty(), &options(View::Nodes));
    assert!(graph.vertices[0].self_loop);
    assert!(graph.edges.is_empty(), "{:?}", graph.edges);
    assert_eq!(graph.loops[0].level, LoopLevel::Triggered);
}

#[test]
fn a_wildcard_subscription_reads_every_matching_topic() {
    let mut reader = one_cycle("reader", "/sensors/*", "/fused", rate_cycle("run"));
    reader.subscribers[0].topic = "/sensors/*".to_string();
    let manifests = [
        one_cycle("imu", "/none", "/sensors/imu", rate_cycle("run")),
        one_cycle("gps", "/none", "/sensors/gps", rate_cycle("run")),
        reader,
    ];
    let graph = build(&facts(&manifests), std::iter::empty(), &options(View::Nodes));
    let into_reader = graph.edges.iter().filter(|edge| edge.to == 2).count();
    assert_eq!(into_reader, 2);
}

#[test]
fn infrastructure_topics_are_hidden_unless_asked_for() {
    let manifests = [one_cycle("a", "/clock", "~/state", rate_cycle("run"))];
    let hidden = build(&facts(&manifests), std::iter::empty(), &options(View::Topics));
    assert_eq!(hidden.vertices.len(), 1);
    let shown_options = Options { view: View::Topics, infrastructure: true, filter: "" };
    let shown = build(&facts(&manifests), std::iter::empty(), &shown_options);
    let names: Vec<&str> = shown.vertices.iter().map(|vertex| vertex.name.as_str()).collect();
    assert_eq!(names, ["a", "/a/state", "/clock"]);
}

#[test]
fn the_filter_keeps_matches_and_their_neighbours() {
    let manifests = [
        one_cycle("a", "/in", "/x", rate_cycle("run")),
        one_cycle("b", "/x", "/y", rate_cycle("run")),
        one_cycle("c", "/y", "/z", rate_cycle("run")),
    ];
    let filter = Options { view: View::Nodes, infrastructure: false, filter: "a" };
    let graph = build(&facts(&manifests), std::iter::empty(), &filter);
    let names: Vec<&str> = graph.vertices.iter().map(|vertex| vertex.name.as_str()).collect();
    assert_eq!(names, ["a", "b"]);
    assert_eq!(graph.edges.len(), 1);
}

#[test]
fn health_follows_liveness_and_state() {
    let now = Instant::now();
    let usable = (NodeState { state: NodeState::INPUTS_USABLE, ..NodeState::default() }, now);
    assert_eq!(Health::of(Some(false), Some(&usable), now), Health::Dead);
    assert_eq!(Health::of(None, None, now), Health::Dead);
    assert_eq!(Health::of(Some(true), None, now), Health::NoState);
    assert_eq!(Health::of(Some(true), Some(&usable), now), Health::Ok);
    let later = now.checked_add(Duration::from_secs(10)).unwrap();
    assert_eq!(Health::of(Some(true), Some(&usable), later), Health::Silent);
    let waiting = (NodeState { state: NodeState::WAITING, ..NodeState::default() }, now);
    assert_eq!(Health::of(Some(true), Some(&waiting), now), Health::Waiting);
}

#[test]
fn the_dot_export_names_vertices_and_loops() {
    let manifests = [
        one_cycle("ping", "/pong", "/ping", triggered_cycle("a", "/pong")),
        one_cycle("pong", "/ping", "/pong", triggered_cycle("b", "/ping")),
    ];
    let graph = build(&facts(&manifests), std::iter::empty(), &options(View::Nodes));
    let dot = to_dot(&graph);
    assert!(dot.starts_with("digraph hypha {"));
    assert!(dot.contains("label=\"ping\\nok\""));
    assert!(dot.contains("v0 -> v1 [color=\"orange\", label=\"/ping\"]"));
    assert!(dot.contains("// triggered loop: ping, pong"));
}
