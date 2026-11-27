//! Drawing the Intercept tab. The layout is derived from the size each frame;
//! nothing layout-dependent is stored.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::time::Instant;

use hypha_tools::intercept_doc::{OverrideSpec, RelayState};
use hypha_tools::msg_patch::{
    Radix, ScalarValue, format_value, index_slots, integer_width, read_scalar,
};
use hypha_tools::msg_reflect::MessageSchema;
use hypha_tools::theme::{ACCENT, ACCENT2, ERR, FG, MUTED, OK, ON_ACCENT, SURFACE, WARN};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout as Split, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::top::intercept::{
    BitState, FormField, InterceptView, Layout, NewForm, Pane, RowKind, StatusLine, TreeRow,
    build_rows, byte_index, suggestions,
};
use crate::top::state::Topics;

/// Bits per grid row when the editor is narrower than this.
const WIDE_GRID: u16 = 56;
/// The list and details column of the wide layout.
const LEFT_WIDTH: u16 = 40;

/// The input schema, as the caller resolved it: `None` while unknown.
pub(crate) type InputSchema<'a> = Option<Result<&'a mut MessageSchema, &'a str>>;

/// The notice shown instead of the tab below 60×12.
pub(crate) fn too_small_text(width: u16, height: u16) -> String {
    format!("terminal too small: need 60×12, have {width}×{height}")
}

pub(crate) fn draw_too_small(f: &mut Frame<'_>, area: Rect) {
    let text = too_small_text(area.width, area.height);
    f.render_widget(
        Paragraph::new(text).style(Style::default().fg(WARN)).wrap(Wrap { trim: true }),
        area,
    );
}

/// `text` cut to `width` columns by dropping its middle: `cells[…ltage`.
pub(crate) fn truncate_middle(text: &str, width: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= width {
        return text.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    let kept = width.saturating_sub(1);
    let tail = kept / 2;
    let head = kept.saturating_sub(tail);
    let start: String = chars.iter().take(head).collect();
    let end: String = chars.iter().skip(chars.len().saturating_sub(tail)).collect();
    format!("{start}…{end}")
}

/// `text` cut to `width` columns by dropping its start: the newest part of an error stays.
fn truncate_left(text: &str, width: usize) -> String {
    let count = text.chars().count();
    if count <= width {
        return text.to_owned();
    }
    let tail: String = text.chars().skip(count.saturating_sub(width.saturating_sub(1))).collect();
    format!("…{tail}")
}

/// The tree columns that fit `width`, dropped right to left: name and ovr always.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Columns {
    pub out: bool,
    pub input: bool,
    pub type_name: bool,
}

const NAME_MIN: usize = 16;
const OVR: usize = 4;
const VALUE: usize = 15;
const TYPE: usize = 17;

pub(crate) fn columns_for(width: usize) -> Columns {
    let mut used = NAME_MIN.saturating_add(OVR);
    let mut fits = |cost: usize| {
        let next = used.saturating_add(cost);
        let fit = next <= width;
        if fit {
            used = next;
        }
        fit
    };
    let out = fits(VALUE);
    let input = out && fits(VALUE);
    let type_name = input && fits(TYPE);
    Columns { out, input, type_name }
}

/// Bits per row of the grid for an editor of `width` columns.
pub(crate) fn bits_per_row(width: u16) -> usize {
    if width < WIDE_GRID { 8 } else { 16 }
}

