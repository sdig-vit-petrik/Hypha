//! The checks an application reports about itself, for `~/state`.
//!
//! An application knows what its inputs cannot say: the device is connected, the fix is
//! held, the calibration is loaded. It reports each such fact as a named check with a level
//! (the levels of `diagnostic_msgs/DiagnosticStatus`) and a message, and `node_state`
//! carries them (`hypha_msgs/NodeCheck`). The rule that turns a check into the state of the
//! node is in `node_state`, in one place.
//!
//! A report is a whole set, replacing the previous set of its owner (a
//! `hypha::DiagnosticUpdater`; several may share a node). A report is bounded, so that
//! reading the state stays bounded: at most [`MAX_CHECKS`] checks in all, a name of at most
//! [`MAX_NAME_BYTES`] bytes, a message cut at [`MAX_MESSAGE_BYTES`] bytes.

use std::os::raw::c_char;

use hypha_msgs::hypha_msgs::NodeCheck;

use crate::node::HyphaNode;
use crate::status::Error;

/// The most checks one node reports, all owners together.
pub const MAX_CHECKS: usize = 64;
/// The longest name of a check, in bytes.
pub const MAX_NAME_BYTES: usize = 128;
/// The longest message of a check, in bytes; a longer one is cut and ends in `...`.
pub const MAX_MESSAGE_BYTES: usize = 256;

const CUT_MARK: &str = "...";

/// One check as the C ABI passes it: borrowed for the call that receives it and copied
/// before that call returns. A null pointer is allowed only for a length of zero.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct HyphaNodeCheck {
    /// The name bytes: UTF-8, not NUL-terminated, 1 to `MAX_NAME_BYTES` (128) bytes.
    pub name: *const c_char,
    /// The byte count of `name`.
    pub name_len: usize,
    /// 0 `OK`, 1 `WARN`, 2 `ERROR`, 3 `STALE`, the levels of `diagnostic_msgs`.
    pub level: u8,
    /// The message bytes: UTF-8, not NUL-terminated; empty allowed. At most 256 bytes: a
    /// longer message is cut, at a character boundary, and ends in `...`.
    pub message: *const c_char,
    /// The byte count of `message`.
    pub message_len: usize,
}

/// The sets of checks the owners of a node reported last.
#[derive(Debug, Default)]
pub(crate) struct NodeChecks {
    owners: Vec<(u64, Vec<NodeCheck>)>,
}

impl NodeChecks {
    /// Replace the set of `owner`; an empty set removes the owner. Nothing changes when a
    /// check is refused.
    fn replace(&mut self, owner: u64, checks: Vec<NodeCheck>) -> Result<(), Error> {
        let others: usize =
            self.owners.iter().filter(|(known, _)| *known != owner).map(|(_, set)| set.len()).sum();
        if others.saturating_add(checks.len()) > MAX_CHECKS {
            return Err(crate::last_error::refuse(
                Error::TooLarge,
                &format!("a node reports at most {MAX_CHECKS} checks"),
            ));
        }
        let known = self.owners.iter().position(|(known, _)| *known == owner);
        match (known, checks.is_empty()) {
            (Some(at), true) => {
                self.owners.remove(at);
            }
            (Some(at), false) => {
                if let Some((_, set)) = self.owners.get_mut(at) {
                    *set = checks;
                }
            }
            (None, false) => self.owners.push((owner, checks)),
            (None, true) => {}
        }
        Ok(())
    }

    /// Every check, the sets in the order their owners first reported.
    pub(crate) fn all(&self) -> Vec<NodeCheck> {
        self.owners.iter().flat_map(|(_, set)| set.iter().cloned()).collect()
    }
}

/// `message` as at most [`MAX_MESSAGE_BYTES`] bytes: a longer one is cut at a character
/// boundary and ends in `...`.
fn bounded_message(message: &str) -> String {
    if message.len() <= MAX_MESSAGE_BYTES {
        return message.to_owned();
    }
    let keep = MAX_MESSAGE_BYTES.saturating_sub(CUT_MARK.len());
    let end = (0..=keep).rev().find(|&at| message.is_char_boundary(at)).unwrap_or(0);
    format!("{}{CUT_MARK}", message.get(..end).unwrap_or_default())
}

/// The row of one check, as `~/state` carries it: the message is bounded.
///
/// # Errors
/// `InvalidArgument` for an empty or too long name, or an unknown level.
pub fn check_row(name: &str, level: u8, message: &str) -> Result<NodeCheck, Error> {
    if name.is_empty() || name.len() > MAX_NAME_BYTES {
        return Err(crate::last_error::refuse(
            Error::InvalidArgument,
            &format!("a check has a name of 1 to {MAX_NAME_BYTES} bytes, not {}", name.len()),
        ));
    }
    if level > NodeCheck::STALE {
        return Err(crate::last_error::refuse(
            Error::InvalidArgument,
            &format!("check '{name}': level {level} is none of OK, WARN, ERROR, STALE"),
        ));
    }
    Ok(NodeCheck { name: name.to_owned(), level, message: bounded_message(message) })
}

/// Report the checks of `owner` to `node`, replacing what `owner` reported before; an empty
/// slice removes them. `~/state` carries them from its next reading.
///
/// # Errors
/// `TooLarge` when the checks of all owners would exceed `MAX_CHECKS`. A refusal leaves the
/// set of `owner` as it was.
pub fn node_set_checks(node: &HyphaNode, owner: u64, checks: Vec<NodeCheck>) -> Result<(), Error> {
    crate::locked(&node.checks).replace(owner, checks)
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
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
}
