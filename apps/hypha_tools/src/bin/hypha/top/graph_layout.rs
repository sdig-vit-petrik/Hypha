//! A layered (Sugiyama-style) layout of a directed graph on a character grid, flowing
//! left to right as `rqt_graph` does.

use crate::top::graph::set;

/// The height of a vertex box: border, label, border.
pub(crate) const BOX_HEIGHT: usize = 3;
/// Rows between two items of one column.
const ROW_GAP: usize = 1;
/// Free cells on each side of the tracks of a column gap.
const GAP_MARGIN: usize = 2;
/// Barycenter sweeps (one down and one up each).
const SWEEPS: usize = 4;
/// Fixed-point scale of a barycenter.
const BARYCENTER_SCALE: usize = 1024;

/// A vertex box at its top-left corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Placed {
    pub x: usize,
    pub y: usize,
    pub width: usize,
}

/// A horizontal run through a column, where an edge crosses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Run {
    pub edge: usize,
    pub x: usize,
    pub y: usize,
    pub width: usize,
}

/// One piece of an edge across one column gap: right from `(x0, y0)` to the track,
/// along the track to `y1`, then right to `(x1, y1)`. `track` is `None` when
/// `y0 == y1` and the piece is straight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Segment {
    pub edge: usize,
    pub x0: usize,
    pub y0: usize,
    pub track: Option<usize>,
    pub x1: usize,
    pub y1: usize,
    /// The edge points left: its head is at `(x0, y0)`.
    pub head_at_start: bool,
    /// The edge points right: its head is at `(x1, y1)`.
    pub head_at_end: bool,
}

/// The placed graph.
#[derive(Debug, Clone, Default)]
pub(crate) struct Layout {
    /// One box per vertex, in vertex order.
    pub boxes: Vec<Placed>,
    pub runs: Vec<Run>,
    pub segments: Vec<Segment>,
    pub width: usize,
    pub height: usize,
}

/// One link between two items of adjacent columns.
struct Link {
    from: usize,
    to: usize,
    edge: usize,
    reversed: bool,
    first: bool,
    last: bool,
}

/// Lays out `widths.len()` vertices with the given box widths and the edges between
/// them. An edge from a vertex to itself is ignored.
pub(crate) fn layout(widths: &[usize], edges: &[(usize, usize)]) -> Layout {
    let vertex_count = widths.len();
    let reversed = loop_closing_edges(vertex_count, edges);
    let mut directed: Vec<(usize, usize, usize, bool)> = Vec::with_capacity(edges.len());
    for (index, (from, to)) in edges.iter().enumerate() {
        let flip = reversed.get(index) == Some(&true);
        let in_range = *from < vertex_count && *to < vertex_count;
        if from == to || !in_range {
            continue;
        }
        if flip {
            directed.push((*to, *from, index, true));
        } else {
            directed.push((*from, *to, index, false));
        }
    }

    let vertex_layer = longest_path_layers(vertex_count, &directed);
    let mut item_layer = vertex_layer.clone();
    let mut links: Vec<Link> = Vec::new();
    for (from, to, edge, reversed) in &directed {
        let from_layer = item_layer.get(*from).copied().unwrap_or(0);
        let to_layer = item_layer.get(*to).copied().unwrap_or(0);
        let mut previous = *from;
        let mut layer = from_layer.saturating_add(1);
        let mut first = true;
        while layer < to_layer {
            let dummy = item_layer.len();
            item_layer.push(layer);
            links.push(Link {
                from: previous,
                to: dummy,
                edge: *edge,
                reversed: *reversed,
                first,
                last: false,
            });
            first = false;
            previous = dummy;
            layer = layer.saturating_add(1);
        }
        links.push(Link {
            from: previous,
            to: *to,
            edge: *edge,
            reversed: *reversed,
            first,
            last: true,
        });
    }

    let item_count = item_layer.len();
    let layer_count = item_layer.iter().max().map_or(0, |last| last.saturating_add(1));
    let mut order: Vec<Vec<usize>> = vec![Vec::new(); layer_count];
    for (item, layer) in item_layer.iter().enumerate() {
        if let Some(column) = order.get_mut(*layer) {
            column.push(item);
        }
    }
    let mut predecessors: Vec<Vec<usize>> = vec![Vec::new(); item_count];
    let mut successors: Vec<Vec<usize>> = vec![Vec::new(); item_count];
    for link in &links {
        if let Some(list) = predecessors.get_mut(link.to) {
            list.push(link.from);
        }
        if let Some(list) = successors.get_mut(link.from) {
            list.push(link.to);
        }
    }
    order_columns(&mut order, &predecessors, &successors, item_count);

    let heights: Vec<usize> =
        (0..item_count).map(|item| if item < vertex_count { BOX_HEIGHT } else { 1 }).collect();
    let item_y = place_rows(&order, &predecessors, &heights, item_count);
    let columns = Columns { vertex_layer: &vertex_layer, item_layer: &item_layer, item_y: &item_y };
    geometry(widths, &columns, &links)
}

