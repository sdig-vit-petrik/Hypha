//! The Graph tab: the node graph drawn with box characters, nodes coloured by health
//! and loops in the warning colours.

use crate::top::graph::{
    self, Graph, Health, LoopLevel, NodeFacts, Options, Vertex, VertexKind, View,
};
use crate::top::graph_layout::{BOX_HEIGHT, Layout, Segment, layout};
use crate::top::state::{AppState, Tab};
use crate::top::ui::muted_block;
use crossterm::event::KeyCode;
use hypha_node::sdk::NodeInfo;
use hypha_tools::theme::{ACCENT, ACCENT2, ERR, FG, MUTED, OK, WARN};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout as Split, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use std::collections::BTreeMap;
use std::time::Instant;

/// Where `e` writes the graph.
const DOT_PATH: &str = "hypha_graph.dot";
/// Columns `←`/`→` pan, and rows `PgUp`/`PgDn` pan.
const PAN_X: usize = 8;
const PAN_Y: usize = 5;
/// The largest canvas drawn; a larger graph asks for a filter or an export.
const MAX_CANVAS_CELLS: usize = 4_000_000;
/// Rows under the canvas: loops, selection, keys, legend.
const STATUS_ROWS: u16 = 4;

/// The state of the Graph tab between frames.
#[derive(Debug, Default)]
pub(crate) struct GraphView {
    pub view: View,
    pub infrastructure: bool,
    /// The key of the selected vertex; it survives a rebuild of the graph.
    selected: Option<String>,
    /// Selection moves not yet applied; the vertex order is known only when drawing.
    moves: i32,
    /// Scroll the selected vertex into view on the next frame.
    follow: bool,
    scroll_x: usize,
    scroll_y: usize,
    /// What the next frame carries out; it needs the graph.
    request: Option<Request>,
    message: String,
}

/// A key action that needs the built graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Request {
    Export,
    Open,
}

impl GraphView {
    /// Handles one key of the Graph tab. Returns `false` for a key the tab leaves to
    /// the global bindings.
    pub(crate) fn handle_key(&mut self, code: KeyCode) -> bool {
        match code {
            KeyCode::Up | KeyCode::Char('k') => self.moves = self.moves.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.moves = self.moves.saturating_add(1),
            KeyCode::Home => self.moves = i32::MIN,
            KeyCode::End => self.moves = i32::MAX,
            KeyCode::Left | KeyCode::Char('h') => {
                self.scroll_x = self.scroll_x.saturating_sub(PAN_X);
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.scroll_x = self.scroll_x.saturating_add(PAN_X);
            }
            KeyCode::PageUp => self.scroll_y = self.scroll_y.saturating_sub(PAN_Y),
            KeyCode::PageDown => self.scroll_y = self.scroll_y.saturating_add(PAN_Y),
            KeyCode::Char('g') => {
                self.view = match self.view {
                    View::Nodes => View::Topics,
                    View::Topics => View::Nodes,
                };
                self.follow = true;
            }
            KeyCode::Char('i') => {
                self.infrastructure = !self.infrastructure;
                self.follow = true;
            }
            KeyCode::Char('e') => self.request = Some(Request::Export),
            KeyCode::Enter => self.request = Some(Request::Open),
            _ => return false,
        }
        true
    }

    /// Applies the pending moves to the vertex order and returns the selected vertex.
    fn resolve_selection(&mut self, graph: &Graph, order: &[usize]) -> Option<usize> {
        let moves = std::mem::take(&mut self.moves);
        let last = order.len().checked_sub(1)?;
        let current = order.iter().position(|vertex| {
            let key = graph.vertices.get(*vertex).map(Vertex::key);
            key.is_some() && key == self.selected
        });
        let base = current.unwrap_or(0);
        let step = usize::try_from(moves.unsigned_abs()).unwrap_or(usize::MAX);
        let target = if moves < 0 { base.saturating_sub(step) } else { base.saturating_add(step) };
        let index = target.min(last);
        if current != Some(index) {
            self.follow = true;
        }
        let vertex = order.get(index).copied()?;
        self.selected = graph.vertices.get(vertex).map(Vertex::key);
        Some(vertex)
    }
}

