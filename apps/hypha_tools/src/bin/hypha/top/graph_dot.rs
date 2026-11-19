//! The graph in Graphviz DOT.

use super::{Graph, Health, LoopLevel, VertexKind};
use std::fmt::Write as _;

/// The graph in Graphviz DOT, coloured as on screen.
pub(crate) fn to_dot(graph: &Graph) -> String {
    let mut out =
        String::from("digraph hypha {\n  rankdir=LR;\n  node [fontname=\"monospace\"];\n");
    for (index, vertex) in graph.vertices.iter().enumerate() {
        let name = escape(&vertex.name);
        let border = loop_colour(vertex.level).unwrap_or("black");
        match vertex.kind {
            VertexKind::Node { health, .. } => {
                let fill = health_colour(health);
                let _ = writeln!(
                    out,
                    "  v{index} [shape=box, style=\"filled\", fillcolor=\"{fill}\", color=\"{border}\", label=\"{name}\\n{}\"];",
                    health.label()
                );
            }
            VertexKind::Topic { .. } => {
                let _ = writeln!(
                    out,
                    "  v{index} [shape=ellipse, color=\"{border}\", label=\"{name}\"];"
                );
            }
        }
        if vertex.self_loop {
            let colour = loop_colour(vertex.level).unwrap_or("orange");
            let _ = writeln!(out, "  v{index} -> v{index} [color=\"{colour}\"];");
        }
    }
    for edge in &graph.edges {
        let colour = loop_colour(edge.level).unwrap_or("gray40");
        let label = escape(&edge.label);
        let _ = writeln!(
            out,
            "  v{} -> v{} [color=\"{colour}\", label=\"{label}\"];",
            edge.from, edge.to
        );
    }
    for item in &graph.loops {
        let kind = match item.level {
            LoopLevel::Triggered => "triggered",
            LoopLevel::Feedback | LoopLevel::None => "feedback",
        };
        let _ = writeln!(out, "  // {kind} loop: {}", item.nodes.join(", "));
    }
    out.push_str("}\n");
    out
}

fn escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

fn loop_colour(level: LoopLevel) -> Option<&'static str> {
    match level {
        LoopLevel::None => None,
        LoopLevel::Feedback | LoopLevel::Triggered => Some("orange"),
    }
}

fn health_colour(health: Health) -> &'static str {
    match health {
        Health::Ok => "palegreen",
        Health::NoState => "white",
        Health::Waiting => "khaki",
        Health::Silent | Health::Degraded => "lightpink",
        Health::Dead => "gray80",
    }
}