/// Where the layers put each item, by item index.
struct Columns<'a> {
    vertex_layer: &'a [usize],
    item_layer: &'a [usize],
    item_y: &'a [usize],
}

/// Spaces the columns and turns the ordered items into boxes, runs and segments.
fn geometry(widths: &[usize], columns: &Columns<'_>, links: &[Link]) -> Layout {
    let Columns { vertex_layer, item_layer, item_y } = *columns;
    let vertex_count = widths.len();
    let layer_count = item_layer.iter().max().map_or(0, |last| last.saturating_add(1));
    let middle = |item: usize| -> usize {
        let top = item_y.get(item).copied().unwrap_or(0);
        if item < vertex_count { top.saturating_add(BOX_HEIGHT / 2) } else { top }
    };

    // Column widths and the tracks of each gap.
    let mut column_width: Vec<usize> = vec![1; layer_count];
    for (vertex, width) in widths.iter().enumerate() {
        let layer = vertex_layer.get(vertex).copied().unwrap_or(0);
        if let Some(slot) = column_width.get_mut(layer) {
            *slot = (*slot).max(*width);
        }
    }
    let mut gap_sources: Vec<Vec<usize>> = vec![Vec::new(); layer_count];
    for link in links {
        let bends = middle(link.from) != middle(link.to);
        let layer = item_layer.get(link.from).copied().unwrap_or(0);
        if let (true, Some(rows)) = (bends, gap_sources.get_mut(layer)) {
            rows.push(middle(link.from));
        }
    }
    for rows in &mut gap_sources {
        rows.sort_unstable();
        rows.dedup();
    }
    let mut column_x: Vec<usize> = Vec::with_capacity(layer_count);
    let mut cursor: usize = 0;
    for layer in 0..layer_count {
        column_x.push(cursor);
        let width = column_width.get(layer).copied().unwrap_or(1);
        let tracks = gap_sources.get(layer).map_or(0, Vec::len);
        let gap = tracks.saturating_add(GAP_MARGIN.saturating_mul(2));
        cursor = cursor.saturating_add(width).saturating_add(gap);
    }

    let mut result = Layout::default();
    for vertex in 0..vertex_count {
        let layer = vertex_layer.get(vertex).copied().unwrap_or(0);
        let x = column_x.get(layer).copied().unwrap_or(0);
        let y = item_y.get(vertex).copied().unwrap_or(0);
        let width = widths.get(vertex).copied().unwrap_or(1);
        result.boxes.push(Placed { x, y, width });
        result.width = result.width.max(x.saturating_add(width));
        result.height = result.height.max(y.saturating_add(BOX_HEIGHT));
    }
    for (item, layer) in item_layer.iter().enumerate().skip(vertex_count) {
        let Some(link) = links.iter().find(|link| link.to == item) else { continue };
        let x = column_x.get(*layer).copied().unwrap_or(0);
        let width = column_width.get(*layer).copied().unwrap_or(1);
        let y = item_y.get(item).copied().unwrap_or(0);
        result.runs.push(Run { edge: link.edge, x, y, width });
        result.height = result.height.max(y.saturating_add(1));
    }
    for link in links {
        let layer = item_layer.get(link.from).copied().unwrap_or(0);
        let x0 = if link.from < vertex_count {
            result.boxes.get(link.from).map_or(0, |placed| placed.x.saturating_add(placed.width))
        } else {
            let x = column_x.get(layer).copied().unwrap_or(0);
            x.saturating_add(column_width.get(layer).copied().unwrap_or(1))
        };
        let next_x = column_x.get(layer.saturating_add(1)).copied().unwrap_or(x0);
        let x1 = next_x.saturating_sub(1);
        let (y0, y1) = (middle(link.from), middle(link.to));
        let track = if y0 == y1 {
            None
        } else {
            let sources = gap_sources.get(layer).map_or(&[][..], Vec::as_slice);
            let index = sources.iter().position(|row| *row == y0).unwrap_or(0);
            let gap_start = x0.max(
                column_x
                    .get(layer)
                    .copied()
                    .unwrap_or(0)
                    .saturating_add(column_width.get(layer).copied().unwrap_or(1)),
            );
            Some(gap_start.saturating_add(GAP_MARGIN).saturating_add(index))
        };
        result.segments.push(Segment {
            edge: link.edge,
            x0,
            y0,
            track,
            x1,
            y1,
            head_at_start: link.first && link.reversed,
            head_at_end: link.last && !link.reversed,
        });
    }
    result
}