/// Draws the tab body (everything below the tab bar) for `layout`.
pub(crate) fn draw_body(
    f: &mut Frame<'_>,
    area: Rect,
    layout: Layout,
    view: &mut InterceptView,
    topics: &Topics,
    schema: InputSchema<'_>,
    now: Instant,
) {
    if layout == Layout::TooSmall {
        draw_too_small(f, area);
        return;
    }
    let [main, status_area] = Split::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .areas(area);
    let (mut schema, schema_error) = match schema {
        Some(Ok(schema)) => (Some(schema), None),
        Some(Err(why)) => (None, Some(format!("cannot be used: {why}"))),
        None => (None, Some("is not resolved yet".to_owned())),
    };
    let tree_error = refresh_rows(view, topics, schema.as_deref_mut(), schema_error.as_deref());
    let status = view.status(now);
    draw_status(f, status_area, &status);

    match layout {
        Layout::Wide => {
            let [left, right] = Split::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(LEFT_WIDTH), Constraint::Min(50)])
                .areas(main);
            draw_left(f, left, view, topics);
            draw_right(f, right, view, topics, schema, tree_error.as_deref());
        }
        Layout::Medium => {
            let [selector, panes] = Split::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(1), Constraint::Min(1)])
                .areas(main);
            draw_selector(f, selector, view, topics);
            draw_right(f, panes, view, topics, schema, tree_error.as_deref());
        }
        Layout::Narrow | Layout::TooSmall => {
            let [crumb, pane] = Split::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(1), Constraint::Min(1)])
                .areas(main);
            draw_breadcrumb(f, crumb, view);
            match view.pane() {
                Pane::List => draw_left(f, pane, view, topics),
                Pane::Tree => draw_tree(f, pane, view, topics, schema, tree_error.as_deref()),
                Pane::Editor => draw_editor(f, pane, view, topics),
            }
        }
    }
    if let Some(form) = &view.form {
        draw_form(f, area, form, topics);
    } else if view.confirm_delete {
        draw_confirm(f, area, view);
    }
}

/// The interceptions above the selected one's details.
fn draw_left(f: &mut Frame<'_>, area: Rect, view: &InterceptView, topics: &Topics) {
    let details = details_lines(view, topics);
    let wanted = u16::try_from(details.len()).unwrap_or(u16::MAX).saturating_add(2);
    let height = wanted.min(area.height.saturating_sub(4));
    let [list, info] = Split::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(height)])
        .areas(area);
    draw_list(f, list, view);
    f.render_widget(
        Paragraph::new(details).block(pane_block("Details", false)).wrap(Wrap { trim: false }),
        info,
    );
}

/// The field tree above the override editor.
fn draw_right(
    f: &mut Frame<'_>,
    area: Rect,
    view: &InterceptView,
    topics: &Topics,
    schema: Option<&mut MessageSchema>,
    error: Option<&str>,
) {
    let inner_width = area.width.saturating_sub(2);
    let lines = editor_lines(view, topics, inner_width).len();
    let wanted = u16::try_from(lines).unwrap_or(u16::MAX).saturating_add(2);
    let height = wanted.clamp(3, (area.height / 2).max(3));
    let [tree, editor] = Split::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(height)])
        .areas(area);
    draw_tree(f, tree, view, topics, schema, error);
    draw_editor(f, editor, view, topics);
}

/// The type of the selected interception's input: as the backend resolved it, else
/// as the bus announces it.
fn input_type<'a>(
    view: &'a InterceptView,
    topics: &'a Topics,
    input: &str,
    name: &str,
) -> Option<&'a str> {
    view.applied(name)
        .map(|state| state.data_type.as_str())
        .filter(|data_type| !data_type.is_empty())
        .or_else(|| topics.get(input).map(|stats| stats.data_type.as_str()))
        .filter(|data_type| !data_type.is_empty())
}

fn rate_text(topics: &Topics, topic: &str) -> String {
    topics
        .get(topic)
        .map_or_else(|| "no traffic".to_owned(), |stats| format!("{:.1} Hz", stats.rate_hz))
}