/// Draws the Graph tab and carries out its export and open requests.
pub(crate) fn draw_graph_tab(
    f: &mut Frame<'_>,
    area: Rect,
    state: &mut AppState,
    nodes: &[NodeInfo],
) {
    let graph = build_graph(state, nodes, Instant::now());
    let widths: Vec<usize> = graph.vertices.iter().map(box_width).collect();
    let edges: Vec<(usize, usize)> = graph.edges.iter().map(|edge| (edge.from, edge.to)).collect();
    let placed = layout(&widths, &edges);

    let mut order: Vec<usize> = (0..graph.vertices.len()).collect();
    order.sort_by_key(|vertex| placed.boxes.get(*vertex).map(|b| (b.x, b.y)));
    let selected = state.graph.resolve_selection(&graph, &order);

    match state.graph.request.take() {
        Some(Request::Export) => {
            state.graph.message = match std::fs::write(DOT_PATH, graph::to_dot(&graph)) {
                Ok(()) => format!("wrote {DOT_PATH}"),
                Err(error) => format!("cannot write {DOT_PATH}: {error}"),
            };
        }
        Some(Request::Open) => {
            open(state, nodes, selected.and_then(|vertex| graph.vertices.get(vertex)));
        }
        None => {}
    }

    let block = muted_block().title(title(state));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let [canvas_area, status_area] = Split::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(STATUS_ROWS)])
        .areas(inner);

    if graph.vertices.is_empty() {
        let text = Paragraph::new(" No nodes on the bus (or none match the filter).")
            .style(Style::default().fg(MUTED));
        f.render_widget(text, canvas_area);
    } else if let Some(canvas) = Canvas::paint(&graph, &placed, selected) {
        let selected_box = selected.and_then(|vertex| placed.boxes.get(vertex)).copied();
        let view = &mut state.graph;
        if std::mem::take(&mut view.follow)
            && let Some(selected_box) = selected_box
        {
            view.scroll_x =
                scroll_to(view.scroll_x, selected_box.x, selected_box.width, canvas_area.width);
            view.scroll_y =
                scroll_to(view.scroll_y, selected_box.y, BOX_HEIGHT, canvas_area.height);
        }
        view.scroll_x =
            view.scroll_x.min(canvas.width.saturating_sub(usize::from(canvas_area.width)));
        view.scroll_y =
            view.scroll_y.min(canvas.height.saturating_sub(usize::from(canvas_area.height)));
        canvas.blit(f, canvas_area, view.scroll_x, view.scroll_y);
    } else {
        let text = Paragraph::new(
            " The graph is too large to draw: filter it with [/] or export it with [e].",
        )
        .style(Style::default().fg(WARN));
        f.render_widget(text, canvas_area);
    }

    let lines = vec![
        loops_line(&graph),
        selection_line(&graph, selected, &state.graph.message),
        keys_line(),
        legend_line(),
    ];
    f.render_widget(Paragraph::new(lines), status_area);
}

/// The graph of every node the registry lists or that sent a manifest.
fn build_graph(state: &AppState, nodes: &[NodeInfo], now: Instant) -> Graph {
    let mut alive: BTreeMap<&str, Option<bool>> = BTreeMap::new();
    for node in nodes {
        let entry = alive.entry(node.name.as_str()).or_insert(Some(false));
        if node.is_alive {
            *entry = Some(true);
        }
    }
    for name in state.node_introspections.keys() {
        alive.entry(name.as_str()).or_insert(None);
    }
    let mut facts: Vec<NodeFacts<'_>> = Vec::with_capacity(alive.len());
    for (name, is_alive) in &alive {
        facts.push(NodeFacts {
            name,
            health: Health::of(*is_alive, state.node_states.get(*name), now),
            manifest: state.node_introspections.get(*name),
        });
    }
    let options = Options {
        view: state.graph.view,
        infrastructure: state.graph.infrastructure,
        filter: &state.filter_query,
    };
    graph::build(&facts, state.topics.keys().map(String::as_str), &options)
}

