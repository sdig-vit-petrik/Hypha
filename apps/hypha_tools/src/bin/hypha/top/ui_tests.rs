//! Tests of the `hypha top` text: node, cycle and input lines.
#![allow(clippy::unwrap_used)]

use super::*;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

#[test]
fn hypha_top_shows_cycle_rows_and_input_counters() {
    let cycle = hypha_node::sdk::msgs::hypha_msgs::CycleState {
        name: "control".to_owned(),
        steps: 12,
        missed_periods: 3,
        last_step_nanos: 1_500_000,
        max_step_nanos: 15_250_000,
        overruns: 1,
        output_drops: 2,
        step_p50_nanos: 1_200_000,
        step_p99_nanos: 3_400_000,
        recent_step_p50_nanos: 1_100_000,
        recent_step_p99_nanos: 2_500_000,
        recent_step_max_nanos: 3_100_000,
        ..hypha_node::sdk::msgs::hypha_msgs::CycleState::default()
    };
    assert_eq!(
        cycle_line_text(&cycle),
        "control: 12 steps, 3 missed, step p50/p99/max 1.20/3.40/15.25 ms, \
         recent 1.10/2.50/3.10, last 1.50, 1 overruns, 2 output drops"
    );
    let input = hypha_node::sdk::msgs::hypha_msgs::RequiredInput {
        topic: "/a".to_owned(),
        verdict: hypha_node::sdk::msgs::hypha_msgs::RequiredInput::USABLE,
        stagnant: true,
        ignored: true,
        dropped: 4,
        rejected: 5,
        undecodable: 6,
        ..hypha_node::sdk::msgs::hypha_msgs::RequiredInput::default()
    };
    assert_eq!(input_counters_text(&input), "4/5/6");
    let (verdict, _) = format_input_verdict(Some(&input), true);
    assert_eq!(verdict, "● USABLE STAGNANT IGNORED");
}

fn render_topics(state: &mut AppState) -> String {
    let mut terminal = Terminal::new(TestBackend::new(80, 16)).unwrap();
    terminal.draw(|f| draw_topics_tab(f, f.area(), state)).unwrap();
    let buffer = terminal.backend().buffer();
    buffer.content.iter().map(ratatui::buffer::Cell::symbol).collect()
}

#[test]
fn the_topics_table_scrolls_to_the_selection_and_keeps_its_offset() {
    let mut state = AppState::new();
    for index in 0..60 {
        state.on_sample(&format!("/t{index:02}"), 1, &[0]);
    }
    let first = render_topics(&mut state);
    assert!(first.contains("/t00") && !first.contains("/t50"));

    state.selected_topic = 50;
    let scrolled = render_topics(&mut state);
    assert!(scrolled.contains("/t50") && !scrolled.contains("/t00"), "{scrolled}");

    // Moving back up one row keeps the view where it is, as a list does.
    let offset = state.topics_table.offset();
    state.selected_topic = 49;
    render_topics(&mut state);
    assert_eq!(state.topics_table.offset(), offset);
}

type Input = hypha_node::sdk::msgs::hypha_msgs::RequiredInput;

fn input(topic: &str, md5: &str, verdict: u8) -> Input {
    Input {
        topic: topic.to_owned(),
        data_type: "t/R".to_owned(),
        md5: md5.to_owned(),
        verdict,
        ..Input::default()
    }
}

fn other(
    data_type: &str,
    md5: &str,
    schema_hash: u64,
) -> hypha_node::sdk::msgs::hypha_msgs::TopicSchema {
    hypha_node::sdk::msgs::hypha_msgs::TopicSchema {
        schema_hash,
        data_type: data_type.to_owned(),
        md5: md5.to_owned(),
    }
}

#[test]
fn other_schemas_are_shown_as_not_checked_none_or_each_version() {
    let mut waiting = input("/a", "b3087778e93fcd34cc8d65bc54e850d1", Input::NEVER_ARRIVED);
    assert_eq!(other_schemas_cell(None, false).0, "-");
    assert_eq!(
        other_schemas_cell(Some(&waiting), false).0,
        "not checked",
        "before the first check"
    );

    waiting.other_schemas_checked = true;
    assert_eq!(other_schemas_cell(Some(&waiting), false).0, "none");

    waiting.other_schemas =
        vec![other("t/R", "f79835def461350458ead76f07016dd7", 1), other("", "", 0x0102)];
    let (text, style) = other_schemas_cell(Some(&waiting), false);
    assert_eq!(text, "t/R (f79835de), ? (0000000000000102)");
    assert_eq!(style.fg, Some(ERR), "none of its own version is published: a mismatch");
    assert_eq!(other_schemas_cell(Some(&waiting), true).1.fg, Some(WARN), "a migration");
}