/// The selected interception: its topics, type, relay state and counters.
fn details_lines(view: &InterceptView, topics: &Topics) -> Vec<Line<'static>> {
    let Some(spec) = view.spec() else {
        return vec![Line::styled("n adds an interception", Style::default().fg(MUTED))];
    };
    let field = |label: &str, value: String, color| {
        Line::from(vec![
            Span::styled(format!("{label:<7}"), Style::default().fg(MUTED)),
            Span::styled(value, Style::default().fg(color)),
        ])
    };
    let applied = view.applied(&spec.name);
    let (mark, color) = state_mark(applied.map(|state| state.state));
    let (arm, arm_color) = if spec.armed { ("● armed", OK) } else { ("○ bypass", WARN) };
    let data_type = input_type(view, topics, &spec.input, &spec.name)
        .map_or_else(|| "unknown until the input publishes".to_owned(), str::to_owned);
    let mut lines = vec![
        Line::styled(spec.name.clone(), Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
        Line::from(vec![
            Span::styled(format!("{:<7}", "state"), Style::default().fg(MUTED)),
            Span::styled(mark, Style::default().fg(color)),
            Span::raw(" · "),
            Span::styled(arm, Style::default().fg(arm_color)),
        ]),
        field("in", spec.input.clone(), FG),
        field("", rate_text(topics, &spec.input), MUTED),
        field("out", spec.output.clone(), FG),
        field("", rate_text(topics, &spec.output), MUTED),
        field("type", data_type, FG),
        field("ovr", format!("{} field(s)", spec.overrides.len()), FG),
    ];
    if let Some(state) = applied {
        lines.push(field(
            "frames",
            format!("{} in · {} changed · {} out", state.received, state.modified, state.published),
            FG,
        ));
        if state.patch_failed > 0 || state.unresolved_dropped > 0 {
            lines.push(field(
                "lost",
                format!(
                    "{} patch failed · {} unresolved",
                    state.patch_failed, state.unresolved_dropped
                ),
                WARN,
            ));
        }
        if !state.last_error.is_empty() {
            lines.push(field("error", state.last_error.replace('\n', " "), ERR));
        }
    }
    lines
}

/// `width`×`height` in the middle of `area`, cut to fit.
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x.saturating_add(area.width.saturating_sub(width) / 2),
        y: area.y.saturating_add(area.height.saturating_sub(height) / 2),
        width,
        height,
    }
}

fn draw_modal(f: &mut Frame<'_>, area: Rect, title: &str, width: u16, lines: Vec<Line<'_>>) {
    let height = u16::try_from(lines.len()).unwrap_or(u16::MAX).saturating_add(2);
    let rect = centered(area, width, height);
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines)
            .block(pane_block(title, true))
            .style(Style::default().bg(SURFACE))
            .wrap(Wrap { trim: false }),
        rect,
    );
}

/// The new-interception modal: three fields, the input's type, and topic hints
/// under the focused topic field.
fn draw_form(f: &mut Frame<'_>, area: Rect, form: &NewForm, topics: &Topics) {
    let width = area.width.saturating_sub(4).min(76);
    let room = usize::from(width.saturating_sub(12));
    let hints = suggestions(form, topics);
    let mut lines = Vec::new();
    for field in FormField::ALL {
        let focused = field == form.field;
        let text = truncate_left(form.text(field), room);
        let style = if focused {
            Style::default().fg(ON_ACCENT).bg(ACCENT)
        } else {
            Style::default().fg(FG)
        };
        let mut spans = vec![
            Span::styled(format!("{:>7}  ", field.label()), Style::default().fg(MUTED)),
            Span::styled(text, style),
        ];
        if focused {
            spans.push(Span::styled("▏", Style::default().fg(ACCENT)));
        }
        lines.push(Line::from(spans));
        if field == FormField::Input {
            let about = match topics.get(&form.input) {
                Some(stats) if !stats.data_type.is_empty() => {
                    format!("{} · {:.1} Hz", stats.data_type, stats.rate_hz)
                }
                Some(_) => "on the bus, type not announced yet".to_owned(),
                None if form.input.is_empty() => "the topic to rewrite".to_owned(),
                None => "not on the bus yet".to_owned(),
            };
            lines.push(Line::styled(format!("{:>9}{about}", ""), Style::default().fg(MUTED)));
        }
        if focused && field != FormField::Name {
            if hints.is_empty() {
                lines.push(Line::styled(
                    format!("{:>9}no topic matches", ""),
                    Style::default().fg(MUTED),
                ));
            }
            for (index, (topic, data_type)) in hints.iter().enumerate() {
                let chosen = form.pick == Some(index);
                let style = if chosen {
                    Style::default().fg(ON_ACCENT).bg(ACCENT2)
                } else {
                    Style::default().fg(FG)
                };
                let topic_width =
                    room.saturating_sub(data_type.chars().count().min(room / 2).saturating_add(3));
                lines.push(Line::from(vec![
                    Span::raw(format!("{:>7}", "")),
                    Span::styled(if chosen { "▸ " } else { "  " }, Style::default().fg(ACCENT)),
                    Span::styled(pad(topic, topic_width), style),
                    Span::styled(format!(" {data_type}"), Style::default().fg(MUTED)),
                ]));
            }
        }
    }
    if !form.error.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::styled(form.error.clone(), Style::default().fg(ERR)));
    }
    draw_modal(f, area, "New interception", width, lines);
}

