//! The rules of a node's application checks: replace per owner, bounds, names, levels.
#![allow(clippy::expect_used, reason = "a failed setup is a test failure")]

use super::*;

fn check(name: &str, level: u8) -> NodeCheck {
    check_row(name, level, "message").expect("a valid check")
}

#[test]
fn a_set_replaces_the_one_of_its_owner_and_leaves_the_others() {
    let mut checks = NodeChecks::default();
    checks.replace(1, vec![check("A", 0), check("B", 1)]).expect("fits");
    checks.replace(2, vec![check("C", 2)]).expect("fits");
    checks.replace(1, vec![check("A", 2)]).expect("fits");
    let all: Vec<_> = checks.all().into_iter().map(|c| (c.name, c.level)).collect();
    assert_eq!(all, [("A".to_owned(), 2), ("C".to_owned(), 2)]);
    checks.replace(2, Vec::new()).expect("an empty set removes");
    assert_eq!(checks.all().len(), 1);
}

#[test]
fn a_report_over_the_bound_is_refused_and_changes_nothing() {
    let mut checks = NodeChecks::default();
    checks.replace(1, vec![check("A", 0)]).expect("fits");
    let many: Vec<_> = (0..MAX_CHECKS).map(|index| check(&format!("n{index}"), 0)).collect();
    assert_eq!(checks.replace(2, many.clone()), Err(Error::TooLarge));
    assert_eq!(checks.all().len(), 1, "a refused report changes nothing");
    checks.replace(1, many).expect("the only owner may fill the bound");
    assert_eq!(checks.all().len(), MAX_CHECKS);
}

#[test]
fn a_name_and_a_level_are_checked() {
    assert_eq!(check_row("", 0, "").err(), Some(Error::InvalidArgument));
    assert_eq!(
        check_row(&"n".repeat(MAX_NAME_BYTES + 1), 0, "").err(),
        Some(Error::InvalidArgument)
    );
    assert!(check_row(&"n".repeat(MAX_NAME_BYTES), 0, "").is_ok());
    assert_eq!(check_row("A", 4, "").err(), Some(Error::InvalidArgument));
    assert!(crate::last_error::last_error().contains("level 4"));
}

#[test]
fn a_long_message_is_cut_on_a_character_and_says_so() {
    let long = "é".repeat(MAX_MESSAGE_BYTES);
    let cut = bounded_message(&long);
    assert!(cut.len() <= MAX_MESSAGE_BYTES, "{}", cut.len());
    assert!(cut.ends_with(CUT_MARK));
    assert!(cut.starts_with('é'));
    assert_eq!(bounded_message("short"), "short");
    let exact = "x".repeat(MAX_MESSAGE_BYTES);
    assert_eq!(bounded_message(&exact), exact, "a message at the bound is whole");
}
