//! Rendering: header, gauges, per-tab content, footer, and the message inspector modal.

use crate::top::inspector::{SchemaCache, decode_lines};
use crate::top::intercept::{Layout as InterceptLayout, layout_for};
use crate::top::intercept_ui::{draw_body, draw_hints, draw_too_small};
use crate::top::state::{AppState, Tab, format_bandwidth, format_bytes, format_rate};
use hypha_node::sdk::NodeInfo;
use hypha_node::sdk::{BusNode, StreamKey};
use hypha_tools::approx_f64;
use hypha_tools::proc_usage::{Usage, format_memory};
use hypha_tools::stream::{fit_type_label, stream_label};
use hypha_tools::theme::{ACCENT, ACCENT2, BG, ERR, FG, MUTED, OK, ON_ACCENT, SURFACE, WARN};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, Gauge, Paragraph, Row, Table, TableState, Tabs, Wrap,
};
use std::time::Instant;

const TAB_TITLES: [&str; 5] =
    [" 1.Topics&Bus ", " 2.Nodes ", " 3.Intercept ", " 4.Graph ", " 5.Help "];

const THROUGHPUT_FULL_SCALE_KBS: f64 = 20_000.0;
const RATE_FULL_SCALE_HZ: f64 = 5_000.0;

/// A table header row: accent bold column names, one blank line below.
fn header_row(columns: &[&str]) -> Row<'static> {
    let style = Style::default().fg(ACCENT).add_modifier(Modifier::BOLD);
    let cells: Vec<Cell<'static>> =
        columns.iter().map(|name| Cell::from((*name).to_owned()).style(style)).collect();
    Row::new(cells).height(1).bottom_margin(1)
}

/// The style of a table row: highlighted when selected, plain otherwise.
fn row_base(is_selected: bool) -> Style {
    if is_selected {
        Style::default().fg(ON_ACCENT).bg(ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    }
}

/// `base` on the selected row, otherwise `base` in `color`.
fn tone(is_selected: bool, base: Style, color: Color) -> Style {
    if is_selected { base } else { base.fg(color) }
}

/// `tone` plus bold on an unselected row.
fn tone_bold(is_selected: bool, base: Style, color: Color) -> Style {
    if is_selected { base } else { base.fg(color).add_modifier(Modifier::BOLD) }
}

/// A full border in the muted colour; the caller adds the title.
pub(crate) fn muted_block() -> Block<'static> {
    Block::default().borders(Borders::ALL).border_style(Style::default().fg(MUTED))
}

pub(crate) fn draw(
    f: &mut Frame<'_>,
    state: &mut AppState,
    nodes: &[NodeInfo],
    bus: &BusNode,
    schema_cache: &mut SchemaCache,
) {
    clamp_selection(state, nodes);

    let area = f.area();
    f.render_widget(Block::default().style(Style::default().bg(BG).fg(FG)), area);

    if state.active_tab == Tab::Intercept {
        draw_intercept_screen(f, area, state, bus, schema_cache);
        return;
    }

    let [header, gauges, tab_bar, body, footer] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .areas(area);

    draw_header(f, header, state);
    draw_gauges(f, gauges, state);
    draw_tab_bar(f, tab_bar, state.active_tab);

    match state.active_tab {
        Tab::TopicsBus => draw_topics_tab(f, body, state),
        Tab::NodeRegistry => draw_nodes_tab(f, body, state, nodes),
        Tab::Graph => crate::top::graph_ui::draw_graph_tab(f, body, state, nodes),
        Tab::Intercept | Tab::Help => draw_help_tab(f, body),
    }

    draw_footer(f, footer);

    if state.show_inspector {
        draw_inspector_modal(f, area, state, nodes, bus, schema_cache);
    }
}

/// The Intercept tab drops the gauges, so its panes keep room down to 60×12; below
/// that the whole terminal shows the size notice and no state is touched.
fn draw_intercept_screen(
    f: &mut Frame<'_>,
    area: Rect,
    state: &mut AppState,
    bus: &BusNode,
    schema_cache: &mut SchemaCache,
) {
    let layout = layout_for(area.width, area.height);
    if layout == InterceptLayout::TooSmall {
        draw_too_small(f, area);
        return;
    }
    let [header, tab_bar, body, footer] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .areas(area);
    draw_header(f, header, state);
    draw_tab_bar(f, tab_bar, state.active_tab);
    let identity = state.intercept.spec().and_then(|spec| {
        let input = state.topics.get(&spec.input)?;
        let resolved = !input.data_type.is_empty() && !input.md5.is_empty();
        resolved.then(|| (spec.input.clone(), input.data_type.clone(), input.md5.clone()))
    });
    let schema = identity
        .as_ref()
        .and_then(|(topic, data_type, md5)| schema_cache.get(bus, topic, data_type, md5));
    draw_body(f, body, layout, &mut state.intercept, &state.topics, schema, Instant::now());
    draw_hints(f, footer, &state.intercept);
}

fn clamp_selection(state: &mut AppState, nodes: &[NodeInfo]) {
    let topic_count = state.visible_topics().len();
    state.selected_topic = clamp_to_last_index(state.selected_topic, topic_count);
    state.selected_node = clamp_to_last_index(state.selected_node, nodes.len());
}

fn clamp_to_last_index(index: usize, count: usize) -> usize {
    if count == 0 { 0 } else { index.min(count.saturating_sub(1)) }
}