/// The depth-first search state of a vertex.
const NEW: u8 = 0;
const ON_PATH: u8 = 1;
const DONE: u8 = 2;

/// Marks the edges a depth-first search finds pointing back to a vertex still on its
/// path. Reversing them leaves a graph without loops.
fn loop_closing_edges(vertex_count: usize, edges: &[(usize, usize)]) -> Vec<bool> {
    let mut outgoing: Vec<Vec<(usize, usize)>> = vec![Vec::new(); vertex_count];
    for (index, (from, to)) in edges.iter().enumerate() {
        if let Some(list) = outgoing.get_mut(*from) {
            list.push((*to, index));
        }
    }
    let mut status = vec![NEW; vertex_count];
    let mut reversed = vec![false; edges.len()];
    let mut stack: Vec<(usize, usize)> = Vec::new();
    for root in 0..vertex_count {
        if status.get(root) != Some(&NEW) {
            continue;
        }
        set(&mut status, root, ON_PATH);
        stack.push((root, 0));
        while let Some(&(vertex, next)) = stack.last() {
            let successor = outgoing.get(vertex).and_then(|list| list.get(next)).copied();
            let Some((target, edge)) = successor else {
                set(&mut status, vertex, DONE);
                stack.pop();
                continue;
            };
            if let Some(top) = stack.last_mut() {
                top.1 = next.saturating_add(1);
            }
            match status.get(target).copied() {
                Some(NEW) => {
                    set(&mut status, target, ON_PATH);
                    stack.push((target, 0));
                }
                Some(ON_PATH) => set(&mut reversed, edge, true),
                _ => {}
            }
        }
    }
    reversed
}

/// The layer of every vertex: one more than its deepest predecessor. Edges must form no
/// loop; a vertex a loop would hide stays in layer 0.
fn longest_path_layers(
    vertex_count: usize,
    directed: &[(usize, usize, usize, bool)],
) -> Vec<usize> {
    let mut incoming: Vec<usize> = vec![0; vertex_count];
    let mut outgoing: Vec<Vec<usize>> = vec![Vec::new(); vertex_count];
    for (from, to, _, _) in directed {
        if let Some(count) = incoming.get_mut(*to) {
            *count = count.saturating_add(1);
        }
        if let Some(list) = outgoing.get_mut(*from) {
            list.push(*to);
        }
    }
    let mut layer = vec![0_usize; vertex_count];
    let mut ready: Vec<usize> =
        (0..vertex_count).filter(|v| incoming.get(*v) == Some(&0)).collect();
    while let Some(vertex) = ready.pop() {
        let next_layer = layer.get(vertex).copied().unwrap_or(0).saturating_add(1);
        let targets = outgoing.get(vertex).map_or(&[][..], Vec::as_slice);
        for target in targets {
            if let Some(slot) = layer.get_mut(*target) {
                *slot = (*slot).max(next_layer);
            }
            if let Some(count) = incoming.get_mut(*target) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    ready.push(*target);
                }
            }
        }
    }
    layer
}

/// Reorders every column by the mean position of each item's neighbours, sweeping down
/// by predecessors and up by successors.
fn order_columns(
    order: &mut [Vec<usize>],
    predecessors: &[Vec<usize>],
    successors: &[Vec<usize>],
    item_count: usize,
) {
    let mut position: Vec<usize> = vec![0; item_count];
    for column in order.iter() {
        record_positions(column, &mut position);
    }
    for _ in 0..SWEEPS {
        for column in order.iter_mut().skip(1) {
            sort_by_barycenter(column, predecessors, &mut position);
        }
        let column_count = order.len();
        for column in order.iter_mut().take(column_count.saturating_sub(1)).rev() {
            sort_by_barycenter(column, successors, &mut position);
        }
    }
    // A vertex without edges goes to the bottom of its column, out of the way.
    for column in order.iter_mut() {
        let mut connected: Vec<usize> = Vec::with_capacity(column.len());
        let mut isolated: Vec<usize> = Vec::new();
        for item in column.iter() {
            let has_predecessor = predecessors.get(*item).is_some_and(|list| !list.is_empty());
            let has_successor = successors.get(*item).is_some_and(|list| !list.is_empty());
            if has_predecessor || has_successor {
                connected.push(*item);
            } else {
                isolated.push(*item);
            }
        }
        connected.extend(isolated);
        *column = connected;
    }
}

fn record_positions(column: &[usize], position: &mut [usize]) {
    for (index, item) in column.iter().enumerate() {
        set(position, *item, index);
    }
}

