//! Drawing tests of the Intercept tab, on a `TestBackend`.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use super::*;
use crate::top::intercept::layout_for;
use crate::top::intercept::tests::{DEFINITION, INPUT, ROOT, bench, payload};
use crate::top::state::AppState;
use crossterm::event::{KeyCode, KeyModifiers};
use hypha_node::sdk::message::EncodeInto as _;
use hypha_tools::intercept_doc::{BackendState, InterceptionState};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use std::time::Duration;

const MD5: &str = "0123456789abcdef0123456789abcdef";

fn render_at(
    view: &mut InterceptView,
    app: &AppState,
    width: u16,
    height: u16,
    now: Instant,
) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    let mut schema = MessageSchema::from_definition(ROOT, DEFINITION).unwrap();
    terminal
        .draw(|f| {
            let area = f.area();
            let layout = layout_for(area.width, area.height);
            draw_body(f, area, layout, view, &app.topics, Some(Ok(&mut schema)), now);
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    let mut text = String::new();
    for (index, cell) in buffer.content.iter().enumerate() {
        if index > 0 && index % usize::from(width) == 0 {
            text.push('\n');
        }
        text.push_str(cell.symbol());
    }
    text
}

fn hints(view: &InterceptView) -> String {
    let mut terminal = Terminal::new(TestBackend::new(140, 1)).unwrap();
    terminal.draw(|f| draw_hints(f, f.area(), view)).unwrap();
    terminal.backend().buffer().content.iter().map(ratatui::buffer::Cell::symbol).collect()
}

fn render(view: &mut InterceptView, app: &AppState, width: u16, height: u16) -> String {
    render_at(view, app, width, height, Instant::now())
}

fn with_cells(app: &mut AppState, cells: usize) {
    app.on_sample(INPUT, 1, &payload(cells));
    app.apply_schema(INPUT, 1, ROOT, MD5);
}

fn press(view: &mut InterceptView, app: &AppState, code: KeyCode) {
    view.handle_key(code, KeyModifiers::NONE, &app.topics, None);
}

#[test]
fn too_small_notice() {
    let (mut view, mut app, _path) = bench("ui_small");
    with_cells(&mut app, 3);
    view.selected_path = Some("status".to_owned());
    let rows = view.rows.clone();
    for (width, height) in [(59, 12), (60, 11)] {
        let text = render(&mut view, &app, width, height);
        assert!(text.contains(&too_small_text(width, height)), "{text}");
        assert_eq!(view.rows, rows, "no state is touched");
        assert_eq!(view.selected_path.as_deref(), Some("status"));
    }
}

#[test]
fn breakpoints_pick_the_layout() {
    let (mut view, mut app, _path) = bench("ui_layout");
    with_cells(&mut app, 3);
    let wide = render(&mut view, &app, 110, 24);
    for pane in ["Interceptions", "Details", "Fields", "Override"] {
        assert!(wide.contains(pane), "{pane}:\n{wide}");
    }
    let medium = render(&mut view, &app, 109, 20);
    assert!(medium.contains("◀ battery (1/1) ▶"), "{medium}");
    assert!(medium.contains("test_msgs/Battery"), "the selector names the type:\n{medium}");
    assert!(!medium.contains("Interceptions") && medium.contains("Fields"), "{medium}");
    let narrow = render(&mut view, &app, 79, 20);
    assert!(narrow.contains("Interceptions") && !narrow.contains("Fields"), "{narrow}");
    view.focus = Some(Pane::Tree);
    view.selected_path = Some("cells[1].voltage".to_owned());
    let narrow = render(&mut view, &app, 60, 12);
    assert!(narrow.contains("battery › cells[1].voltage"), "{narrow}");
    assert!(narrow.contains("Fields") && !narrow.contains("Interceptions"), "{narrow}");
}

#[test]
fn resize_through_all_breakpoints_keeps_edit_state() {
    let (mut view, mut app, _path) = bench("ui_resize");
    with_cells(&mut app, 12);
    render(&mut view, &app, 160, 14);
    view.focus = Some(Pane::Tree);
    view.selected_path = Some("cells[11].voltage".to_owned());
    press(&mut view, &app, KeyCode::Enter);
    assert_eq!(view.edit.as_ref().unwrap().text, "3.5");
    for _ in 0..3 {
        press(&mut view, &app, KeyCode::Backspace);
    }
    for character in "0.5".chars() {
        press(&mut view, &app, KeyCode::Char(character));
    }
    press(&mut view, &app, KeyCode::Left);
    let edit = view.edit.clone().unwrap();
    assert_eq!((edit.text.as_str(), edit.cursor), ("0.5", 2));
    for width in [160, 100, 70, 50, 70, 100, 160] {
        let text = render(&mut view, &app, width, 14);
        assert_eq!(view.edit.as_ref(), Some(&edit), "at {width}");
        assert_eq!(view.selected_path.as_deref(), Some("cells[11].voltage"), "at {width}");
        match layout_for(width, 14) {
            Layout::Wide | Layout::Medium => {
                assert!(text.contains("[11]"), "the selected row is in view at {width}:\n{text}");
            }
            Layout::Narrow => {
                assert!(text.contains("set 0.▏5"), "the editor shows the buffer:\n{text}");
            }
            Layout::TooSmall => assert!(text.contains("terminal too small")),
        }
    }
}

#[test]
fn columns_drop_right_to_left() {
    assert_eq!(columns_for(34), Columns { out: false, input: false, type_name: false });
    assert_eq!(columns_for(35), Columns { out: true, input: false, type_name: false });
    assert_eq!(columns_for(50), Columns { out: true, input: true, type_name: false });
    assert_eq!(columns_for(67), Columns { out: true, input: true, type_name: true });
    let (mut view, mut app, _path) = bench("ui_columns");
    with_cells(&mut app, 3);
    let wide = render(&mut view, &app, 160, 20);
    let header = wide.lines().find(|line| line.contains("name")).unwrap();
    for column in ["ovr", "out", "in", "type"] {
        assert!(header.contains(column), "{header}");
    }
    view.focus = Some(Pane::Tree);
    let narrow = render(&mut view, &app, 60, 20);
    let header = narrow.lines().find(|line| line.contains("name")).unwrap();
    assert!(header.contains("ovr") && !header.contains("type"), "{header}");
}

#[test]
fn details_show_topics_type_and_counters() {
    let (mut view, mut app, _path) = bench("ui_details");
    with_cells(&mut app, 3);
    let text = render(&mut view, &app, 120, 30);
    for want in
        ["Details", "/battery/raw", "/battery", "test_msgs/Battery", "◌ resolving", "● armed"]
    {
        assert!(text.contains(want), "{want}:\n{text}");
    }
    let footer = hints(&view);
    assert!(footer.contains("[n] New  [d] Delete"), "the footer names the list keys: {footer}");
    view.focus = Some(Pane::Tree);
    assert!(hints(&view).contains("[d] Delete"), "{}", hints(&view));
}

#[test]
fn new_interception_is_a_modal_with_topic_hints_and_type() {
    let (mut view, mut app, _path) = bench("ui_form");
    with_cells(&mut app, 3);
    app.on_sample("/lidar/points", 2, &[0]);
    view.handle_key(KeyCode::Char('n'), KeyModifiers::NONE, &app.topics, Some(INPUT));
    let text = render(&mut view, &app, 120, 30);
    assert!(text.contains("New interception"), "{text}");
    assert!(text.contains("test_msgs/Battery"), "the input's type is shown:\n{text}");
    assert!(text.contains("/battery/raw/intercepted"), "{text}");
    assert!(text.contains("battery_raw"), "{text}");
    assert!(!text.contains("/lidar/points"), "only matching topics are hinted:\n{text}");
    for _ in 0..INPUT.len() {
        press(&mut view, &app, KeyCode::Backspace);
    }
    press(&mut view, &app, KeyCode::Down);
    let text = render(&mut view, &app, 120, 30);
    assert!(text.contains("▸ /battery/raw") && text.contains("/lidar/points"), "{text}");
    assert!(hints(&view).contains("[Enter] Pick / Create"), "{}", hints(&view));
    let narrow = render(&mut view, &app, 60, 14);
    assert!(narrow.contains("New interception"), "the modal fits a narrow terminal:\n{narrow}");
}

#[test]
fn delete_asks_in_a_modal() {
    let (mut view, mut app, _path) = bench("ui_confirm");
    with_cells(&mut app, 3);
    press(&mut view, &app, KeyCode::Char('d'));
    let text = render(&mut view, &app, 120, 30);
    assert!(text.contains("Delete interception?"), "{text}");
    assert!(text.contains("/battery/raw → /battery"), "{text}");
    assert!(text.contains("y deletes"), "{text}");
    assert!(hints(&view).contains("[y] Delete"), "{}", hints(&view));
}

#[test]
fn truncate_middle_keeps_both_ends() {
    assert_eq!(truncate_middle("cells[2].voltage", 12), "cells[…ltage");
    assert_eq!(truncate_middle("cells[2].voltage", 12).chars().count(), 12);
    assert_eq!(truncate_middle("status", 12), "status");
    assert_eq!(truncate_left("abcdef", 4), "…def");
}

#[test]
fn status_bar_shows_armed_error_pending_offline() {
    let (mut view, mut app, path) = bench("ui_status");
    with_cells(&mut app, 3);
    let now = Instant::now();
    let text = render_at(&mut view, &app, 140, 20, now);
    assert!(text.contains("● ARMED") && !text.contains("pending"), "{text}");

    let state = BackendState {
        backend: "bench".to_owned(),
        config_path: path.display().to_string(),
        applied_revision: 3,
        config_error: String::new(),
        interception: vec![InterceptionState {
            name: "battery".to_owned(),
            last_error: "cells[5].voltage: <no such field in this message>".to_owned(),
            ..InterceptionState::default()
        }],
    };
    let message =
        hypha_node::sdk::msgs::std_msgs::String { data: toml::to_string(&state).unwrap() };
    let mut doc = vec![0; message.encoded_len()];
    message.encode_into(&mut doc).unwrap();
    view.on_state(&doc, now);
    let text = render_at(&mut view, &app, 140, 20, now);
    assert!(text.contains("<no such field in this message>"), "{text}");

    press(&mut view, &app, KeyCode::Char('a'));
    let text = render_at(&mut view, &app, 140, 20, now);
    assert!(text.contains("○ BYPASS") && text.contains("◌ pending"), "{text}");
    let later = render_at(&mut view, &app, 140, 20, now + Duration::from_secs(3));
    assert!(later.contains("⚠ backend offline"), "{later}");
}

#[test]
fn bit_grid_is_one_row_of_16_in_the_wide_editor() {
    assert_eq!(bits_per_row(55), 8);
    assert_eq!(bits_per_row(56), 16);
    let (mut view, mut app, _path) = bench("ui_bits");
    with_cells(&mut app, 3);
    render(&mut view, &app, 160, 30);
    view.focus = Some(Pane::Tree);
    view.selected_path = Some("status".to_owned());
    press(&mut view, &app, KeyCode::Enter);
    let text = render(&mut view, &app, 160, 30);
    assert!(text.contains("15│") && !text.contains(" 7│"), "one row of 16 bits:\n{text}");
    assert!(text.contains("in  0xf0"), "{text}");
}
