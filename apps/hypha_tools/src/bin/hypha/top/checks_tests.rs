//! The check rows and the badge of `hypha top`.

use super::*;

type Input = hypha_node::sdk::msgs::hypha_msgs::RequiredInput;

fn input(topic: &str, md5: &str, verdict: u8) -> Input {
    Input { topic: topic.to_owned(), md5: md5.to_owned(), verdict, ..Input::default() }
}

#[test]
fn the_card_lists_failing_checks_first_and_counts_what_it_does_not_show() {
    type Check = hypha_node::sdk::msgs::hypha_msgs::NodeCheck;
    let check = |name: &str, level: u8, message: &str| Check {
        name: name.to_owned(),
        level,
        message: message.to_owned(),
    };
    let state = |checks: Vec<Check>| hypha_node::sdk::msgs::hypha_msgs::NodeState {
        checks,
        ..hypha_node::sdk::msgs::hypha_msgs::NodeState::default()
    };
    let text_color = |lines: &[Line<'_>], at: usize| {
        lines.get(at).and_then(|line| line.spans.get(1)).and_then(|span| span.style.fg)
    };
    let text = |lines: &[Line<'_>]| -> Vec<String> {
        lines
            .iter()
            .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
            .collect()
    };

    assert!(check_lines(&state(Vec::new())).is_empty(), "a node with no checks has no line");
    assert_eq!(
        text(&check_lines(&state(vec![check("A", Check::OK, "fine"), check("B", Check::OK, "")]))),
        [" Check: 2 OK"]
    );

    let mixed = state(vec![
        check("Rate", Check::WARN, "8 Hz"),
        check("Device", Check::ERROR, "serial port absent"),
        check("Fix", Check::STALE, ""),
        check("Calibration", Check::OK, "loaded"),
    ]);
    let lines = check_lines(&mixed);
    assert_eq!(
        text(&lines),
        [
            " Check: Device: ERROR serial port absent",
            " Check: Fix: STALE",
            " Check: Rate: WARN 8 Hz",
        ],
        "the failing checks first, the OK one not at all"
    );
    assert_eq!(text_color(&lines, 0), Some(ERR));
    assert_eq!(text_color(&lines, 2), Some(WARN), "a WARN is not red");

    let many = state((0..7).map(|index| check(&format!("c{index}"), Check::ERROR, "")).collect());
    let shown = text(&check_lines(&many));
    assert_eq!(shown.len(), CHECK_LINES_SHOWN + 1);
    assert_eq!(shown.last().map(String::as_str), Some(" Check: ... and 3 more"));
    assert_eq!(check_line_text(&check("X", 9, "")), "X: LEVEL 9");
}

#[test]
fn the_badge_of_a_degraded_node_names_the_input_the_check_or_both() {
    type State = hypha_node::sdk::msgs::hypha_msgs::NodeState;
    type Check = hypha_node::sdk::msgs::hypha_msgs::NodeCheck;
    let failed = Check { level: Check::ERROR, ..Check::default() };
    let degraded = |inputs: Vec<Input>, checks: Vec<Check>| State {
        state: State::DEGRADED,
        inputs,
        checks,
        ..State::default()
    };
    let stale = input("/a", "b3087778e93fcd34cc8d65bc54e850d1", Input::STALE);
    let usable = input("/a", "b3087778e93fcd34cc8d65bc54e850d1", Input::USABLE);
    assert_eq!(
        degraded_badge(&degraded(vec![stale.clone()], vec![])),
        "[✖ DEGRADED / STALE INPUTS]"
    );
    assert_eq!(
        degraded_badge(&degraded(vec![usable.clone()], vec![failed.clone()])),
        "[✖ DEGRADED / FAILED CHECK]"
    );
    assert_eq!(
        degraded_badge(&degraded(vec![], vec![failed.clone()])),
        "[✖ DEGRADED / FAILED CHECK]"
    );
    assert_eq!(
        degraded_badge(&degraded(vec![stale], vec![failed])),
        "[✖ DEGRADED / STALE INPUTS, FAILED CHECK]"
    );
}
