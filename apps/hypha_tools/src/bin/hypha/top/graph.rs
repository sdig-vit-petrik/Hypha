//! The node graph of the bus: which node publishes and which subscribes each topic,
//! the health of every node, and the feedback loops between nodes.

use hypha_node::sdk::msgs::hypha_msgs::{NodeIntrospection, NodeState};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

/// A node whose `~/state` was silent this long is shown as silent. The state goes out
/// every second.
pub(crate) const STATE_SILENCE: Duration = Duration::from_secs(3);

/// The health of a node, worst last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Health {
    /// Every required input is usable.
    Ok,
    /// The node publishes no `~/state`.
    NoState,
    /// A required input never arrived.
    Waiting,
    /// `~/state` stopped arriving.
    Silent,
    /// A required input is stale.
    Degraded,
    /// The process is gone.
    Dead,
}

impl Health {
    /// The health of one node. `alive` is `None` for a node the registry no longer lists.
    pub(crate) fn of(
        alive: Option<bool>,
        state: Option<&(NodeState, Instant)>,
        now: Instant,
    ) -> Self {
        if alive != Some(true) {
            return Self::Dead;
        }
        let Some((state, received)) = state else {
            return Self::NoState;
        };
        let silent = now.saturating_duration_since(*received) > STATE_SILENCE;
        if silent {
            return Self::Silent;
        }
        match state.state {
            NodeState::INPUTS_USABLE => Self::Ok,
            NodeState::WAITING => Self::Waiting,
            _ => Self::Degraded,
        }
    }

    /// The short word shown for this health.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::NoState => "no state",
            Self::Waiting => "waiting",
            Self::Silent => "silent",
            Self::Degraded => "degraded",
            Self::Dead => "dead",
        }
    }

    /// The one-character marker shown before a node name.
    pub(crate) fn marker(self) -> char {
        match self {
            Self::Ok => '\u{25cf}',
            Self::NoState => '?',
            Self::Waiting => '\u{25b2}',
            Self::Silent => '\u{25cc}',
            Self::Degraded => '\u{2716}',
            Self::Dead => '\u{25cb}',
        }
    }
}

/// How bad the loop is that an edge or a vertex belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub(crate) enum LoopLevel {
    /// In no loop.
    #[default]
    None,
    /// In a loop that a rate cycle bounds.
    Feedback,
    /// In a loop of event-driven units only.
    Triggered,
}

/// Which vertices the graph has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum View {
    /// Nodes only; an edge stands for every topic from one node to another.
    #[default]
    Nodes,
    /// Nodes and topics, as `rqt_graph` draws them.
    Topics,
}

/// What a vertex stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VertexKind {
    /// A node, with its health and whether it published a manifest.
    Node { health: Health, manifest: bool },
    /// A topic, with its publisher and subscriber counts.
    Topic { publishers: usize, subscribers: usize },
}

/// One drawn vertex.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Vertex {
    pub name: String,
    pub kind: VertexKind,
    pub level: LoopLevel,
    /// A node that subscribes a topic it publishes itself (nodes view only).
    pub self_loop: bool,
}

impl Vertex {
    /// A key that stays the same while the graph is rebuilt.
    pub(crate) fn key(&self) -> String {
        match self.kind {
            VertexKind::Node { .. } => format!("n:{}", self.name),
            VertexKind::Topic { .. } => format!("t:{}", self.name),
        }
    }
}

/// One drawn edge between two different vertices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Edge {
    pub from: usize,
    pub to: usize,
    /// The topics the edge carries (nodes view); empty in the topics view.
    pub label: String,
    pub level: LoopLevel,
}

/// One loop between nodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Loop {
    /// The member node names, sorted.
    pub nodes: Vec<String>,
    pub level: LoopLevel,
}

/// The graph to draw.
#[derive(Debug, Clone, Default)]
pub(crate) struct Graph {
    pub vertices: Vec<Vertex>,
    pub edges: Vec<Edge>,
    /// Every loop, including the ones the filter hides.
    pub loops: Vec<Loop>,
}

