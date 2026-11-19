//! Loop detection: strongly connected components of the node graph and of the graph
//! of units (callbacks and cycles), and the grading of every node-to-node link.

use super::set;
use super::{Loop, LoopLevel, NodeFacts, NodeLink, TopicEnds, UnitList};
use std::collections::BTreeMap;

/// Every node-to-node link, graded by the loop it is in.
pub(super) fn grade_links(
    node_count: usize,
    topics: &BTreeMap<String, TopicEnds>,
    event_driven: &[Vec<bool>],
) -> Vec<NodeLink> {
    let mut node_adjacency: Vec<Vec<usize>> = vec![Vec::new(); node_count];
    for ends in topics.values() {
        for (from, _) in &ends.publishers {
            for (to, _) in &ends.subscribers {
                if let Some(list) = node_adjacency.get_mut(*from) {
                    list.push(*to);
                }
            }
        }
    }
    let node_loops = LoopSets::of(&node_adjacency);

    // One unit per (node, callbacks or cycle).
    let mut unit_offset: Vec<usize> = Vec::with_capacity(node_count);
    let mut unit_event_driven: Vec<bool> = Vec::new();
    for units in event_driven {
        unit_offset.push(unit_event_driven.len());
        unit_event_driven.extend_from_slice(units);
    }
    let global = |node: usize, unit: usize| -> Option<usize> {
        unit_offset.get(node).and_then(|offset| offset.checked_add(unit))
    };
    let mut unit_adjacency: Vec<Vec<usize>> = vec![Vec::new(); unit_event_driven.len()];
    for ends in topics.values() {
        for_each_unit_pair(ends, &global, |from, to| {
            if let Some(list) = unit_adjacency.get_mut(from) {
                list.push(to);
            }
        });
    }
    let unit_loops = LoopSets::of(&unit_adjacency);
    let mut triggered_component: Vec<bool> = vec![true; unit_loops.component_count];
    for (unit, component) in unit_loops.component.iter().enumerate() {
        let periodic = unit_event_driven.get(unit) == Some(&false);
        if periodic {
            set(&mut triggered_component, *component, false);
        }
    }

    let mut links = Vec::new();
    for (topic, ends) in topics {
        for (from, from_units) in &ends.publishers {
            for (to, to_units) in &ends.subscribers {
                let mut level = LoopLevel::None;
                if node_loops.same_loop(*from, *to) {
                    level = LoopLevel::Feedback;
                }
                let pair = UnitPair { from: (*from, from_units), to: (*to, to_units) };
                let triggered = pair.in_triggered_loop(&global, &unit_loops, &triggered_component);
                if triggered {
                    level = LoopLevel::Triggered;
                }
                links.push(NodeLink { from: *from, to: *to, topic: topic.clone(), level });
            }
        }
    }
    links
}

/// The units on both ends of one topic between two nodes.
struct UnitPair<'a> {
    from: (usize, &'a UnitList),
    to: (usize, &'a UnitList),
}

impl UnitPair<'_> {
    /// True when a writing and a reading unit sit in the same loop of event-driven units.
    fn in_triggered_loop(
        &self,
        global: &impl Fn(usize, usize) -> Option<usize>,
        loops: &LoopSets,
        triggered_component: &[bool],
    ) -> bool {
        for from_unit in self.from.1 {
            for to_unit in self.to.1 {
                let (Some(a), Some(b)) =
                    (global(self.from.0, *from_unit), global(self.to.0, *to_unit))
                else {
                    continue;
                };
                let component = loops.component.get(a).copied();
                let all_event_driven =
                    component.is_some_and(|c| triggered_component.get(c) == Some(&true));
                let closes_loop = loops.same_loop(a, b) && all_event_driven;
                if closes_loop {
                    return true;
                }
            }
        }
        false
    }
}

/// Calls `edge` for every (writing unit, reading unit) pair of one topic.
fn for_each_unit_pair(
    ends: &TopicEnds,
    global: &impl Fn(usize, usize) -> Option<usize>,
    mut edge: impl FnMut(usize, usize),
) {
    for (from, from_units) in &ends.publishers {
        for (to, to_units) in &ends.subscribers {
            for from_unit in from_units {
                for to_unit in to_units {
                    if let (Some(a), Some(b)) = (global(*from, *from_unit), global(*to, *to_unit)) {
                        edge(a, b);
                    }
                }
            }
        }
    }
}

/// The strongly connected components of a directed graph, and which of them are loops.
pub(super) struct LoopSets {
    /// The component of every vertex.
    pub component: Vec<usize>,
    pub component_count: usize,
    /// Per component: more than one vertex, or one vertex with an edge to itself.
    pub is_loop: Vec<bool>,
}