/// Switches to the Nodes or Topics tab with the selected vertex selected there.
fn open(state: &mut AppState, nodes: &[NodeInfo], vertex: Option<&Vertex>) {
    let Some(vertex) = vertex else { return };
    match vertex.kind {
        VertexKind::Node { .. } => {
            let sorted = crate::top::ui::sorted_nodes(nodes);
            match sorted.iter().position(|node| node.name == vertex.name) {
                Some(index) => {
                    state.selected_node = index;
                    state.active_tab = Tab::NodeRegistry;
                }
                None => {
                    state.graph.message = format!("{} is not in the node registry", vertex.name);
                }
            }
        }
        VertexKind::Topic { .. } => {
            let position =
                state.visible_topics().iter().position(|(topic, _)| **topic == vertex.name);
            match position {
                Some(index) => {
                    state.selected_topic = index;
                    state.active_tab = Tab::TopicsBus;
                }
                None => state.graph.message = format!("{} has carried no traffic yet", vertex.name),
            }
        }
    }
}

/// The new scroll offset that keeps `[start, start + length)` inside a view of `view` cells.
fn scroll_to(scroll: usize, start: usize, length: usize, view: u16) -> usize {
    let view = usize::from(view);
    let end = start.saturating_add(length);
    if start < scroll {
        return start;
    }
    if end > scroll.saturating_add(view) {
        return end.saturating_sub(view);
    }
    scroll
}

fn title(state: &AppState) -> String {
    let view = match state.graph.view {
        View::Nodes => "nodes",
        View::Topics => "nodes and topics",
    };
    let infrastructure = if state.graph.infrastructure { "shown" } else { "hidden" };
    if state.filter_query.is_empty() {
        format!(" Graph: {view} \u{00b7} infrastructure {infrastructure} ")
    } else {
        format!(
            " Graph: {view} \u{00b7} infrastructure {infrastructure} \u{00b7} around \"{}\" ",
            state.filter_query
        )
    }
}

/// The text inside a vertex box.
fn label(vertex: &Vertex) -> String {
    match vertex.kind {
        VertexKind::Node { health, manifest } => {
            let mut text = format!("{} {}", health.marker(), vertex.name);
            if vertex.self_loop {
                text.push_str(" \u{21bb}");
            }
            if !manifest {
                text.push_str(" \u{2205}");
            }
            text
        }
        VertexKind::Topic { .. } => vertex.name.clone(),
    }
}

/// Border, space, label, space, border.
fn box_width(vertex: &Vertex) -> usize {
    label(vertex).chars().count().saturating_add(4)
}

fn health_style(health: Health) -> Style {
    let colour = match health {
        Health::Ok => OK,
        Health::NoState => FG,
        Health::Waiting => WARN,
        Health::Silent | Health::Degraded | Health::Dead => ERR,
    };
    Style::default().fg(colour)
}

/// How an edge or a border is drawn; a higher rank wins where lines share a cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Paint {
    Plain,
    Selected,
    /// Any loop. A loop is a warning, never an error: it can be intended.
    Loop,
}

impl Paint {
    fn of(level: LoopLevel, touches_selection: bool) -> Self {
        match level {
            LoopLevel::Triggered | LoopLevel::Feedback => Self::Loop,
            LoopLevel::None if touches_selection => Self::Selected,
            LoopLevel::None => Self::Plain,
        }
    }

    fn style(self) -> Style {
        match self {
            Self::Plain => Style::default().fg(MUTED),
            Self::Selected => Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            Self::Loop => Style::default().fg(WARN).add_modifier(Modifier::BOLD),
        }
    }
}

const UP: u8 = 1;
const DOWN: u8 = 2;
const LEFT: u8 = 4;
const RIGHT: u8 = 8;