fn draw_confirm(f: &mut Frame<'_>, area: Rect, view: &InterceptView) {
    let Some(spec) = view.spec() else { return };
    let mut lines = vec![
        Line::styled(spec.name.clone(), Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
        Line::raw(format!("{} → {}", spec.input, spec.output)),
    ];
    if !spec.overrides.is_empty() {
        lines.push(Line::styled(
            format!("its {} override(s) are removed", spec.overrides.len()),
            Style::default().fg(WARN),
        ));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled("y deletes · any other key keeps it", Style::default().fg(MUTED)));
    let width = area.width.saturating_sub(4).min(60);
    draw_modal(f, area, "Delete interception?", width, lines);
}

/// The keys of what has the focus, in place of the global footer.
pub(crate) fn draw_hints(f: &mut Frame<'_>, area: Rect, view: &InterceptView) {
    let text = if view.form.is_some() {
        " [↑↓] Topic hints  [Enter] Pick / Create  [Tab] Next field  [Esc] Cancel "
    } else if view.confirm_delete {
        " [y] Delete  [any other key] Keep "
    } else {
        match view.pane() {
            Pane::List => {
                " [n] New  [d] Delete  [a] Arm/Bypass  [↑↓] Select  [Enter] Fields  [Tab] Pane  [1-6] Tabs  [q] Quit "
            }
            Pane::Tree => {
                " [Enter] Edit  [←→] Fold  [x] Clear  [X] Clear all  [a] Arm/Bypass  [d] Delete  [Esc] List "
            }
            Pane::Editor => {
                match view.edit.as_ref().map(|edit| (edit.bits.is_empty(), edit.type_name.as_str()))
                {
                    Some((false, _)) => {
                        " [←→] Bit  [Space] Keep/Set/Clear  [Enter] Apply  [Ctrl+X] Clear  [Esc] Fields "
                    }
                    Some((true, "bool")) => " [Space] Toggle  [Ctrl+X] Clear  [Esc] Fields ",
                    _ => " [Enter] Apply  [Ctrl+X] Clear  [Esc] Fields ",
                }
            }
        }
    };
    f.render_widget(
        Paragraph::new(text)
            .style(Style::default().bg(SURFACE).fg(FG).add_modifier(Modifier::BOLD)),
        area,
    );
}

/// Rebuilds the field tree from the input's newest payload. Returns why there is
/// no tree, when there is none.
fn refresh_rows(
    view: &mut InterceptView,
    topics: &Topics,
    schema: Option<&mut MessageSchema>,
    schema_error: Option<&str>,
) -> Option<String> {
    let Some(spec) = view.spec() else {
        view.rows.clear();
        return Some("no interception: press n in the list to add one".to_owned());
    };
    let input = spec.input.clone();
    let Some(stats) = topics.get(&input).filter(|stats| !stats.last_payload.is_empty()) else {
        view.rows.clear();
        return Some(format!("no traffic on {input} yet"));
    };
    let Some(schema) = schema else {
        view.rows.clear();
        return Some(format!("the schema of {input} {}", schema_error.unwrap_or("is missing")));
    };
    match build_rows(schema, &stats.data_type, &stats.last_payload, &view.toggled) {
        Ok(rows) => {
            view.rows = rows;
            None
        }
        Err(why) => {
            view.rows.clear();
            Some(why)
        }
    }
}

fn pane_block(title: &str, focused: bool) -> Block<'_> {
    let color = if focused { ACCENT } else { MUTED };
    Block::default().borders(Borders::ALL).border_style(Style::default().fg(color)).title(
        Span::styled(format!(" {title} "), Style::default().fg(color).add_modifier(Modifier::BOLD)),
    )
}