/// What the graph is built from, per node.
pub(crate) struct NodeFacts<'a> {
    pub name: &'a str,
    pub health: Health,
    pub manifest: Option<&'a NodeIntrospection>,
}

/// How the graph is built.
pub(crate) struct Options<'a> {
    pub view: View,
    /// Keep `/clock`, `/hypha/...` and the `~/state` and `~/introspection` topics.
    pub infrastructure: bool,
    /// Keep only vertices whose name contains this, and their neighbours.
    pub filter: &'a str,
}

/// The unit of a node that reads or writes a topic: index 0 is the node's callbacks,
/// index `1 + i` its cycle `i`.
type UnitList = Vec<usize>;

/// The topics one node reads and writes, after `~/` and wildcards are resolved.
struct NodeEnds {
    publishes: Vec<(String, UnitList)>,
    subscribes: Vec<(String, UnitList)>,
    /// Per unit: true when it runs on events rather than on a period.
    event_driven: Vec<bool>,
}

/// The publishers and subscribers of one topic, as (node index, units).
#[derive(Default)]
struct TopicEnds {
    publishers: Vec<(usize, UnitList)>,
    subscribers: Vec<(usize, UnitList)>,
}

/// One topic carried from one node to another; `from == to` for a self loop.
struct NodeLink {
    from: usize,
    to: usize,
    topic: String,
    level: LoopLevel,
}

/// The unit index of the node's callbacks.
const CALLBACKS: usize = 0;

/// Builds the graph of `nodes`. `bus_topics` are the topics seen on the bus; a
/// wildcard subscription is drawn against the ones it matches.
pub(crate) fn build<'t>(
    nodes: &[NodeFacts<'_>],
    bus_topics: impl Iterator<Item = &'t str>,
    options: &Options<'_>,
) -> Graph {
    let names: BTreeSet<&str> = nodes.iter().map(|node| node.name).collect();
    let mut universe: BTreeSet<String> = bus_topics.map(str::to_string).collect();
    for node in nodes {
        let Some(manifest) = node.manifest else { continue };
        for row in &manifest.publishers {
            universe.insert(resolve(node.name, &row.topic));
        }
    }

    let mut topics: BTreeMap<String, TopicEnds> = BTreeMap::new();
    let mut event_driven: Vec<Vec<bool>> = Vec::with_capacity(nodes.len());
    for (index, node) in nodes.iter().enumerate() {
        let ends = node_ends(node, &universe);
        for (topic, units) in ends.publishes {
            let hidden = !options.infrastructure && is_infrastructure(&topic, &names);
            if !hidden {
                topics.entry(topic).or_default().publishers.push((index, units));
            }
        }
        for (topic, units) in ends.subscribes {
            let hidden = !options.infrastructure && is_infrastructure(&topic, &names);
            if !hidden {
                topics.entry(topic).or_default().subscribers.push((index, units));
            }
        }
        event_driven.push(ends.event_driven);
    }

    let links = grade_links(nodes.len(), &topics, &event_driven);
    let loops = collect_loops(nodes, &links);
    let graph = match options.view {
        View::Nodes => nodes_view(nodes, &links),
        View::Topics => topics_view(nodes, &topics, &links),
    };
    let mut filtered = keep_matching(graph, options.filter);
    filtered.loops = loops;
    filtered
}

/// `~/x` of node `n` is `/n/x`; any other name is returned as written.
fn resolve(node: &str, topic: &str) -> String {
    match topic.strip_prefix("~/") {
        Some(rest) => format!("/{node}/{rest}"),
        None => topic.to_string(),
    }
}

/// Framework plumbing rather than application data flow.
fn is_infrastructure(topic: &str, nodes: &BTreeSet<&str>) -> bool {
    if topic == "/clock" || topic.starts_with("/hypha/") {
        return true;
    }
    let Some(rest) = topic.strip_prefix('/') else { return false };
    let owner = rest.strip_suffix("/state").or_else(|| rest.strip_suffix("/introspection"));
    owner.is_some_and(|name| nodes.contains(name))
}