fn draw_header(f: &mut Frame<'_>, area: Rect, state: &AppState) {
    let clock_str = state
        .sim_clock_sec
        .map_or_else(|| "Sim Clock: Inactive".to_string(), |secs| format!("Sim Clock: {secs:.3}s"));
    let status_span = if state.paused {
        Span::styled(" [PAUSED] ", Style::default().fg(WARN).add_modifier(Modifier::BOLD))
    } else {
        Span::styled(" LIVE ", Style::default().fg(OK))
    };

    let line = Line::from(vec![
        Span::styled(
            " hypha top ",
            Style::default().bg(ACCENT).fg(ON_ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled("\u{25cf} BUS ACTIVE", Style::default().fg(OK).add_modifier(Modifier::BOLD)),
        Span::raw("   "),
        Span::styled(clock_str, Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
        Span::raw("   "),
        status_span,
    ]);
    let line = match &state.notice {
        Some(notice) => {
            let mut spans = line.spans;
            spans.push(Span::raw("  "));
            spans.push(Span::styled(notice.clone(), Style::default().fg(WARN)));
            Line::from(spans)
        }
        None => line,
    };
    f.render_widget(Paragraph::new(line), area);
}

fn draw_gauges(f: &mut Frame<'_>, area: Rect, state: &AppState) {
    let [throughput_row, rate_row, volume_row] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1), Constraint::Length(1)])
        .areas(area);

    let bw_ratio = (state.bus_bandwidth_kbs / THROUGHPUT_FULL_SCALE_KBS).clamp(0.0, 1.0);
    let rate_ratio = (state.bus_rate_hz / RATE_FULL_SCALE_HZ).clamp(0.0, 1.0);

    let throughput_label = format!(
        "{} (Peak: {})",
        format_bandwidth(state.bus_bandwidth_kbs),
        format_bandwidth(state.peak_bandwidth_kbs)
    );
    f.render_widget(
        Gauge::default()
            .ratio(bw_ratio)
            .label(throughput_label)
            .gauge_style(Style::default().fg(WARN).bg(BG))
            .block(Block::default().borders(Borders::NONE).title(" Throughput ")),
        throughput_row,
    );

    let rate_label =
        format!("{} (Peak: {})", format_rate(state.bus_rate_hz), format_rate(state.peak_rate_hz));
    f.render_widget(
        Gauge::default()
            .ratio(rate_ratio)
            .label(rate_label)
            .gauge_style(Style::default().fg(ACCENT2).bg(BG))
            .block(Block::default().borders(Borders::NONE).title(" Bus Rate   ")),
        rate_row,
    );

    let shm_label = format!(
        "{} (Bus Volume) | Topics: {} | Total: {} msgs",
        format_bytes(state.total_bus_bytes),
        state.topics.len(),
        state.total_bus_messages
    );
    f.render_widget(
        Gauge::default()
            .ratio(0.0)
            .label(shm_label)
            .gauge_style(Style::default().fg(ACCENT).bg(BG))
            .block(Block::default().borders(Borders::NONE).title(" SHM Memory ")),
        volume_row,
    );
}

fn draw_tab_bar(f: &mut Frame<'_>, area: Rect, active: Tab) {
    let tabs = Tabs::new(TAB_TITLES.to_vec())
        .select(active.index())
        .style(Style::default().fg(MUTED))
        .highlight_style(Style::default().fg(ON_ACCENT).bg(ACCENT).add_modifier(Modifier::BOLD))
        .block(muted_block());
    f.render_widget(tabs, area);
}

fn draw_footer(f: &mut Frame<'_>, area: Rect) {
    let footer = Paragraph::new(
        " [1-6] Tabs  [\u{2191}/\u{2193}] Select  [Enter] Inspect  [/] Search  [s] Sort  [Space] Pause  [q] Quit ",
    )
    .style(Style::default().bg(SURFACE).fg(FG).add_modifier(Modifier::BOLD));
    f.render_widget(footer, area);
}

fn draw_topics_tab(f: &mut Frame<'_>, area: Rect, state: &mut AppState) {
    let mut table_state = std::mem::take(&mut state.topics_table);
    table_state.select(Some(state.selected_topic));
    draw_topics_table(f, area, state, &mut table_state);
    state.topics_table = table_state;
}

/// The topics table. Rendered with its state, so a list longer than the view
/// scrolls to keep the selected row visible.
fn draw_topics_table(
    f: &mut Frame<'_>,
    area: Rect,
    state: &AppState,
    table_state: &mut TableState,
) {
    let [filter_area, table_area] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(3)])
        .areas(area);

    let filter_line = Line::from(vec![
        Span::styled(" Filter: ", Style::default().fg(WARN).add_modifier(Modifier::BOLD)),
        Span::styled(
            if state.filter_query.is_empty() {
                "(press / to search)"
            } else {
                state.filter_query.as_str()
            },
            Style::default().fg(if state.search_focused { FG } else { MUTED }),
        ),
        Span::raw("   "),
        Span::styled(" Sort: ", Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(state.sort_by.label(), Style::default().fg(WARN).add_modifier(Modifier::BOLD)),
    ]);
    f.render_widget(Paragraph::new(filter_line), filter_area);

    let visible = state.visible_topics();
    let header = header_row(&["TOPIC", "TYPE", "RATE (Hz)", "BW (KB/s)", "MSGS", "TOTAL SIZE"]);

    let selected = state.selected_topic;
    let type_width = column_width(table_area, 28);
    let rows = visible.iter().enumerate().map(|(i, (stream, stats))| {
        let is_selected = i == selected;
        let base = row_base(is_selected);

        Row::new(vec![
            Cell::from(stream.topic.clone()).style(tone(is_selected, base, FG)),
            Cell::from(type_cell(stats, type_width)).style(tone(is_selected, base, OK)),
            Cell::from(format_rate(stats.rate_hz)).style(tone(is_selected, base, ACCENT2)),
            Cell::from(format_bandwidth(stats.bandwidth_kbs)).style(tone(is_selected, base, WARN)),
            Cell::from(stats.total_messages.to_string()).style(tone(is_selected, base, FG)),
            Cell::from(format_bytes(stats.total_bytes)).style(tone(is_selected, base, MUTED)),
        ])
        .height(1)
    });

    let title = if visible.is_empty() {
        " Topics (no traffic seen matching filter) ".to_string()
    } else {
        format!(" Topics ({}) ", visible.len())
    };

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(28),
            Constraint::Percentage(28),
            Constraint::Percentage(12),
            Constraint::Percentage(12),
            Constraint::Percentage(10),
            Constraint::Percentage(10),
        ],
    )
    .header(header)
    .block(muted_block().title(title));

    f.render_stateful_widget(table, table_area, table_state);
}

/// The type of a stream and the start of its MD5, so two versions of a type are two
/// different rows: `?` while its schema is not resolved.
fn type_cell(stats: &crate::top::state::TopicStats, width: usize) -> String {
    if stats.data_type.is_empty() || stats.md5.is_empty() {
        return "?".to_owned();
    }
    fit_type_label(&stats.data_type, &stats.md5, width)
}

/// The columns of a table column that takes `percent` of `area`, less the gap before the next.
fn column_width(area: Rect, percent: u16) -> usize {
    usize::from(area.width)
        .saturating_mul(usize::from(percent))
        .checked_div(100)
        .unwrap_or(0)
        .saturating_sub(1)
}