/// The box character joining the line ends in `bits`.
fn line_char(bits: u8) -> char {
    let up = bits & UP != 0;
    let down = bits & DOWN != 0;
    let left = bits & LEFT != 0;
    let right = bits & RIGHT != 0;
    match (up, down, left, right) {
        (false, false, false, false) => ' ',
        (false, false, _, _) => '\u{2500}',
        (_, _, false, false) => '\u{2502}',
        (false, true, true, false) => '\u{2510}',
        (false, true, false, true) => '\u{250c}',
        (true, false, true, false) => '\u{2518}',
        (true, false, false, true) => '\u{2514}',
        (true, true, true, false) => '\u{2524}',
        (true, true, false, true) => '\u{251c}',
        (false, true, true, true) => '\u{252c}',
        (true, false, true, true) => '\u{2534}',
        (true, true, true, true) => '\u{253c}',
    }
}

/// How edge `edge` is drawn: its loop colour, or the selection colour when it touches
/// the selected vertex.
fn edge_paint(graph: &Graph, edge: usize, selected: Option<usize>) -> Paint {
    let Some(edge) = graph.edges.get(edge) else { return Paint::Plain };
    let touches = selected.is_some_and(|vertex| edge.from == vertex || edge.to == vertex);
    Paint::of(edge.level, touches)
}

/// One character cell of the canvas.
#[derive(Debug, Clone, Copy)]
struct Cell {
    /// Line ends meeting here.
    bits: u8,
    /// A character that replaces the line, such as box text or an arrow head.
    symbol: Option<char>,
    line: Paint,
    style: Style,
}

impl Default for Cell {
    fn default() -> Self {
        Self { bits: 0, symbol: None, line: Paint::Plain, style: Paint::Plain.style() }
    }
}

/// The whole graph drawn into a grid of cells, larger than the screen.
struct Canvas {
    width: usize,
    height: usize,
    cells: Vec<Cell>,
}

impl Canvas {
    /// Draws the graph, or returns `None` when it is larger than `MAX_CANVAS_CELLS`.
    fn paint(graph: &Graph, placed: &Layout, selected: Option<usize>) -> Option<Self> {
        let width = placed.width.saturating_add(1);
        let height = placed.height;
        let size = width.checked_mul(height).filter(|size| *size <= MAX_CANVAS_CELLS)?;
        let mut canvas = Self { width, height, cells: vec![Cell::default(); size] };

        for run in &placed.runs {
            let paint = edge_paint(graph, run.edge, selected);
            let end = run.x.saturating_add(run.width).saturating_sub(1);
            canvas.add_bits(run.x, run.y, LEFT, paint);
            canvas.stroke((run.x, run.y), (end, run.y), paint);
            canvas.add_bits(end, run.y, RIGHT, paint);
        }
        for segment in &placed.segments {
            let paint = edge_paint(graph, segment.edge, selected);
            canvas.segment(segment, paint);
        }
        for segment in &placed.segments {
            let paint = edge_paint(graph, segment.edge, selected);
            if segment.head_at_end {
                canvas.put(segment.x1, segment.y1, '\u{25b6}', paint.style());
            }
            if segment.head_at_start {
                canvas.put(segment.x0, segment.y0, '\u{25c0}', paint.style());
            }
        }
        for (index, vertex) in graph.vertices.iter().enumerate() {
            if let Some(placed_box) = placed.boxes.get(index) {
                canvas.vertex(
                    vertex,
                    placed_box.x,
                    placed_box.y,
                    placed_box.width,
                    selected == Some(index),
                );
            }
        }
        Some(canvas)
    }

    fn cell(&mut self, x: usize, y: usize) -> Option<&mut Cell> {
        if x >= self.width {
            return None;
        }
        let index = y.checked_mul(self.width)?.checked_add(x)?;
        self.cells.get_mut(index)
    }

    fn add_bits(&mut self, x: usize, y: usize, bits: u8, paint: Paint) {
        if let Some(cell) = self.cell(x, y) {
            cell.bits |= bits;
            if paint >= cell.line {
                cell.line = paint;
            }
        }
    }

    fn put(&mut self, x: usize, y: usize, symbol: char, style: Style) {
        if let Some(cell) = self.cell(x, y) {
            cell.symbol = Some(symbol);
            cell.style = style;
        }
    }