/// The topics of one node with the units that write and read them.
fn node_ends(node: &NodeFacts<'_>, universe: &BTreeSet<String>) -> NodeEnds {
    let mut ends =
        NodeEnds { publishes: Vec::new(), subscribes: Vec::new(), event_driven: vec![true] };
    let Some(manifest) = node.manifest else { return ends };

    for cycle in &manifest.cycles {
        let periodic = cycle.has_period && cycle.triggers.is_empty();
        ends.event_driven.push(!periodic);
    }
    let unit_count = ends.event_driven.len();
    let cycle_unit = |name: &str| -> Option<usize> {
        let position = manifest.cycles.iter().position(|cycle| cycle.name == name)?;
        position.checked_add(1)
    };

    for row in &manifest.publishers {
        let owner = if row.output_cycle.is_empty() { None } else { cycle_unit(&row.output_cycle) };
        let units: UnitList = match owner {
            Some(unit) => vec![unit],
            None => (0..unit_count).collect(),
        };
        ends.publishes.push((resolve(node.name, &row.topic), units));
    }

    for row in &manifest.subscribers {
        let mut units: UnitList = Vec::new();
        for binding in &manifest.input_bindings {
            if binding.subscriber_id != row.id {
                continue;
            }
            if let Some(unit) = cycle_unit(&binding.cycle) {
                units.push(unit);
            }
        }
        let pattern = resolve(node.name, &row.topic);
        for (position, cycle) in manifest.cycles.iter().enumerate() {
            let triggers_here =
                cycle.triggers.iter().any(|trigger| resolve(node.name, trigger) == pattern);
            if triggers_here {
                units.push(position.saturating_add(1));
            }
        }
        if units.is_empty() {
            units.push(CALLBACKS);
        }
        units.sort_unstable();
        units.dedup();
        for topic in expand(&pattern, universe) {
            ends.subscribes.push((topic, units.clone()));
        }
    }
    ends
}

/// The topics a subscription reads: the pattern's matches, or the pattern itself when
/// nothing matches (yet).
fn expand(pattern: &str, universe: &BTreeSet<String>) -> Vec<String> {
    if !pattern.contains('*') {
        return vec![pattern.to_string()];
    }
    let matches: Vec<String> = universe
        .iter()
        .filter(|topic| hypha_node::sdk::topic_matches_pattern(pattern, topic))
        .cloned()
        .collect();
    if matches.is_empty() {
        return vec![pattern.to_string()];
    }
    matches
}

#[path = "graph_dot.rs"]
mod dot;
#[path = "graph_loops.rs"]
mod loops;

pub(crate) use dot::to_dot;
use loops::{collect_loops, grade_links};

/// Writes `value` at `index` when the index is in range.
pub(crate) fn set<T>(slice: &mut [T], index: usize, value: T) {
    if let Some(slot) = slice.get_mut(index) {
        *slot = value;
    }
}

fn node_vertex(node: &NodeFacts<'_>) -> Vertex {
    Vertex {
        name: node.name.to_string(),
        kind: VertexKind::Node { health: node.health, manifest: node.manifest.is_some() },
        level: LoopLevel::None,
        self_loop: false,
    }
}