pub(crate) fn sorted_nodes(nodes: &[NodeInfo]) -> Vec<&NodeInfo> {
    let mut sorted: Vec<&NodeInfo> = nodes.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    sorted
}

fn draw_nodes_tab(f: &mut Frame<'_>, area: Rect, state: &mut AppState, nodes: &[NodeInfo]) {
    let [list_area, details_area] = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .areas(area);

    let sorted = sorted_nodes(nodes);
    let mut table_state = std::mem::take(&mut state.nodes_table);
    table_state.select(Some(state.selected_node));
    draw_nodes_list_panel(f, list_area, state, &sorted, &mut table_state);
    state.nodes_table = table_state;
    draw_node_details_panel(f, details_area, state, &sorted);
}

fn format_node_status(
    is_alive: bool,
    node_st: Option<&(hypha_node::sdk::msgs::hypha_msgs::NodeState, Instant)>,
    intro: Option<&hypha_node::sdk::msgs::hypha_msgs::NodeIntrospection>,
    is_selected: bool,
    base: Style,
) -> (&'static str, Style) {
    if !is_alive {
        (" \u{25cb} DEAD ", tone(is_selected, base, ERR))
    } else if let Some((st, _)) = node_st {
        match st.state {
            hypha_node::sdk::msgs::hypha_msgs::NodeState::INPUTS_USABLE => {
                (" \u{25cf} OK ", tone_bold(is_selected, base, OK))
            }
            hypha_node::sdk::msgs::hypha_msgs::NodeState::WAITING => {
                (" \u{25b2} WAIT ", tone_bold(is_selected, base, WARN))
            }
            hypha_node::sdk::msgs::hypha_msgs::NodeState::DEGRADED => {
                (" \u{2716} DEGR ", tone_bold(is_selected, base, ERR))
            }
            _ => (" \u{25cf} LIVE ", tone(is_selected, base, OK)),
        }
    } else if intro.is_some() {
        (" \u{25cf} ACTIVE ", tone(is_selected, base, OK))
    } else {
        (" \u{25cf} LIVE ", tone(is_selected, base, OK))
    }
}

fn format_node_inputs(
    node_st: Option<&(hypha_node::sdk::msgs::hypha_msgs::NodeState, Instant)>,
    intro: Option<&hypha_node::sdk::msgs::hypha_msgs::NodeIntrospection>,
    app: &AppState,
    is_selected: bool,
    base: Style,
) -> (String, Style) {
    if let Some((st, _)) = node_st {
        let total = st.inputs.len();
        if total == 0 {
            return ("-".to_string(), tone(is_selected, base, MUTED));
        }
        let usable = st
            .inputs
            .iter()
            .filter(|i| i.verdict == hypha_node::sdk::msgs::hypha_msgs::RequiredInput::USABLE)
            .count();
        let stale = st
            .inputs
            .iter()
            .filter(|i| i.verdict == hypha_node::sdk::msgs::hypha_msgs::RequiredInput::STALE)
            .count();
        let wait = st
            .inputs
            .iter()
            .filter(|i| {
                i.verdict == hypha_node::sdk::msgs::hypha_msgs::RequiredInput::NEVER_ARRIVED
            })
            .count();

        // An input whose own stream has no live publisher while another version has: the
        // mismatch a migration must not leave behind, whatever the input's verdict.
        let other_version = st
            .inputs
            .iter()
            .filter(|i| !i.other_schemas.is_empty() && !app.has_publisher(&i.topic, &i.md5))
            .count();
        let version = if other_version > 0 {
            format!(", {other_version} OTHER VERSION")
        } else {
            String::new()
        };

        if stale > 0 {
            (
                format!("{usable}/{total} ({stale} STALE{version})"),
                tone_bold(is_selected, base, ERR),
            )
        } else if wait > 0 {
            (format!("{usable}/{total} ({wait} WAIT{version})"), tone_bold(is_selected, base, WARN))
        } else if other_version > 0 {
            (format!("{usable}/{total} OK{version}"), tone_bold(is_selected, base, ERR))
        } else {
            (format!("{usable}/{total} OK"), tone(is_selected, base, OK))
        }
    } else if let Some(intro) = intro {
        let req_count = intro.input_bindings.iter().filter(|i| i.required).count();
        let opt_count = intro.input_bindings.iter().filter(|i| !i.required).count();
        match (req_count, opt_count) {
            (0, 0) => ("-".to_string(), tone(is_selected, base, MUTED)),
            (r, 0) => (format!("{r} req"), tone(is_selected, base, FG)),
            (0, o) => (format!("{o} opt"), tone(is_selected, base, MUTED)),
            (r, o) => (format!("{r} req, {o} opt"), tone(is_selected, base, FG)),
        }
    } else {
        ("-".to_string(), tone(is_selected, base, MUTED))
    }
}

fn draw_nodes_list_panel(
    f: &mut Frame<'_>,
    area: Rect,
    state: &AppState,
    sorted_nodes: &[&NodeInfo],
    table_state: &mut TableState,
) {
    let header = header_row(&[
        "STATUS",
        "NODE NAME",
        "PUBS",
        "SUBS",
        "CYCLES",
        "INPUTS",
        "FAILS",
        "CPU%",
        "MEM",
        "PID",
    ]);

    let selected = state.selected_node;
    let rows = sorted_nodes.iter().enumerate().map(|(i, node)| {
        let is_selected = i == selected;
        let base = row_base(is_selected);

        let node_st = state.node_state(&node.name);
        let intro = state.node_introspection(&node.name);
        let usage = if node.is_alive { state.proc_usage.usage(node.pid) } else { None };
        let (status_text, status_style) =
            format_node_status(node.is_alive, node_st, intro, is_selected, base);
        let name_display = if node.name.is_empty() { "<unnamed>" } else { node.name.as_str() };
        let (inputs_text, inputs_style) =
            format_node_inputs(node_st, intro, state, is_selected, base);

        let pubs_text = intro.map_or_else(
            || state.node_scoped_topics(&node.name).len().to_string(),
            |i| i.publishers.len().to_string(),
        );
        let subs_text = intro.map_or_else(|| "-".to_string(), |i| i.subscribers.len().to_string());
        let cycles_text = intro.map_or_else(|| "-".to_string(), |i| i.cycles.len().to_string());

        let (fails_text, fails_style) = if let Some((st, _)) = node_st {
            let fails = st.callback_failures;
            let style = if fails > 0 {
                tone_bold(is_selected, base, ERR)
            } else {
                tone(is_selected, base, MUTED)
            };
            (fails.to_string(), style)
        } else {
            ("-".to_string(), tone(is_selected, base, MUTED))
        };

        Row::new(vec![
            Cell::from(status_text).style(status_style),
            Cell::from(name_display.to_string()).style(tone_bold(is_selected, base, FG)),
            Cell::from(pubs_text).style(tone(is_selected, base, OK)),
            Cell::from(subs_text).style(tone(is_selected, base, ACCENT)),
            Cell::from(cycles_text).style(tone(is_selected, base, WARN)),
            Cell::from(inputs_text).style(inputs_style),
            Cell::from(fails_text).style(fails_style),
            cpu_cell(usage, is_selected, base),
            Cell::from(usage.map_or_else(|| "-".to_owned(), |u| format_memory(u.rss_bytes)))
                .style(tone(is_selected, base, ACCENT2)),
            Cell::from(node.pid.to_string()).style(tone(is_selected, base, WARN)),
        ])
        .height(1)
    });

    let title = if sorted_nodes.is_empty() {
        " Node Process Registry (0 active) ".to_string()
    } else {
        format!(" Node Process Registry ({}) ", sorted_nodes.len())
    };

    let table = Table::new(
        rows,
        [
            Constraint::Length(9),
            Constraint::Min(12),
            Constraint::Length(5),
            Constraint::Length(5),
            Constraint::Length(7),
            Constraint::Length(9),
            Constraint::Length(6),
            Constraint::Length(6),
            Constraint::Length(7),
            Constraint::Length(7),
        ],
    )
    .header(header)
    .block(muted_block().title(title));
    f.render_stateful_widget(table, area, table_state);
}

