//! The application checks of a node in `hypha top` (#121): their rows on the node card and
//! the badge of a node they make `DEGRADED`.

use hypha_tools::theme::{ERR, MUTED, OK, WARN};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// The most checks shown line by line; the rest are counted.
const CHECK_LINES_SHOWN: usize = 4;

/// The word of a check level, as `diagnostic_msgs` names it.
pub(super) fn check_level_word(level: u8) -> String {
    use hypha_node::sdk::msgs::hypha_msgs::NodeCheck;
    match level {
        NodeCheck::OK => "OK".to_owned(),
        NodeCheck::WARN => "WARN".to_owned(),
        NodeCheck::ERROR => "ERROR".to_owned(),
        NodeCheck::STALE => "STALE".to_owned(),
        other => format!("LEVEL {other}"),
    }
}

/// Whether the state counts the check against the node: any level but `OK` and `WARN`.
pub(super) fn check_blocks(check: &hypha_node::sdk::msgs::hypha_msgs::NodeCheck) -> bool {
    use hypha_node::sdk::msgs::hypha_msgs::NodeCheck;
    !matches!(check.level, NodeCheck::OK | NodeCheck::WARN)
}

/// One line of a check that is not `OK`: its name, level and message.
pub(super) fn check_line_text(check: &hypha_node::sdk::msgs::hypha_msgs::NodeCheck) -> String {
    let word = check_level_word(check.level);
    if check.message.is_empty() {
        format!("{}: {word}", check.name)
    } else {
        format!("{}: {word} {}", check.name, check.message)
    }
}

/// The lines of the application's checks on the node card: each check that is not `OK`
/// (the failing ones first, at most [`CHECK_LINES_SHOWN`]) and a count of the rest; one
/// line when every check is `OK`; none when the node reports none.
pub(super) fn check_lines(
    state: &hypha_node::sdk::msgs::hypha_msgs::NodeState,
) -> Vec<Line<'static>> {
    use hypha_node::sdk::msgs::hypha_msgs::NodeCheck;
    let label = || Span::styled(" Check: ", Style::default().add_modifier(Modifier::BOLD));
    if state.checks.is_empty() {
        return Vec::new();
    }
    let mut unwell: Vec<&NodeCheck> =
        state.checks.iter().filter(|check| check.level != NodeCheck::OK).collect();
    if unwell.is_empty() {
        let text = format!("{} OK", state.checks.len());
        return vec![Line::from(vec![label(), Span::styled(text, Style::default().fg(OK))])];
    }
    unwell.sort_by_key(|check| !check_blocks(check));
    let mut lines: Vec<Line<'static>> = unwell
        .iter()
        .take(CHECK_LINES_SHOWN)
        .map(|check| {
            let color = if check_blocks(check) { ERR } else { WARN };
            let style = Style::default().fg(color).add_modifier(Modifier::BOLD);
            Line::from(vec![label(), Span::styled(check_line_text(check), style)])
        })
        .collect();
    if let Some(more) = unwell.len().checked_sub(CHECK_LINES_SHOWN).filter(|more| *more > 0) {
        lines
            .push(Line::styled(format!(" Check: ... and {more} more"), Style::default().fg(MUTED)));
    }
    lines
}

/// The badge of a `DEGRADED` node, with what degrades it: an input that is not usable, a
/// check at `ERROR` or `STALE`.
fn degraded_badge(state: &hypha_node::sdk::msgs::hypha_msgs::NodeState) -> String {
    use hypha_node::sdk::msgs::hypha_msgs::RequiredInput;
    let input_not_ok = state
        .inputs
        .iter()
        .any(|input| !input.ignored && (input.verdict != RequiredInput::USABLE || input.stagnant));
    let mut reasons = Vec::new();
    if input_not_ok {
        reasons.push("STALE INPUTS");
    }
    if state.checks.iter().any(check_blocks) {
        reasons.push("FAILED CHECK");
    }
    if reasons.is_empty() {
        return "[✖ DEGRADED]".to_owned();
    }
    format!("[✖ DEGRADED / {}]", reasons.join(", "))
}

/// The health badge of a node's card and its style: a `DEGRADED` node names what made it so.
pub(super) fn state_badge(st: &hypha_node::sdk::msgs::hypha_msgs::NodeState) -> (String, Style) {
    let bold = |color| Style::default().fg(color).add_modifier(Modifier::BOLD);
    match st.state {
        hypha_node::sdk::msgs::hypha_msgs::NodeState::INPUTS_USABLE => {
            ("[● ALL INPUTS USABLE]".to_owned(), bold(OK))
        }
        hypha_node::sdk::msgs::hypha_msgs::NodeState::WAITING => {
            ("[▲ WAITING FOR INPUTS]".to_owned(), bold(WARN))
        }
        hypha_node::sdk::msgs::hypha_msgs::NodeState::DEGRADED => (degraded_badge(st), bold(ERR)),
        _ => ("[UNKNOWN STATE]".to_owned(), Style::default().fg(MUTED)),
    }
}

#[cfg(test)]
#[path = "checks_tests.rs"]
mod tests;