fn state_mark(state: Option<RelayState>) -> (&'static str, ratatui::style::Color) {
    match state {
        Some(RelayState::Live) => ("● live", OK),
        Some(RelayState::Resolving) => ("◌ resolving", WARN),
        Some(RelayState::SchemaChanged) => ("✕ schema changed", ERR),
        Some(RelayState::Error) => ("✕ error", ERR),
        None => ("○ not applied", MUTED),
    }
}

fn draw_list(f: &mut Frame<'_>, area: Rect, view: &InterceptView) {
    let selected = view.spec().map(|spec| spec.name.clone());
    let width = usize::from(area.width.saturating_sub(4));
    let mut lines = Vec::new();
    for spec in &view.config.interception {
        let mark = state_mark(view.applied(&spec.name).map(|state| state.state));
        let chosen = selected.as_ref() == Some(&spec.name);
        let style = if chosen {
            Style::default().fg(ON_ACCENT).bg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(FG)
        };
        let arm = if spec.armed { "●" } else { "○" };
        lines.push(Line::from(vec![
            Span::styled(
                format!("{arm} {}", truncate_middle(&spec.name, width.saturating_sub(4))),
                style,
            ),
            Span::styled(
                format!(" {}", mark.0.chars().next().unwrap_or(' ')),
                Style::default().fg(mark.1),
            ),
        ]));
    }
    if lines.is_empty() {
        lines.push(Line::styled("none yet: n adds one", Style::default().fg(MUTED)));
    }
    f.render_widget(
        Paragraph::new(lines).block(pane_block("Interceptions", view.pane() == Pane::List)),
        area,
    );
}

fn draw_selector(f: &mut Frame<'_>, area: Rect, view: &InterceptView, topics: &Topics) {
    let names = &view.config.interception;
    let line = match view.spec() {
        Some(spec) => {
            let index = names.iter().position(|entry| entry.name == spec.name).unwrap_or(0);
            let mark = state_mark(view.applied(&spec.name).map(|state| state.state));
            Line::from(vec![
                Span::styled(
                    format!("◀ {} ({}/{}) ▶  ", spec.name, index.saturating_add(1), names.len()),
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("{} → {} ", spec.input, spec.output), Style::default().fg(FG)),
                Span::styled(
                    format!(
                        "{} ",
                        input_type(view, topics, &spec.input, &spec.name).unwrap_or("?")
                    ),
                    Style::default().fg(MUTED),
                ),
                Span::styled(mark.0, Style::default().fg(mark.1)),
            ])
        }
        None => Line::styled("no interception: n adds one", Style::default().fg(MUTED)),
    };
    f.render_widget(Paragraph::new(line), area);
}

fn draw_breadcrumb(f: &mut Frame<'_>, area: Rect, view: &InterceptView) {
    let mut text =
        view.spec().map_or_else(|| "no interception".to_owned(), |spec| spec.name.clone());
    if let Some(path) = &view.selected_path {
        text = format!("{text} › {path}");
    }
    let text = truncate_middle(&text, usize::from(area.width));
    f.render_widget(Paragraph::new(text).style(Style::default().fg(ACCENT)), area);
}

/// How a row's path is overridden: `V` value, `B` bits, `S` something below it.
fn override_mark(view: &InterceptView, path: &str) -> &'static str {
    let Some(spec) = view.spec() else { return "" };
    match spec.overrides.get(path) {
        Some(OverrideSpec::Value { .. }) => "V",
        Some(OverrideSpec::Bits { .. }) => "B",
        None => {
            let below = spec.overrides.keys().any(|key| {
                key.strip_prefix(path)
                    .is_some_and(|rest| rest.starts_with('.') || rest.starts_with('['))
            });
            if below { "S" } else { "" }
        }
    }
}

