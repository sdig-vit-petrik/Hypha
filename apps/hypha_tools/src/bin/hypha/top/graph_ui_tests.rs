//! Drawing tests of the Graph tab, on a `TestBackend`.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use super::*;
use hypha_node::sdk::message::EncodeInto as _;
use hypha_node::sdk::msgs::hypha_msgs::{
    IntrospectedCycle, IntrospectedEndpoint, IntrospectedInput, NodeIntrospection,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn manifest(name: &str, input: &str, output: &str) -> NodeIntrospection {
    NodeIntrospection {
        node: name.to_string(),
        subscribers: vec![IntrospectedEndpoint {
            id: 1,
            topic: input.to_string(),
            ..IntrospectedEndpoint::default()
        }],
        publishers: vec![IntrospectedEndpoint {
            id: 2,
            topic: output.to_string(),
            output_cycle: "on_input".to_string(),
            ..IntrospectedEndpoint::default()
        }],
        cycles: vec![IntrospectedCycle {
            name: "on_input".to_string(),
            triggers: vec![input.to_string()],
            ..IntrospectedCycle::default()
        }],
        input_bindings: vec![IntrospectedInput {
            cycle: "on_input".to_string(),
            subscriber_id: 1,
            ..IntrospectedInput::default()
        }],
        ..NodeIntrospection::default()
    }
}

fn receive(state: &mut AppState, manifest: &NodeIntrospection) {
    let mut buffer = vec![0_u8; manifest.encoded_len()];
    let length = manifest.encode_into(&mut buffer).unwrap();
    buffer.truncate(length);
    let hash = hypha_node::sdk::schema_hash_from_md5(NodeIntrospection::SCHEMA_HASH).unwrap();
    state.on_sample(&format!("/{}/introspection", manifest.node), hash, &buffer);
}

fn node(name: &str) -> NodeInfo {
    NodeInfo { name: name.to_string(), executable: String::new(), pid: 1, is_alive: true }
}

fn render(state: &mut AppState, nodes: &[NodeInfo]) -> String {
    let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
    terminal.draw(|f| draw_graph_tab(f, f.area(), state, nodes)).unwrap();
    let buffer = terminal.backend().buffer();
    let mut text = String::new();
    for (index, cell) in buffer.content.iter().enumerate() {
        if index % 120 == 0 {
            text.push('\n');
        }
        text.push_str(cell.symbol());
    }
    text
}

#[test]
fn a_triggered_loop_is_drawn_and_reported() {
    let mut state = AppState::new();
    receive(&mut state, &manifest("ping", "/pong", "/ping"));
    receive(&mut state, &manifest("pong", "/ping", "/pong"));
    let nodes = [node("ping"), node("pong")];
    let screen = render(&mut state, &nodes);
    assert!(screen.contains("? ping"), "{screen}");
    assert!(screen.contains("? pong"), "{screen}");
    assert!(screen.contains("[ping, pong] triggered"), "{screen}");
    assert!(screen.contains('\u{25b6}') && screen.contains('\u{25c0}'), "{screen}");
}

#[test]
fn the_topics_view_draws_topics_and_enter_opens_the_node() {
    let mut state = AppState::new();
    receive(&mut state, &manifest("source", "/in", "/data"));
    let nodes = [node("source")];
    state.active_tab = Tab::Graph;
    assert!(state.graph.handle_key(KeyCode::Char('g')));
    let screen = render(&mut state, &nodes);
    assert!(screen.contains("/data") && screen.contains("/in"), "{screen}");
    assert!(screen.contains("No loops"), "{screen}");

    // Reading order is /in, source, /data. A topic without traffic stays here.
    state.graph.handle_key(KeyCode::Home);
    state.graph.handle_key(KeyCode::Enter);
    let screen = render(&mut state, &nodes);
    assert_eq!(state.graph.selected.as_deref(), Some("t:/in"));
    assert_eq!(state.active_tab, Tab::Graph);
    assert!(screen.contains("/in has carried no traffic yet"), "{screen}");

    state.graph.handle_key(KeyCode::Down);
    state.graph.handle_key(KeyCode::Enter);
    render(&mut state, &nodes);
    assert_eq!(state.graph.selected.as_deref(), Some("n:source"));
    assert_eq!(state.active_tab, Tab::NodeRegistry);
}

#[test]
fn keys_outside_the_tab_fall_through() {
    let mut view = GraphView::default();
    assert!(!view.handle_key(KeyCode::Char('q')));
    assert!(!view.handle_key(KeyCode::Char('3')));
    assert!(view.handle_key(KeyCode::Char('i')));
    assert!(view.infrastructure);
}

#[test]
fn line_ends_join_into_box_characters() {
    assert_eq!(line_char(LEFT | RIGHT), '\u{2500}');
    assert_eq!(line_char(UP | DOWN), '\u{2502}');
    assert_eq!(line_char(LEFT | DOWN), '\u{2510}');
    assert_eq!(line_char(UP | RIGHT), '\u{2514}');
    assert_eq!(line_char(UP | DOWN | LEFT | RIGHT), '\u{253c}');
}