/// CPU of a node as percent of one core: `…` until two samples, `-` when the
/// process cannot be read (dead, or another user's).
fn cpu_cell(usage: Option<Usage>, is_selected: bool, base: Style) -> Cell<'static> {
    let Some(usage) = usage else {
        return Cell::from("-").style(tone(is_selected, base, MUTED));
    };
    let Some(percent) = usage.cpu_percent else {
        return Cell::from("…").style(tone(is_selected, base, MUTED));
    };
    let color = if percent >= 80.0 {
        ERR
    } else if percent >= 40.0 {
        WARN
    } else {
        OK
    };
    Cell::from(format!("{percent:.1}")).style(tone(is_selected, base, color))
}

#[allow(clippy::too_many_lines)]
fn draw_node_details_panel(
    f: &mut Frame<'_>,
    area: Rect,
    state: &AppState,
    sorted_nodes: &[&NodeInfo],
) {
    let Some(sel) = sorted_nodes.get(state.selected_node) else {
        let empty = Paragraph::new(vec![Line::styled(
            " (No active nodes registered on the bus) ",
            Style::default().fg(MUTED),
        )])
        .block(muted_block().title(" Node Diagnostics "));
        f.render_widget(empty, area);
        return;
    };

    let node_st = state.node_state(&sel.name);
    let node_intro = state.node_introspection(&sel.name);
    let scoped = state.node_scoped_topics(&sel.name);

    if let Some(intro) = node_intro {
        let vertical = Layout::default().direction(Direction::Vertical);
        if intro.input_bindings.is_empty() {
            let [card, endpoints] =
                vertical.constraints([Constraint::Length(7), Constraint::Min(3)]).areas(area);
            draw_node_summary_card(f, card, sel, node_st, Some(intro));
            draw_node_endpoints_table(f, endpoints, intro, state);
        } else {
            let [card, bindings, endpoints] = vertical
                .constraints([
                    Constraint::Length(7),
                    Constraint::Percentage(45),
                    Constraint::Percentage(55),
                ])
                .areas(area);
            draw_node_summary_card(f, card, sel, node_st, Some(intro));
            draw_node_bindings_table(f, bindings, intro, node_st, state);
            draw_node_endpoints_table(f, endpoints, intro, state);
        }
    } else {
        let [card, topics] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(6), Constraint::Min(3)])
            .areas(area);

        draw_node_summary_card(f, card, sel, node_st, None);
        draw_node_scoped_topics_table(f, topics, &sel.name, &scoped);
    }
}