fn scalar_text(type_name: &str, wire: &[u8]) -> String {
    match read_scalar(type_name, wire) {
        Ok(ScalarValue::Text(bytes)) => format!("\"{}\"", hypha_tools::msg_patch::escape(&bytes)),
        Ok(value) => format_value(&value, Radix::Dec),
        Err(_) => "–".to_owned(),
    }
}

fn type_text(row: &TreeRow) -> String {
    match &row.kind {
        RowKind::Scalar { type_name, .. } => type_name.clone(),
        RowKind::Message => "msg".to_owned(),
        RowKind::Array { count } => format!("[{count}]"),
        RowKind::Bytes { count } => format!("uint8[{count}]"),
    }
}

fn pad(text: &str, width: usize) -> String {
    let cut = truncate_middle(text, width);
    let fill = width.saturating_sub(cut.chars().count());
    format!("{cut}{}", " ".repeat(fill))
}

fn draw_tree(
    f: &mut Frame<'_>,
    area: Rect,
    view: &InterceptView,
    topics: &Topics,
    schema: Option<&mut MessageSchema>,
    error: Option<&str>,
) {
    let block = pane_block("Fields", view.pane() == Pane::Tree);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if let Some(error) = error {
        f.render_widget(
            Paragraph::new(error.to_owned())
                .style(Style::default().fg(MUTED))
                .wrap(Wrap { trim: true }),
            inner,
        );
        return;
    }
    let width = usize::from(inner.width);
    let columns = columns_for(width);
    let fixed = OVR
        .saturating_add(if columns.out { VALUE } else { 0 })
        .saturating_add(if columns.input { VALUE } else { 0 })
        .saturating_add(if columns.type_name { TYPE } else { 0 });
    let name_width = width.saturating_sub(fixed).max(1);
    let height = usize::from(inner.height.saturating_sub(1));
    let selected = view
        .selected_path
        .as_ref()
        .and_then(|path| view.rows.iter().position(|row| &row.path == path));
    let offset = match selected {
        Some(index) if height > 0 && index >= height => {
            index.saturating_sub(height.saturating_sub(1))
        }
        _ => 0,
    };
    let visible: Vec<&TreeRow> = view.rows.iter().skip(offset).take(height).collect();
    let spec = view.spec();
    let input_payload =
        spec.and_then(|spec| topics.get(&spec.input)).map(|stats| stats.last_payload.as_slice());
    let out_values =
        if columns.out { output_values(view, topics, schema, &visible) } else { BTreeMap::new() };

    let mut header = format!("{}{}", pad("name", name_width), pad("ovr", OVR));
    if columns.out {
        header.push_str(&pad("out", VALUE));
    }
    if columns.input {
        header.push_str(&pad("in", VALUE));
    }
    if columns.type_name {
        header.push_str(&pad("type", TYPE));
    }
    let mut lines =
        vec![Line::styled(header, Style::default().fg(ACCENT).add_modifier(Modifier::BOLD))];
    for row in visible {
        let marker = match row.kind {
            RowKind::Message | RowKind::Array { .. } if row.toggleable => {
                if row.open {
                    "▾ "
                } else {
                    "▸ "
                }
            }
            _ => "  ",
        };
        let name = format!("{}{marker}{}", "  ".repeat(row.depth), row.label);
        let mut text =
            format!("{}{}", pad(&name, name_width), pad(override_mark(view, &row.path), OVR));
        let input_text = match &row.kind {
            RowKind::Scalar { type_name, span } => input_payload
                .and_then(|payload| payload.get(span.clone()))
                .map_or_else(|| "–".to_owned(), |wire| scalar_text(type_name, wire)),
            RowKind::Bytes { count } => format!("<{count} bytes>"),
            RowKind::Array { count } => format!("[{count}]"),
            RowKind::Message => String::new(),
        };
        if columns.out {
            let out = match &row.kind {
                RowKind::Scalar { .. } => {
                    out_values.get(&row.path).cloned().unwrap_or_else(|| "–".to_owned())
                }
                _ => String::new(),
            };
            text.push_str(&pad(&out, VALUE));
        }
        if columns.input {
            text.push_str(&pad(&input_text, VALUE));
        }
        if columns.type_name {
            text.push_str(&pad(&type_text(row), TYPE));
        }
        let style = if view.selected_path.as_ref() == Some(&row.path) {
            Style::default().fg(ON_ACCENT).bg(ACCENT2)
        } else {
            Style::default().fg(FG)
        };
        lines.push(Line::styled(text, style));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

/// The output's value of each visible scalar, decoded from the output's newest
/// payload against the input schema. A path the output does not hold has no entry
/// and shows `–`: an old value is never shown in its place.
fn output_values(
    view: &InterceptView,
    topics: &Topics,
    schema: Option<&mut MessageSchema>,
    visible: &[&TreeRow],
) -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();
    let (Some(spec), Some(schema)) = (view.spec(), schema) else { return values };
    let live = view.applied(&spec.name).is_some_and(|state| state.state == RelayState::Live);
    let (Some(input), Some(output)) = (topics.get(&spec.input), topics.get(&spec.output)) else {
        return values;
    };
    if !live || output.last_payload.is_empty() || input.md5.is_empty() || input.md5 != output.md5 {
        return values;
    }
    let want: BTreeSet<String> = visible
        .iter()
        .filter(|row| matches!(row.kind, RowKind::Scalar { .. }))
        .map(|row| row.path.clone())
        .collect();
    if let Ok(slots) = index_slots(schema, &input.data_type, &output.last_payload, Some(&want)) {
        for slot in slots {
            if let Some(wire) = output.last_payload.get(slot.span.clone()) {
                values.insert(slot.path, scalar_text(&slot.type_name, wire));
            }
        }
    }
    values
}

fn draw_editor(f: &mut Frame<'_>, area: Rect, view: &InterceptView, topics: &Topics) {
    let block = pane_block("Override", view.pane() == Pane::Editor);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let lines = editor_lines(view, topics, inner.width);
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn editor_lines<'a>(view: &'a InterceptView, topics: &Topics, width: u16) -> Vec<Line<'a>> {
    let mut lines: Vec<Line<'_>> = Vec::new();
    let Some(edit) = &view.edit else {
        lines.push(Line::styled("select a field, then Enter", Style::default().fg(MUTED)));
        return lines;
    };
    lines.push(Line::styled(
        truncate_middle(&edit.path, usize::from(width)),
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
    ));
    lines.push(Line::styled(edit.type_name.clone(), Style::default().fg(MUTED)));
    let current = view.spec().and_then(|spec| topics.get(&spec.input)).and_then(|stats| {
        let row = view.rows.iter().find(|row| row.path == edit.path)?;
        let RowKind::Scalar { span, .. } = &row.kind else { return None };
        stats.last_payload.get(span.clone()).map(<[u8]>::to_vec)
    });
    if let Some(wire) = &current
        && let Ok(value) = read_scalar(&edit.type_name, wire)
    {
        match (&value, integer_width(&edit.type_name)) {
            (ScalarValue::Int(_) | ScalarValue::UInt(_), Some(_)) => {
                for radix in [Radix::Dec, Radix::Hex, Radix::Bin] {
                    lines.push(Line::raw(format!("in  {}", format_value(&value, radix))));
                }
            }
            (ScalarValue::Float(_), _) => {
                let hex = wire.iter().rev().fold(String::new(), |mut hex, byte| {
                    let _ = write!(hex, "{byte:02x}");
                    hex
                });
                lines.push(Line::raw(format!("in  {}", format_value(&value, Radix::Dec))));
                lines.push(Line::raw(format!("in  0x{hex}")));
            }
            _ => lines.push(Line::raw(format!("in  {}", scalar_text(&edit.type_name, wire)))),
        }
    }
    let (before, after) = edit.text.split_at(byte_index(&edit.text, edit.cursor));
    lines.push(Line::from(vec![
        Span::styled("set ", Style::default().fg(MUTED)),
        Span::styled(before.to_owned(), Style::default().fg(FG)),
        Span::styled("▏", Style::default().fg(ACCENT)),
        Span::styled(after.to_owned(), Style::default().fg(FG)),
    ]));
    if !edit.error.is_empty() {
        lines.push(Line::styled(edit.error.clone(), Style::default().fg(ERR)));
    }
    if !edit.bits.is_empty() {
        lines.extend(bit_grid(edit, current.as_deref(), bits_per_row(width)));
    }
    lines
}