    /// A straight horizontal or vertical line between two cells.
    fn stroke(&mut self, from: (usize, usize), to: (usize, usize), paint: Paint) {
        let (x0, y0) = from;
        let (x1, y1) = to;
        if y0 == y1 {
            for x in x0.min(x1)..x0.max(x1) {
                self.add_bits(x, y0, RIGHT, paint);
                self.add_bits(x.saturating_add(1), y0, LEFT, paint);
            }
        } else if x0 == x1 {
            for y in y0.min(y1)..y0.max(y1) {
                self.add_bits(x0, y, DOWN, paint);
                self.add_bits(x0, y.saturating_add(1), UP, paint);
            }
        }
    }

    fn segment(&mut self, segment: &Segment, paint: Paint) {
        let start = (segment.x0, segment.y0);
        let end = (segment.x1, segment.y1);
        self.add_bits(segment.x0, segment.y0, LEFT, paint);
        match segment.track {
            Some(track) => {
                self.stroke(start, (track, segment.y0), paint);
                self.stroke((track, segment.y0), (track, segment.y1), paint);
                self.stroke((track, segment.y1), end, paint);
            }
            None => self.stroke(start, end, paint),
        }
        self.add_bits(segment.x1, segment.y1, RIGHT, paint);
    }

    /// A node is a square box, a topic a rounded one.
    fn vertex(&mut self, vertex: &Vertex, x: usize, y: usize, width: usize, selected: bool) {
        let border = if selected {
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Paint::of(vertex.level, false).style()
        };
        let corners = match vertex.kind {
            VertexKind::Node { .. } => ['\u{250c}', '\u{2510}', '\u{2514}', '\u{2518}'],
            VertexKind::Topic { .. } => ['\u{256d}', '\u{256e}', '\u{2570}', '\u{256f}'],
        };
        let right = x.saturating_add(width).saturating_sub(1);
        let bottom = y.saturating_add(BOX_HEIGHT).saturating_sub(1);
        let middle = y.saturating_add(1);
        for column in x..=right {
            self.put(column, y, '\u{2500}', border);
            self.put(column, bottom, '\u{2500}', border);
            self.put(column, middle, ' ', border);
        }
        self.put(x, middle, '\u{2502}', border);
        self.put(right, middle, '\u{2502}', border);
        let [top_left, top_right, bottom_left, bottom_right] = corners;
        self.put(x, y, top_left, border);
        self.put(right, y, top_right, border);
        self.put(x, bottom, bottom_left, border);
        self.put(right, bottom, bottom_right, border);

        let text_style = match vertex.kind {
            VertexKind::Node { health, .. } => health_style(health),
            VertexKind::Topic { publishers, subscribers } => {
                let dangling = publishers == 0 || subscribers == 0;
                Style::default().fg(if dangling { MUTED } else { ACCENT2 })
            }
        };
        let text_style =
            if selected { text_style.add_modifier(Modifier::BOLD) } else { text_style };
        let mut column = x.saturating_add(2);
        for symbol in label(vertex).chars() {
            self.put(column, middle, symbol, text_style);
            column = column.saturating_add(1);
        }
    }

    /// Copies the part of the canvas from `(left, top)` into `area`.
    fn blit(&self, f: &mut Frame<'_>, area: Rect, left: usize, top: usize) {
        let buffer = f.buffer_mut();
        for row in 0..area.height {
            for column in 0..area.width {
                let x = left.saturating_add(usize::from(column));
                let y = top.saturating_add(usize::from(row));
                if x >= self.width {
                    continue;
                }
                let index = y.checked_mul(self.width).and_then(|base| base.checked_add(x));
                let Some(cell) = index.and_then(|index| self.cells.get(index)) else { continue };
                let (symbol, style) = match cell.symbol {
                    Some(symbol) => (symbol, cell.style),
                    None if cell.bits == 0 => continue,
                    None => (line_char(cell.bits), cell.line.style()),
                };
                let target = (area.x.saturating_add(column), area.y.saturating_add(row));
                if let Some(slot) = buffer.cell_mut(target) {
                    slot.set_char(symbol);
                    slot.set_style(style);
                }
            }
        }
    }
}