#[allow(clippy::too_many_lines)]
fn draw_node_summary_card(
    f: &mut Frame<'_>,
    area: Rect,
    node: &NodeInfo,
    node_st: Option<&(hypha_node::sdk::msgs::hypha_msgs::NodeState, Instant)>,
    node_intro: Option<&hypha_node::sdk::msgs::hypha_msgs::NodeIntrospection>,
) {
    let name_display = if node.name.is_empty() { "<unnamed>" } else { node.name.as_str() };
    let exe_display = if node.executable.is_empty() { "<none>" } else { node.executable.as_str() };

    let alive_span = if node.is_alive {
        Span::styled(" [● ALIVE] ", Style::default().fg(OK).add_modifier(Modifier::BOLD))
    } else {
        Span::styled(" [○ DEAD] ", Style::default().fg(ERR).add_modifier(Modifier::BOLD))
    };

    let mut line0_spans = vec![
        Span::styled(" Node: ", Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(name_display, Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
        Span::raw("   PID: "),
        Span::styled(node.pid.to_string(), Style::default().fg(WARN).add_modifier(Modifier::BOLD)),
        Span::raw("   Exe: "),
        Span::styled(exe_display, Style::default().fg(FG)),
    ];
    if let Some(intro) = node_intro {
        line0_spans.push(Span::raw("   Domain: "));
        line0_spans.push(Span::styled(intro.domain.clone(), Style::default().fg(ACCENT2)));
    }
    line0_spans.push(Span::raw("   Status: "));
    line0_spans.push(alive_span);

    let mut lines = Vec::new();
    lines.push(Line::from(line0_spans));

    if let Some(intro) = node_intro {
        lines.push(Line::from(vec![
            Span::styled(" Blueprint: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(format!("hypha {}", intro.hypha), Style::default().fg(ACCENT)),
            Span::raw("   Started: "),
            Span::styled(intro.created.clone(), Style::default().fg(FG)),
            Span::raw("   Pubs: "),
            Span::styled(
                intro.publishers.len().to_string(),
                Style::default().fg(OK).add_modifier(Modifier::BOLD),
            ),
            Span::raw("   Subs: "),
            Span::styled(
                intro.subscribers.len().to_string(),
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::raw("   Cycles: "),
            Span::styled(
                intro.cycles.len().to_string(),
                Style::default().fg(WARN).add_modifier(Modifier::BOLD),
            ),
        ]));
    }

    if let Some((st, last_seen)) = node_st {
        let (badge, badge_style) = match st.state {
            hypha_node::sdk::msgs::hypha_msgs::NodeState::INPUTS_USABLE => {
                ("[● ALL INPUTS USABLE]", Style::default().fg(OK).add_modifier(Modifier::BOLD))
            }
            hypha_node::sdk::msgs::hypha_msgs::NodeState::WAITING => {
                ("[▲ WAITING FOR INPUTS]", Style::default().fg(WARN).add_modifier(Modifier::BOLD))
            }
            hypha_node::sdk::msgs::hypha_msgs::NodeState::DEGRADED => (
                "[✖ DEGRADED / STALE INPUTS]",
                Style::default().fg(ERR).add_modifier(Modifier::BOLD),
            ),
            _ => ("[UNKNOWN STATE]", Style::default().fg(MUTED)),
        };

        let age_sec = last_seen.elapsed().as_secs_f64();
        lines.push(Line::from(vec![
            Span::styled(" Health: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(badge, badge_style),
            Span::raw("   Heartbeat: "),
            Span::styled(format!("{age_sec:.1}s ago"), Style::default().fg(ACCENT2)),
            Span::raw("   Monitored: "),
            Span::styled(format!("{} inputs", st.inputs.len()), Style::default().fg(FG)),
        ]));

        let fail_style = if st.callback_failures > 0 {
            Style::default().fg(ERR).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(FG)
        };
        let slow_style = if st.slow_callbacks > 0 {
            Style::default().fg(WARN).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(FG)
        };

        lines.push(Line::from(vec![
            Span::styled(" Execution: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(format!("{} callbacks", st.callbacks_invoked), Style::default().fg(FG)),
            Span::raw(" | "),
            Span::styled(format!("{} failures", st.callback_failures), fail_style),
            Span::raw(" | "),
            Span::styled(format!("{} slow", st.slow_callbacks), slow_style),
        ]));
        for cycle in &st.cycles {
            let cycle_style = if cycle.overruns > 0 || cycle.output_drops > 0 {
                Style::default().fg(WARN)
            } else {
                Style::default().fg(FG)
            };
            lines.push(Line::from(vec![
                Span::styled(" Cycle ", Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(cycle_line_text(cycle), cycle_style),
            ]));
        }
        let log_faults = st.log_lines_dropped > 0 || st.log_write_failures > 0;
        let log_style =
            if log_faults { Style::default().fg(WARN) } else { Style::default().fg(FG) };
        lines.push(Line::from(vec![
            Span::styled(" Log: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(
                format!(
                    "{} lines dropped, {} write failures",
                    st.log_lines_dropped, st.log_write_failures
                ),
                log_style,
            ),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::styled(" Health: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(
                "(Waiting for /<node>/state diagnostic heartbeat)",
                Style::default().fg(MUTED),
            ),
        ]));
    }

    lines.push(Line::styled(
        " Hint: Press [Enter] to inspect /<node>/introspection or /<node>/state payload",
        Style::default().fg(MUTED),
    ));

    let block =
        Paragraph::new(lines).block(muted_block().title(" Node Blueprint & Health Diagnostics "));
    f.render_widget(block, area);
}

/// The bus rate and bandwidth of the stream of `topic` and `md5` as text; zero when no
/// traffic was seen. A stream with no MD5 (an untyped endpoint) is the topic's first.
fn bus_rates(state: &AppState, topic: &str, md5: &str) -> (String, String) {
    let seen =
        if md5.is_empty() { state.topics.get(topic) } else { state.topics.stream(topic, md5) };
    match seen {
        Some(seen) => (format_rate(seen.rate_hz), format_bandwidth(seen.bandwidth_kbs)),
        None => ("0.0 Hz".to_string(), "0.0 KB/s".to_string()),
    }
}

/// The type of an introspected endpoint and the start of its MD5; `?` for an untyped one.
fn endpoint_type(
    endpoint: &hypha_node::sdk::msgs::hypha_msgs::IntrospectedEndpoint,
    width: usize,
) -> String {
    if endpoint.data_type.is_empty() {
        return "?".to_owned();
    }
    fit_type_label(&endpoint.data_type, &endpoint.md5, width)
}

/// One publisher or subscriber row of the endpoints table.
fn endpoint_row(
    state: &AppState,
    tag: Cell<'static>,
    endpoint: &hypha_node::sdk::msgs::hypha_msgs::IntrospectedEndpoint,
    policy: String,
    type_width: usize,
) -> Row<'static> {
    let (hz_str, bw_str) = bus_rates(state, &endpoint.topic, &endpoint.md5);
    Row::new(vec![
        tag,
        Cell::from(endpoint.topic.clone()).style(Style::default().fg(FG)),
        Cell::from(endpoint_type(endpoint, type_width)).style(Style::default().fg(OK)),
        Cell::from(hz_str).style(Style::default().fg(ACCENT2)),
        Cell::from(bw_str).style(Style::default().fg(WARN)),
        Cell::from(policy).style(Style::default().fg(MUTED)),
    ])
    .height(1)
}

fn draw_node_endpoints_table(
    f: &mut Frame<'_>,
    area: Rect,
    intro: &hypha_node::sdk::msgs::hypha_msgs::NodeIntrospection,
    state: &AppState,
) {
    let header = header_row(&["DIR", "TOPIC", "TYPE", "BUS HZ", "BW", "POLICY"]);
    let type_width = column_width(area, 25);

    let pub_rows = intro.publishers.iter().map(|p| {
        let policy = if p.latched {
            "latched".to_string()
        } else if !p.output_cycle.is_empty() {
            format!("cycle {}", p.output_cycle)
        } else {
            "default".to_string()
        };
        let tag = Cell::from("[PUB]").style(Style::default().fg(OK).add_modifier(Modifier::BOLD));
        endpoint_row(state, tag, p, policy, type_width)
    });

    let sub_rows = intro.subscribers.iter().map(|s| {
        let tag =
            Cell::from("[SUB]").style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD));
        endpoint_row(state, tag, s, format!("queue {}", s.queue_depth), type_width)
    });

    let all_rows: Vec<Row<'_>> = pub_rows.chain(sub_rows).collect();
    let title = format!(
        " Introspected Endpoints ({} pubs, {} subs) ",
        intro.publishers.len(),
        intro.subscribers.len()
    );

    let table = Table::new(
        all_rows,
        [
            Constraint::Length(7),
            Constraint::Percentage(36),
            Constraint::Percentage(25),
            Constraint::Percentage(11),
            Constraint::Percentage(11),
            Constraint::Percentage(10),
        ],
    )
    .header(header)
    .block(muted_block().title(title));
    f.render_widget(table, area);
}

/// The samples an input never delivered, the frames its subscriber refused and those
/// it could not decode: `dropped/rejected/undecodable`.
fn input_counters_text(input: &hypha_node::sdk::msgs::hypha_msgs::RequiredInput) -> String {
    format!("{}/{}/{}", input.dropped, input.rejected, input.undecodable)
}

/// The words that follow a verdict: ` STAGNANT` and ` IGNORED` when they hold.
fn input_flags_text(input: &hypha_node::sdk::msgs::hypha_msgs::RequiredInput) -> String {
    let mut flags = String::new();
    if input.stagnant {
        flags.push_str(" STAGNANT");
    }
    if input.ignored {
        flags.push_str(" IGNORED");
    }
    flags
}

/// One detail line per cycle: steps, missed periods, last and max step in ms,
/// overruns and output drops.
fn cycle_line_text(cycle: &hypha_node::sdk::msgs::hypha_msgs::CycleState) -> String {
    format!(
        "{}: {} steps, {} missed, step {:.2}/{:.2} ms (last/max), {} overruns, {} output drops",
        cycle.name,
        cycle.steps,
        cycle.missed_periods,
        approx_f64(cycle.last_step_nanos) / 1_000_000.0,
        approx_f64(cycle.max_step_nanos) / 1_000_000.0,
        cycle.overruns,
        cycle.output_drops
    )
}

fn format_input_verdict(
    verdict_info: Option<&hypha_node::sdk::msgs::hypha_msgs::RequiredInput>,
    required: bool,
) -> (String, Style) {
    let (text, style) = format_input_verdict_word(verdict_info, required);
    let flags = verdict_info.map_or_else(String::new, input_flags_text);
    (format!("{text}{flags}"), style)
}

fn format_input_verdict_word(
    verdict_info: Option<&hypha_node::sdk::msgs::hypha_msgs::RequiredInput>,
    required: bool,
) -> (String, Style) {
    if let Some(v) = verdict_info {
        match v.verdict {
            hypha_node::sdk::msgs::hypha_msgs::RequiredInput::USABLE => {
                ("● USABLE".to_string(), Style::default().fg(OK).add_modifier(Modifier::BOLD))
            }
            hypha_node::sdk::msgs::hypha_msgs::RequiredInput::NEVER_ARRIVED => {
                ("○ WAITING".to_string(), Style::default().fg(WARN).add_modifier(Modifier::BOLD))
            }
            hypha_node::sdk::msgs::hypha_msgs::RequiredInput::STALE => {
                if v.age_nanos > 0 {
                    (
                        format!("✖ STALE ({:.1}s)", approx_f64(v.age_nanos) / 1_000_000_000.0),
                        Style::default().fg(ERR).add_modifier(Modifier::BOLD),
                    )
                } else {
                    ("✖ STALE".to_string(), Style::default().fg(ERR).add_modifier(Modifier::BOLD))
                }
            }
            _ => ("?".to_string(), Style::default().fg(MUTED)),
        }
    } else if !required {
        ("○ OPTIONAL".to_string(), Style::default().fg(MUTED))
    } else {
        ("○ WAITING".to_string(), Style::default().fg(WARN))
    }
}

fn format_input_policy(input: &hypha_node::sdk::msgs::hypha_msgs::IntrospectedInput) -> String {
    let mut policy_parts = Vec::new();
    if input.mode == 0 {
        policy_parts.push("latest".to_string());
    } else {
        policy_parts.push(format!("queue max {}", input.max_samples_per_iteration));
    }
    // A wired input without a silence limit is shown as such, never as a healthy default.
    if input.silence_nanos > 0 {
        policy_parts.push(format!("silence {}ms", input.silence_nanos / 1_000_000));
    } else if !input.topic.is_empty() {
        policy_parts.push("no time limit".to_string());
    }
    if input.stagnation_nanos > 0 {
        policy_parts.push(format!("stagnation {}ms", input.stagnation_nanos / 1_000_000));
    }
    policy_parts.join(", ")
}

/// The verdict row of the input that `binding` names: the one of its topic and MD5, so the
/// inputs of two versions on one topic are judged apart. A binding with no endpoint row, so no
/// MD5, is found by its topic alone only when one input has that topic: with two, none is
/// its own.
fn verdict_of<'a>(
    state: &'a hypha_node::sdk::msgs::hypha_msgs::NodeState,
    binding: &hypha_node::sdk::msgs::hypha_msgs::IntrospectedInput,
    md5: &str,
) -> Option<&'a hypha_node::sdk::msgs::hypha_msgs::RequiredInput> {
    let mut on_topic = state.inputs.iter().filter(|input| input.topic == binding.topic);
    if md5.is_empty() {
        let only = on_topic.next()?;
        return on_topic.next().is_none().then_some(only);
    }
    on_topic.find(|input| input.md5 == md5)
}

/// The other versions of an input's topic that a live publisher sends, as its node last
/// found them: `not checked` before the node's first check, `none`, or each type with the
/// start of its MD5. Red when it lists a version and the input's own stream has no live
/// publisher, whatever the verdict (an input without a limit stays `Usable` on its last old
/// value); yellow when the input's own stream is published as well, as in a migration.
fn other_schemas_cell(
    verdict: Option<&hypha_node::sdk::msgs::hypha_msgs::RequiredInput>,
    own_is_published: bool,
) -> (String, Style) {
    let Some(verdict) = verdict else {
        return ("-".to_owned(), Style::default().fg(MUTED));
    };
    if !verdict.other_schemas_checked {
        return ("not checked".to_owned(), Style::default().fg(MUTED));
    }
    if verdict.other_schemas.is_empty() {
        return ("none".to_owned(), Style::default().fg(MUTED));
    }
    let versions: Vec<String> = verdict
        .other_schemas
        .iter()
        .map(|schema| {
            let known = (!schema.data_type.is_empty() && !schema.md5.is_empty()).then(|| {
                hypha_node::sdk::ResolvedSchema {
                    data_type: schema.data_type.clone(),
                    md5: schema.md5.clone(),
                    definition: String::new(),
                }
            });
            stream_label(known.as_ref(), schema.schema_hash)
        })
        .collect();
    let style = Style::default().fg(if own_is_published { WARN } else { ERR });
    (versions.join(", "), style)
}

fn draw_node_bindings_table(
    f: &mut Frame<'_>,
    area: Rect,
    intro: &hypha_node::sdk::msgs::hypha_msgs::NodeIntrospection,
    node_st: Option<&(hypha_node::sdk::msgs::hypha_msgs::NodeState, Instant)>,
    state: &AppState,
) {
    let header = header_row(&[
        "ROLE",
        "VERDICT",
        "CYCLE",
        "INPUT TOPIC",
        "TYPE",
        "BUS HZ",
        "BUS BW",
        "DROP/REJ/UNDEC",
        "OTHER SCHEMAS",
        "POLICY / SUPERVISION",
    ]);
    let type_width = column_width(area, 16);

    let rows = intro.input_bindings.iter().map(|input| {
        let (role_text, role_style) = if input.required {
            ("REQUIRED", Style::default().fg(FG).add_modifier(Modifier::BOLD))
        } else {
            ("OPTIONAL", Style::default().fg(MUTED))
        };

        let endpoint = intro.subscribers.iter().find(|row| row.id == input.subscriber_id);
        let md5 = endpoint.map_or("", |row| row.md5.as_str());
        let verdict_info = node_st.and_then(|(st, _)| verdict_of(st, input, md5));
        let (verdict_text, verdict_style) = format_input_verdict(verdict_info, input.required);
        let own_md5 = verdict_info.map_or(md5, |verdict| verdict.md5.as_str());
        let (others_text, others_style) =
            other_schemas_cell(verdict_info, state.has_publisher(&input.topic, own_md5));

        let (hz_str, bw_str) = bus_rates(state, &input.topic, md5);

        let policy_str = format_input_policy(input);
        let counters = verdict_info.map_or_else(|| "-".to_owned(), input_counters_text);
        let type_text =
            endpoint.map_or_else(|| "?".to_owned(), |row| endpoint_type(row, type_width));

        Row::new(vec![
            Cell::from(role_text).style(role_style),
            Cell::from(verdict_text).style(verdict_style),
            Cell::from(input.cycle.clone()).style(Style::default().fg(WARN)),
            Cell::from(input.topic.clone()).style(Style::default().fg(FG)),
            Cell::from(type_text).style(Style::default().fg(OK)),
            Cell::from(hz_str).style(Style::default().fg(ACCENT2)),
            Cell::from(bw_str).style(Style::default().fg(WARN)),
            Cell::from(counters).style(Style::default().fg(FG)),
            Cell::from(others_text).style(others_style),
            Cell::from(policy_str).style(Style::default().fg(MUTED)),
        ])
        .height(1)
    });

    let title = format!(" Cycle Input Bindings ({}) ", intro.input_bindings.len());
    let table = Table::new(
        rows,
        [
            Constraint::Length(10),
            Constraint::Length(14),
            Constraint::Length(12),
            Constraint::Percentage(16),
            Constraint::Percentage(16),
            Constraint::Percentage(6),
            Constraint::Percentage(6),
            Constraint::Length(15),
            Constraint::Percentage(14),
            Constraint::Percentage(14),
        ],
    )
    .header(header)
    .block(muted_block().title(title));
    f.render_widget(table, area);
}

fn draw_node_scoped_topics_table(
    f: &mut Frame<'_>,
    area: Rect,
    node_name: &str,
    scoped: &[(&StreamKey, &crate::top::state::TopicStats)],
) {
    let header = header_row(&["TOPIC", "TYPE", "RATE (Hz)", "BW"]);
    let type_width = column_width(area, 31);

    let rows = scoped.iter().map(|(stream, stats)| {
        Row::new(vec![
            Cell::from(stream.topic.clone()).style(Style::default().fg(FG)),
            Cell::from(type_cell(stats, type_width)).style(Style::default().fg(OK)),
            Cell::from(format_rate(stats.rate_hz)).style(Style::default().fg(ACCENT2)),
            Cell::from(format_bandwidth(stats.bandwidth_kbs)).style(Style::default().fg(WARN)),
        ])
        .height(1)
    });

    let title = if scoped.is_empty() {
        format!(" Node Topics (0 published under /{node_name}/*) ")
    } else {
        format!(" Node Topics ({} under /{node_name}/*) ", scoped.len())
    };

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(45),
            Constraint::Percentage(31),
            Constraint::Percentage(12),
            Constraint::Percentage(12),
        ],
    )
    .header(header)
    .block(muted_block().title(title));
    f.render_widget(table, area);
}