/// The tri-state grid, most significant bit first: the input's bit where it is
/// kept, `1`/`0` where it is set/cleared.
fn bit_grid<'a>(
    edit: &crate::top::intercept::EditBuffer,
    wire: Option<&[u8]>,
    per_row: usize,
) -> Vec<Line<'a>> {
    let mut raw = [0_u8; 8];
    if let Some(wire) = wire {
        for (slot, byte) in raw.iter_mut().zip(wire.iter()) {
            *slot = *byte;
        }
    }
    let input = u64::from_le_bytes(raw);
    let count = edit.bits.len();
    let mut lines = Vec::new();
    let mut spans = Vec::new();
    for index in (0..count).rev() {
        let shift = u32::try_from(index).unwrap_or(0);
        let input_bit = input.checked_shr(shift).unwrap_or(0) & 1;
        let (text, color) = match edit.bits.get(index) {
            Some(BitState::Set) => ("1".to_owned(), OK),
            Some(BitState::Clear) => ("0".to_owned(), ERR),
            _ => (input_bit.to_string(), MUTED),
        };
        let mut style = Style::default().fg(color);
        if index == edit.bit {
            style = style.add_modifier(Modifier::REVERSED);
        }
        spans.push(Span::styled(text, style));
        spans.push(Span::raw(if index % 4 == 0 { "  " } else { " " }));
        if index.checked_rem(per_row) == Some(0) {
            let top = index.saturating_add(per_row).saturating_sub(1);
            spans.insert(0, Span::styled(format!("{top:>2}│"), Style::default().fg(MUTED)));
            lines.push(Line::from(std::mem::take(&mut spans)));
        }
    }
    lines
}