/// Nodes only: links between the same two nodes become one edge.
fn nodes_view(nodes: &[NodeFacts<'_>], links: &[NodeLink]) -> Graph {
    let mut vertices: Vec<Vertex> = nodes.iter().map(node_vertex).collect();
    let mut merged: BTreeMap<(usize, usize), (BTreeSet<&str>, LoopLevel)> = BTreeMap::new();
    for link in links {
        if let Some(vertex) = vertices.get_mut(link.from) {
            vertex.level = vertex.level.max(link.level);
        }
        if let Some(vertex) = vertices.get_mut(link.to) {
            vertex.level = vertex.level.max(link.level);
        }
        if link.from == link.to {
            if let Some(vertex) = vertices.get_mut(link.from) {
                vertex.self_loop = true;
            }
            continue;
        }
        let entry = merged.entry((link.from, link.to)).or_default();
        entry.0.insert(link.topic.as_str());
        entry.1 = entry.1.max(link.level);
    }
    let mut edges = Vec::with_capacity(merged.len());
    for ((from, to), (topics, level)) in merged {
        let label = topics.into_iter().collect::<Vec<&str>>().join(", ");
        edges.push(Edge { from, to, label, level });
    }
    Graph { vertices, edges, loops: Vec::new() }
}

/// Nodes and topics: node → topic for a publisher, topic → node for a subscriber.
fn topics_view(
    nodes: &[NodeFacts<'_>],
    topics: &BTreeMap<String, TopicEnds>,
    links: &[NodeLink],
) -> Graph {
    let mut vertices: Vec<Vertex> = nodes.iter().map(node_vertex).collect();
    let mut edges: Vec<Edge> = Vec::new();
    for (topic, ends) in topics {
        let mut topic_level = LoopLevel::None;
        for link in links {
            if link.topic == *topic {
                topic_level = topic_level.max(link.level);
            }
        }
        let topic_index = vertices.len();
        vertices.push(Vertex {
            name: topic.clone(),
            kind: VertexKind::Topic {
                publishers: ends.publishers.len(),
                subscribers: ends.subscribers.len(),
            },
            level: topic_level,
            self_loop: false,
        });
        let mut publishers: BTreeSet<usize> = BTreeSet::new();
        for (node, _) in &ends.publishers {
            publishers.insert(*node);
        }
        let mut subscribers: BTreeSet<usize> = BTreeSet::new();
        for (node, _) in &ends.subscribers {
            subscribers.insert(*node);
        }
        for node in publishers {
            let level = worst_link(links, topic, Some(node), None);
            edges.push(Edge { from: node, to: topic_index, label: String::new(), level });
            raise(&mut vertices, node, level);
        }
        for node in subscribers {
            let level = worst_link(links, topic, None, Some(node));
            edges.push(Edge { from: topic_index, to: node, label: String::new(), level });
            raise(&mut vertices, node, level);
        }
    }
    Graph { vertices, edges, loops: Vec::new() }
}

/// The worst level of the links of `topic` from `from` (any when `None`) to `to`.
fn worst_link(
    links: &[NodeLink],
    topic: &str,
    from: Option<usize>,
    to: Option<usize>,
) -> LoopLevel {
    let mut level = LoopLevel::None;
    for link in links {
        let matches = link.topic == topic
            && from.is_none_or(|node| node == link.from)
            && to.is_none_or(|node| node == link.to);
        if matches {
            level = level.max(link.level);
        }
    }
    level
}

fn raise(vertices: &mut [Vertex], index: usize, level: LoopLevel) {
    if let Some(vertex) = vertices.get_mut(index) {
        vertex.level = vertex.level.max(level);
    }
}

/// Keeps the vertices whose name contains `filter` and their direct neighbours.
fn keep_matching(graph: Graph, filter: &str) -> Graph {
    if filter.is_empty() {
        return graph;
    }
    let mut keep: Vec<bool> =
        graph.vertices.iter().map(|vertex| vertex.name.contains(filter)).collect();
    let seeds = keep.clone();
    for edge in &graph.edges {
        if seeds.get(edge.from) == Some(&true) {
            set(&mut keep, edge.to, true);
        }
        if seeds.get(edge.to) == Some(&true) {
            set(&mut keep, edge.from, true);
        }
    }

    let mut new_index: Vec<Option<usize>> = vec![None; graph.vertices.len()];
    let mut vertices = Vec::new();
    for (old, vertex) in graph.vertices.into_iter().enumerate() {
        if keep.get(old) == Some(&true) {
            set(&mut new_index, old, Some(vertices.len()));
            vertices.push(vertex);
        }
    }
    let mut edges = Vec::new();
    for edge in graph.edges {
        let from = new_index.get(edge.from).copied().flatten();
        let to = new_index.get(edge.to).copied().flatten();
        if let (Some(from), Some(to)) = (from, to) {
            edges.push(Edge { from, to, ..edge });
        }
    }
    Graph { vertices, edges, loops: graph.loops }
}

#[cfg(test)]
#[path = "graph_tests.rs"]
mod tests;