fn draw_help_tab(f: &mut Frame<'_>, area: Rect) {
    let lines = vec![
        Line::styled(
            " hypha top: help & shortcuts ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
        help_line("[1] .. [5]", "Switch between Topics, Nodes, Intercept, Graph, Help tabs"),
        help_line("[\u{2191}] / [\u{2193}] / [k] / [j]", "Navigate topic/node selection"),
        help_line("[Enter]", "Open live reflected Message Inspector for selected topic or node"),
        help_line("[/]", "Search / filter topics dynamically"),
        help_line("[s]", "Cycle sort order (Name -> Bandwidth -> Rate -> Messages)"),
        help_line("[PgUp/PgDn/Home/End]", "Page through topic or node lists; jump to the ends"),
        help_line("[Space]", "Pause / Resume live rate calculations"),
        help_line("[Esc]", "Clear search filter"),
        help_line("[q] / [Ctrl+C]", "Quit hypha top cleanly"),
        Line::raw(""),
        Line::styled(
            " Intercept tab (drives a running hypha intercept): ",
            Style::default().fg(FG).add_modifier(Modifier::BOLD),
        ),
        help_line("[Tab] / [Shift+Tab]", "Next / previous pane: list, fields, override"),
        help_line("[\u{2191}/\u{2193}] / [j/k]", "List: select interception; fields: move"),
        help_line("[\u{2190}/\u{2192}] / [h/l]", "Fields: collapse / expand; override: move bit"),
        help_line("[Enter]", "List: to fields; fields: edit; override: apply value"),
        help_line("[Esc]", "Override: cancel the edit; fields: back to the list"),
        help_line("[Space]", "Override: bit keep -> set -> clear; bool: toggle"),
        help_line("[x] / [X]", "Fields: clear the field's override / all overrides"),
        help_line("[Ctrl+X]", "Override: clear the field's override (x types text)"),
        help_line("[a]", "List or fields: arm / bypass the interception"),
        help_line("[n]", "List: new interception (topic hints: arrows, Enter picks)"),
        help_line("[d] / [Del] then [y]", "List or fields: delete the interception"),
        Line::raw(""),
        Line::styled(
            " Graph tab (who publishes to whom, from the node manifests): ",
            Style::default().fg(FG).add_modifier(Modifier::BOLD),
        ),
        help_line("[\u{2191}/\u{2193}] / [j/k]", "Select a node or topic"),
        help_line("[\u{2190}/\u{2192}] [PgUp/PgDn]", "Pan the graph"),
        help_line("[g] / [i]", "Nodes or nodes+topics view; show infrastructure topics"),
        help_line("[/]", "Show only matching vertices and their neighbours"),
        help_line("[e]", "Write the graph to hypha_graph.dot (Graphviz)"),
        help_line("[Enter]", "Open the selected node or topic in its tab"),
        Line::raw(""),
        Line::styled(
            " Zero-Copy Architecture: ",
            Style::default().fg(FG).add_modifier(Modifier::BOLD),
        ),
        Line::styled(
            "  - Reads the hypha shared-memory bus of this machine.",
            Style::default().fg(MUTED),
        ),
        Line::styled(
            "  - Microsecond latency with zero heap serialization on pub/sub.",
            Style::default().fg(MUTED),
        ),
        Line::styled(
            "  - Dynamic schema reflection enabled for all ROS 1 and custom messages.",
            Style::default().fg(MUTED),
        ),
    ];
    f.render_widget(Paragraph::new(lines).block(muted_block()), area);
}

fn help_line<'a>(keys: &'a str, desc: &'a str) -> Line<'a> {
    Line::from(vec![
        Span::styled(
            format!("  {keys:<24} "),
            Style::default().fg(WARN).add_modifier(Modifier::BOLD),
        ),
        Span::raw(desc),
    ])
}

#[allow(clippy::too_many_lines)]
fn draw_inspector_modal(
    f: &mut Frame<'_>,
    area: Rect,
    app_state: &AppState,
    nodes: &[NodeInfo],
    bus: &BusNode,
    schema_cache: &mut SchemaCache,
) {
    let sorted = sorted_nodes(nodes);
    let target = match app_state.active_tab {
        Tab::TopicsBus => {
            let visible = app_state.visible_topics();
            visible.get(app_state.selected_topic).copied()
        }
        Tab::NodeRegistry => {
            if let Some(sel) = sorted.get(app_state.selected_node) {
                let intro_topic = format!("/{}/introspection", sel.name);
                let state_topic = format!("/{}/state", sel.name);
                if let Some(found) = app_state.topics.streams_of(&intro_topic).next() {
                    Some(found)
                } else if let Some(found) = app_state.topics.streams_of(&state_topic).next() {
                    Some(found)
                } else {
                    let scoped = app_state.node_scoped_topics(&sel.name);
                    scoped.first().copied()
                }
            } else {
                None
            }
        }
        Tab::Intercept | Tab::Graph | Tab::Help => None,
    };

    let Some((stream, stats)) = target else {
        let modal_area = centered_rect(60, 20, area);
        f.render_widget(Clear, modal_area);
        let lines = vec![
            Line::raw(""),
            Line::styled(
                " (No topics or introspection blueprint recorded for this selection yet) ",
                Style::default().fg(WARN),
            ),
            Line::raw(""),
            Line::styled(" Press [Esc] / [Enter] / [q] to return ", Style::default().fg(MUTED)),
        ];
        let modal = Paragraph::new(lines)
            .block(muted_block().title(" Message Inspector ").style(Style::default().bg(BG)));
        f.render_widget(modal, modal_area);
        return;
    };

    let modal_area = centered_rect(70, 70, area);
    f.render_widget(Clear, modal_area);

    let mut lines: Vec<Line<'_>> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled(" Topic: ", Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(
            stream.topic.clone(),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  Type: "),
        Span::styled(
            type_cell(stats, usize::MAX),
            Style::default().fg(OK).add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::raw(""));

    if stats.last_payload.is_empty() {
        lines.push(Line::styled(
            " (Waiting for first message payload on bus...) ",
            Style::default().fg(WARN),
        ));
    } else {
        match schema_cache.get(bus, &stream.topic, &stats.data_type, &stats.md5) {
            None => {
                lines.push(Line::styled(
                    " (Schema not yet resolved -- waiting for beacon) ",
                    Style::default().fg(WARN),
                ));
                lines
                    .push(Line::raw(format!(" Payload bytes: {} bytes", stats.last_payload.len())));
            }
            Some(Err(cause)) => {
                lines.push(Line::styled(
                    format!(" (Schema could not be parsed: {cause}) "),
                    Style::default().fg(WARN),
                ));
            }
            Some(Ok(schema)) => {
                for line in decode_lines(&stats.data_type, schema, &stats.last_payload) {
                    lines.push(Line::styled(format!("  {line}"), Style::default().fg(FG)));
                }
            }
        }
    }

    lines.push(Line::raw(""));
    let active_publisher_count = nodes.iter().filter(|n| n.is_alive).count();
    lines.push(Line::styled(
        format!(" {active_publisher_count} node(s) currently registered on the bus."),
        Style::default().fg(MUTED),
    ));
    lines.push(Line::raw(""));
    lines.push(Line::styled(" Press [Esc] / [Enter] / [q] to return ", Style::default().fg(MUTED)));

    let modal = Paragraph::new(lines).wrap(Wrap { trim: false }).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(ACCENT))
            .title(format!(" Message Inspector: {} ", stream.topic))
            .title_style(Style::default().fg(FG).add_modifier(Modifier::BOLD))
            .style(Style::default().bg(BG)),
    );
    f.render_widget(modal, modal_area);
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let margin_y = 100_u16.saturating_sub(percent_y) / 2;
    let margin_x = 100_u16.saturating_sub(percent_x) / 2;
    let [_, middle, _] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(margin_y),
            Constraint::Percentage(percent_y),
            Constraint::Percentage(margin_y),
        ])
        .areas(area);
    let [_, center, _] = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(margin_x),
            Constraint::Percentage(percent_x),
            Constraint::Percentage(margin_x),
        ])
        .areas(middle);
    center
}

#[cfg(test)]
#[path = "ui_tests.rs"]
mod tests;