/// Sorts one column by the mean position of each item's neighbours; an item without
/// neighbours keeps its own position.
fn sort_by_barycenter(column: &mut [usize], neighbours: &[Vec<usize>], position: &mut [usize]) {
    let mut keyed: Vec<(usize, usize)> = Vec::with_capacity(column.len());
    for item in column.iter() {
        let around = neighbours.get(*item).map_or(&[][..], Vec::as_slice);
        let own = position.get(*item).copied().unwrap_or(0).saturating_mul(BARYCENTER_SCALE);
        let mut sum: usize = 0;
        for neighbour in around {
            let at = position.get(*neighbour).copied().unwrap_or(0);
            sum = sum.saturating_add(at.saturating_mul(BARYCENTER_SCALE));
        }
        let key = sum.checked_div(around.len()).unwrap_or(own);
        keyed.push((key, *item));
    }
    keyed.sort_by_key(|(key, _)| *key);
    for (slot, (_, item)) in column.iter_mut().zip(keyed) {
        *slot = item;
    }
    record_positions(column, position);
}

/// The top row of every item: as close to the mean middle row of its predecessors as
/// the items above it in its column allow.
fn place_rows(
    order: &[Vec<usize>],
    predecessors: &[Vec<usize>],
    heights: &[usize],
    item_count: usize,
) -> Vec<usize> {
    let mut top: Vec<usize> = vec![0; item_count];
    for column in order {
        let mut cursor: usize = 0;
        for item in column {
            let height = heights.get(*item).copied().unwrap_or(1);
            let around = predecessors.get(*item).map_or(&[][..], Vec::as_slice);
            let mut sum: usize = 0;
            for neighbour in around {
                let neighbour_top = top.get(*neighbour).copied().unwrap_or(0);
                let neighbour_height = heights.get(*neighbour).copied().unwrap_or(1);
                sum = sum.saturating_add(neighbour_top.saturating_add(neighbour_height / 2));
            }
            let wanted = sum
                .checked_div(around.len())
                .map_or(cursor, |middle| middle.saturating_sub(height / 2));
            let placed = wanted.max(cursor);
            set(&mut top, *item, placed);
            cursor = placed.saturating_add(height).saturating_add(ROW_GAP);
        }
    }
    top
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn overlaps(a: &Placed, b: &Placed) -> bool {
        let apart_x = a.x.saturating_add(a.width) <= b.x || b.x.saturating_add(b.width) <= a.x;
        let apart_y =
            a.y.saturating_add(BOX_HEIGHT) <= b.y || b.y.saturating_add(BOX_HEIGHT) <= a.y;
        !(apart_x || apart_y)
    }

    #[test]
    fn a_chain_flows_left_to_right_on_one_row() {
        let placed = layout(&[5, 5, 5], &[(0, 1), (1, 2)]);
        assert!(placed.boxes[0].x < placed.boxes[1].x);
        assert!(placed.boxes[1].x < placed.boxes[2].x);
        assert_eq!(placed.boxes[0].y, placed.boxes[2].y);
        assert!(
            placed.segments.iter().all(|segment| segment.track.is_none() && segment.head_at_end)
        );
    }

    #[test]
    fn a_loop_is_drawn_with_one_edge_pointing_back() {
        let placed = layout(&[4, 4], &[(0, 1), (1, 0)]);
        let backwards: Vec<&Segment> =
            placed.segments.iter().filter(|segment| segment.head_at_start).collect();
        assert_eq!(backwards.len(), 1);
        assert_eq!(backwards[0].edge, 1);
        assert_eq!(placed.segments.len(), 2);
    }

    #[test]
    fn a_long_edge_runs_through_the_columns_it_crosses() {
        let placed = layout(&[3, 3, 3], &[(0, 1), (1, 2), (0, 2)]);
        assert_eq!(placed.runs.len(), 1);
        assert_eq!(placed.runs[0].edge, 2);
        let pieces = placed.segments.iter().filter(|segment| segment.edge == 2).count();
        assert_eq!(pieces, 2);
    }

    #[test]
    fn boxes_never_overlap_and_tracks_stay_in_their_gap() {
        let edges = [(0, 3), (1, 3), (2, 3), (0, 4), (2, 4), (3, 5), (4, 5), (5, 0)];
        let placed = layout(&[6, 8, 4, 10, 3, 7], &edges);
        for (index, a) in placed.boxes.iter().enumerate() {
            for b in placed.boxes.iter().skip(index.saturating_add(1)) {
                assert!(!overlaps(a, b), "{a:?} {b:?}");
            }
        }
        for segment in &placed.segments {
            assert!(segment.x0 <= segment.x1);
            if let Some(track) = segment.track {
                assert!(segment.x0 < track && track < segment.x1, "{segment:?}");
            }
        }
    }

    #[test]
    fn self_edges_and_unknown_vertices_are_ignored() {
        let placed = layout(&[2], &[(0, 0), (0, 9)]);
        assert!(placed.segments.is_empty(), "{:?}", placed.segments);
        assert_eq!(placed.boxes.len(), 1);
    }
}