fn draw_status(f: &mut Frame<'_>, area: Rect, status: &StatusLine) {
    let mut spans = Vec::new();
    match status.armed {
        Some(true) => spans
            .push(Span::styled(" ● ARMED ", Style::default().fg(OK).add_modifier(Modifier::BOLD))),
        Some(false) => spans.push(Span::styled(
            " ○ BYPASS ",
            Style::default().fg(WARN).add_modifier(Modifier::BOLD),
        )),
        None => spans.push(Span::styled(" no interception ", Style::default().fg(MUTED))),
    }
    let right = if status.offline {
        " ⚠ backend offline "
    } else if status.pending {
        " ◌ pending "
    } else {
        ""
    };
    let message = if !status.message.is_empty() {
        status.message.clone()
    } else if !status.config_error.is_empty() {
        format!("config: {}", status.config_error)
    } else {
        status.last_error.clone()
    };
    let used = spans.iter().map(Span::width).sum::<usize>().saturating_add(right.chars().count());
    let room = usize::from(area.width).saturating_sub(used);
    let message = truncate_left(&message.replace('\n', " "), room);
    let fill = room.saturating_sub(message.chars().count());
    spans.push(Span::styled(message, Style::default().fg(ERR)));
    spans.push(Span::raw(" ".repeat(fill)));
    spans.push(Span::styled(right, Style::default().fg(WARN).add_modifier(Modifier::BOLD)));
    f.render_widget(Paragraph::new(Line::from(spans)).style(Style::default().bg(SURFACE)), area);
}

#[cfg(test)]
#[path = "intercept_ui_tests.rs"]
mod tests;