impl LoopSets {
    pub(crate) fn of(adjacency: &[Vec<usize>]) -> Self {
        let (component, component_count) = strongly_connected(adjacency);
        let mut size: Vec<usize> = vec![0; component_count];
        for id in &component {
            if let Some(slot) = size.get_mut(*id) {
                *slot = slot.saturating_add(1);
            }
        }
        let mut is_loop: Vec<bool> = size.iter().map(|count| *count > 1).collect();
        for (vertex, list) in adjacency.iter().enumerate() {
            if list.contains(&vertex)
                && let Some(id) = component.get(vertex)
            {
                set(&mut is_loop, *id, true);
            }
        }
        Self { component, component_count, is_loop }
    }

    /// True when `a` and `b` lie in one loop.
    pub(crate) fn same_loop(&self, a: usize, b: usize) -> bool {
        let (Some(first), Some(second)) = (self.component.get(a), self.component.get(b)) else {
            return false;
        };
        first == second && self.is_loop.get(*first) == Some(&true)
    }
}

/// A vertex no component holds yet.
const UNASSIGNED: usize = usize::MAX;

/// Kosaraju's algorithm with explicit stacks: the component id of every vertex, and the
/// number of components.
fn strongly_connected(adjacency: &[Vec<usize>]) -> (Vec<usize>, usize) {
    let count = adjacency.len();
    let mut visited = vec![false; count];
    let mut finished: Vec<usize> = Vec::with_capacity(count);
    // (vertex, index of the next successor to visit)
    let mut stack: Vec<(usize, usize)> = Vec::new();
    for root in 0..count {
        if visited.get(root) == Some(&true) {
            continue;
        }
        set(&mut visited, root, true);
        stack.push((root, 0));
        while let Some(&(vertex, next)) = stack.last() {
            let successor = adjacency.get(vertex).and_then(|list| list.get(next)).copied();
            let Some(target) = successor else {
                finished.push(vertex);
                stack.pop();
                continue;
            };
            if let Some(top) = stack.last_mut() {
                top.1 = next.saturating_add(1);
            }
            if visited.get(target) == Some(&false) {
                set(&mut visited, target, true);
                stack.push((target, 0));
            }
        }
    }

    let mut reversed: Vec<Vec<usize>> = vec![Vec::new(); count];
    for (from, list) in adjacency.iter().enumerate() {
        for to in list {
            if let Some(sources) = reversed.get_mut(*to) {
                sources.push(from);
            }
        }
    }

    let mut component = vec![UNASSIGNED; count];
    let mut next_id: usize = 0;
    let mut pending: Vec<usize> = Vec::new();
    for root in finished.iter().rev() {
        if component.get(*root) != Some(&UNASSIGNED) {
            continue;
        }
        set(&mut component, *root, next_id);
        pending.push(*root);
        while let Some(vertex) = pending.pop() {
            let sources = reversed.get(vertex).map_or(&[][..], Vec::as_slice);
            for source in sources {
                if component.get(*source) == Some(&UNASSIGNED) {
                    set(&mut component, *source, next_id);
                    pending.push(*source);
                }
            }
        }
        next_id = next_id.saturating_add(1);
    }
    (component, next_id)
}

/// Every loop between nodes, graded by its worst link.
pub(super) fn collect_loops(nodes: &[NodeFacts<'_>], links: &[NodeLink]) -> Vec<Loop> {
    let mut adjacency: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    for link in links {
        if let Some(list) = adjacency.get_mut(link.from) {
            list.push(link.to);
        }
    }
    let sets = LoopSets::of(&adjacency);
    let mut loops = Vec::new();
    for (component, is_loop) in sets.is_loop.iter().enumerate() {
        if !is_loop {
            continue;
        }
        let mut members: Vec<String> = Vec::new();
        for (vertex, id) in sets.component.iter().enumerate() {
            if *id == component
                && let Some(node) = nodes.get(vertex)
            {
                members.push(node.name.to_string());
            }
        }
        members.sort();
        let mut level = LoopLevel::Feedback;
        for link in links {
            let inside = sets.component.get(link.from) == Some(&component)
                && sets.component.get(link.to) == Some(&component);
            if inside {
                level = level.max(link.level);
            }
        }
        loops.push(Loop { nodes: members, level });
    }
    loops.sort_by(|a, b| b.level.cmp(&a.level).then_with(|| a.nodes.cmp(&b.nodes)));
    loops
}