fn loops_line(graph: &Graph) -> Line<'static> {
    if graph.loops.is_empty() {
        return Line::styled(" No loops", Style::default().fg(OK));
    }
    let mut spans = vec![Span::styled(
        format!(" \u{26a0} {} loop(s), check they behave as intended: ", graph.loops.len()),
        Style::default().fg(WARN).add_modifier(Modifier::BOLD),
    )];
    for item in &graph.loops {
        let kind = match item.level {
            LoopLevel::Triggered => "triggered",
            LoopLevel::Feedback | LoopLevel::None => "feedback",
        };
        spans.push(Span::styled(
            format!("[{}] {kind}  ", item.nodes.join(", ")),
            Paint::Loop.style(),
        ));
    }
    Line::from(spans)
}

fn selection_line(graph: &Graph, selected: Option<usize>, message: &str) -> Line<'static> {
    let Some(vertex) = selected.and_then(|index| graph.vertices.get(index)) else {
        return Line::styled(format!(" {message}"), Style::default().fg(MUTED));
    };
    let index = selected.unwrap_or(0);
    let mut outgoing: Vec<&str> = Vec::new();
    let mut incoming: Vec<&str> = Vec::new();
    for edge in &graph.edges {
        if edge.from == index
            && let Some(target) = graph.vertices.get(edge.to)
        {
            outgoing.push(target.name.as_str());
        }
        if edge.to == index
            && let Some(source) = graph.vertices.get(edge.from)
        {
            incoming.push(source.name.as_str());
        }
    }
    let state = match vertex.kind {
        VertexKind::Node { health, manifest: true } => health.label().to_string(),
        VertexKind::Node { health, manifest: false } => format!("{}, no manifest", health.label()),
        VertexKind::Topic { publishers, subscribers } => {
            format!("{publishers} publisher(s), {subscribers} subscriber(s)")
        }
    };
    let mut spans = vec![
        Span::styled(
            format!(" \u{25b8} {} ", vertex.name),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("({state})  "), Style::default().fg(FG)),
        Span::styled(format!("\u{2190} {}  ", list_or_dash(&incoming)), Style::default().fg(MUTED)),
        Span::styled(format!("\u{2192} {}", list_or_dash(&outgoing)), Style::default().fg(MUTED)),
    ];
    if !message.is_empty() {
        spans.push(Span::styled(format!("   {message}"), Style::default().fg(WARN)));
    }
    Line::from(spans)
}

fn list_or_dash(names: &[&str]) -> String {
    if names.is_empty() {
        return "-".to_string();
    }
    names.join(", ")
}

fn keys_line() -> Line<'static> {
    let key = Style::default().fg(WARN).add_modifier(Modifier::BOLD);
    let text = Style::default().fg(MUTED);
    let mut spans = Vec::new();
    for (keys, what) in [
        ("[\u{2191}\u{2193}]", " select "),
        ("[\u{2190}\u{2192} PgUp PgDn]", " pan "),
        ("[g]", " topics "),
        ("[i]", " infrastructure "),
        ("[/]", " neighbourhood "),
        ("[e]", " export .dot "),
        ("[Enter]", " open"),
    ] {
        spans.push(Span::styled(format!(" {keys}"), key));
        spans.push(Span::styled(what, text));
    }
    Line::from(spans)
}

fn legend_line() -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    for health in [
        Health::Ok,
        Health::Waiting,
        Health::Degraded,
        Health::Silent,
        Health::Dead,
        Health::NoState,
    ] {
        spans.push(Span::styled(
            format!("{} {} ", health.marker(), health.label()),
            health_style(health),
        ));
    }
    spans
        .push(Span::styled("\u{2205} no manifest \u{21bb} self loop ", Style::default().fg(MUTED)));
    spans.push(Span::styled("\u{2500} loop", Paint::Loop.style()));
    Line::from(spans)
}

#[cfg(test)]
#[path = "graph_ui_tests.rs"]
mod tests;