#[test]
fn an_input_that_stays_usable_on_an_old_value_is_red_when_only_another_version_is_published() {
    let mut stale_value = input("/a", "b3087778e93fcd34cc8d65bc54e850d1", Input::USABLE);
    stale_value.other_schemas_checked = true;
    stale_value.other_schemas = vec![other("t/R", "f79835def461350458ead76f07016dd7", 1)];
    let own_is_published = false;
    assert_eq!(other_schemas_cell(Some(&stale_value), own_is_published).1.fg, Some(ERR));
}

#[test]
fn the_verdict_of_a_binding_is_the_one_of_its_own_stream() {
    let state = hypha_node::sdk::msgs::hypha_msgs::NodeState {
        inputs: vec![
            input("/a", "b3087778e93fcd34cc8d65bc54e850d1", Input::USABLE),
            input("/a", "f79835def461350458ead76f07016dd7", Input::NEVER_ARRIVED),
        ],
        ..hypha_node::sdk::msgs::hypha_msgs::NodeState::default()
    };
    let binding = hypha_node::sdk::msgs::hypha_msgs::IntrospectedInput {
        topic: "/a".to_owned(),
        ..hypha_node::sdk::msgs::hypha_msgs::IntrospectedInput::default()
    };
    let second = verdict_of(&state, &binding, "f79835def461350458ead76f07016dd7").unwrap();
    assert_eq!(second.verdict, Input::NEVER_ARRIVED);
    let first = verdict_of(&state, &binding, "b3087778e93fcd34cc8d65bc54e850d1").unwrap();
    assert_eq!(first.verdict, Input::USABLE);
    assert!(verdict_of(&state, &binding, "00000000000000000000000000000000").is_none());
    assert!(
        verdict_of(&state, &binding, "").is_none(),
        "with no MD5 and two inputs on the topic, neither is its own"
    );
}

#[test]
fn two_versions_of_a_topic_are_two_rows_each_with_its_type_and_md5() {
    let mut state = AppState::new();
    let (md5_1, md5_2) = ("b3087778e93fcd34cc8d65bc54e850d1", "f79835def461350458ead76f07016dd7");
    for (md5, hash) in [(md5_1, 1_u64), (md5_2, 2)] {
        state.on_sample("/odom", hash, &[0]);
        state.apply_schema("/odom", hash, "t/R", md5);
    }
    let shown = render_topics(&mut state);
    assert_eq!(shown.matches("/odom").count(), 2, "{shown}");
    assert!(shown.contains("t/R (b3087778)") && shown.contains("t/R (f79835de)"), "{shown}");
    assert_eq!(state.topics.names().count(), 1, "a name is one topic");
    assert_eq!(state.topics.stream("/odom", md5_2).map(|stats| stats.schema_hash), Some(2));
}

#[test]
fn the_nodes_list_says_when_an_input_waits_while_another_version_is_published() {
    let mut waiting = input("/a", "b3087778e93fcd34cc8d65bc54e850d1", Input::NEVER_ARRIVED);
    let node = |inputs: Vec<Input>| {
        (
            hypha_node::sdk::msgs::hypha_msgs::NodeState {
                inputs,
                ..hypha_node::sdk::msgs::hypha_msgs::NodeState::default()
            },
            Instant::now(),
        )
    };
    let shown = |state: &(hypha_node::sdk::msgs::hypha_msgs::NodeState, Instant)| {
        format_node_inputs(Some(state), None, &AppState::new(), false, Style::default()).0
    };
    assert_eq!(shown(&node(vec![waiting.clone()])), "0/1 (1 WAIT)");

    waiting.other_schemas = vec![other("t/R", "f79835def461350458ead76f07016dd7", 1)];
    assert_eq!(shown(&node(vec![waiting.clone()])), "0/1 (1 WAIT, 1 OTHER VERSION)");

    // An input without a limit stays usable on its last value: the other version is the news.
    waiting.verdict = Input::USABLE;
    assert_eq!(shown(&node(vec![waiting])), "1/1 OK, 1 OTHER VERSION");
}
