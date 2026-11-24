//! Removing the shared memory of the bus domains nobody uses: the rules of `hypha clean`.
//!
//! A domain is kept while one of its nodes lives (iceoryx2's node monitor tells it, and
//! a node of another user or in an undefined state counts as living), or while its run
//! lives: under a prefix, the process whose id follows the prefix in the domain's name
//! (a test domain is `t_<pid of its run>_...`). Every resource of any other matching
//! domain is removed; nothing is killed.

use std::collections::BTreeMap;
use std::fmt;

use hypha_iceoryx2::host_resources::{self, NodeLiveness, Resource};

/// The domains a clean covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainSelection {
    /// The domains whose names start with the prefix.
    Prefix(String),
    /// One domain; the empty name is the domain "none".
    Exact(String),
}

impl DomainSelection {
    /// The domains whose names start with `prefix`, a `[a-z][a-z0-9_]*` of at most 32
    /// characters.
    ///
    /// # Errors
    /// A message that names the refused prefix.
    pub fn prefix(prefix: &str) -> Result<Self, String> {
        if hypha_iceoryx2::domain::is_domain_name(prefix) {
            return Ok(Self::Prefix(prefix.to_owned()));
        }
        Err(format!(
            "'{prefix}' is not a domain prefix: use [a-z][a-z0-9_]*, at most {} characters",
            hypha_iceoryx2::domain::MAX_DOMAIN_LENGTH
        ))
    }

    fn covers(&self, domain: &str) -> bool {
        match self {
            Self::Prefix(prefix) => !domain.is_empty() && domain.starts_with(prefix.as_str()),
            Self::Exact(name) => domain == name,
        }
    }

    /// The process id that follows the prefix in `domain`, if digits follow it.
    fn run_of(&self, domain: &str) -> Option<u32> {
        let Self::Prefix(prefix) = self else {
            return None;
        };
        let rest = domain.strip_prefix(prefix.as_str())?;
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        digits.parse().ok()
    }
}

/// Why a domain is kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeepReason {
    /// One of its nodes lives.
    NodeAlive {
        /// The node's name, empty when unknown.
        name: String,
        /// Its process id, when known.
        pid: Option<i32>,
    },
    /// One of its nodes belongs to another user, so whether it lives is unknown.
    NodeInaccessible,
    /// One of its nodes is in an undefined state, so whether it lives is unknown.
    NodeUndefined,
    /// The run whose process id follows the prefix lives.
    RunAlive(u32),
}

impl fmt::Display for KeepReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NodeAlive { name, pid } => {
                let name = if name.is_empty() { "a node" } else { name.as_str() };
                match pid {
                    Some(pid) => write!(formatter, "node {name} of pid {pid} is alive"),
                    None => write!(formatter, "node {name} is alive"),
                }
            }
            Self::NodeInaccessible => {
                formatter.write_str("a node of another user, which this user cannot check")
            }
            Self::NodeUndefined => {
                formatter.write_str("a node in an undefined state, which may still live")
            }
            Self::RunAlive(pid) => write!(formatter, "its run, pid {pid}, is alive"),
        }
    }
}

/// A domain the clean removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovedDomain {
    /// The domain, empty for the domain "none".
    pub domain: String,
    /// How many resources it had.
    pub resources: usize,
}

/// A domain the clean kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeptDomain {
    /// The domain, empty for the domain "none".
    pub domain: String,
    /// Why.
    pub reason: KeepReason,
}

/// The outcome of a clean, worst first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanOutcome {
    /// A resource could not be read or removed.
    Failed,
    /// A covered domain is in use and was kept; others may have been removed.
    Kept,
    /// Every covered domain was removed.
    Removed,
    /// No resource of a covered domain was found.
    NothingToDo,
}

/// What a clean did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CleanReport {
    /// The domains removed, by name.
    pub removed: Vec<RemovedDomain>,
    /// The domains kept, by name.
    pub kept: Vec<KeptDomain>,
    /// A message for each resource or domain that could not be read or removed.
    pub failures: Vec<String>,
}

impl CleanReport {
    /// The outcome, worst first: a failure, a kept domain, a removal, nothing.
    #[must_use]
    pub fn outcome(&self) -> CleanOutcome {
        if !self.failures.is_empty() {
            CleanOutcome::Failed
        } else if !self.kept.is_empty() {
            CleanOutcome::Kept
        } else if !self.removed.is_empty() {
            CleanOutcome::Removed
        } else {
            CleanOutcome::NothingToDo
        }
    }
}

/// Removes every resource of the domains `selection` covers that nobody uses.
#[must_use]
pub fn clean(selection: &DomainSelection) -> CleanReport {
    let mut report = CleanReport::default();
    let resources = match host_resources::host_resources() {
        Ok(resources) => resources,
        Err(message) => {
            report.failures.push(message);
            return report;
        }
    };
    let mut by_domain: BTreeMap<String, Vec<Resource>> = BTreeMap::new();
    for (domain, resource) in resources.into_iter().filter(|(domain, _)| selection.covers(domain)) {
        by_domain.entry(domain).or_default().push(resource);
    }
    for (domain, resources) in by_domain {
        match keep_reason(selection, &domain) {
            Ok(Some(reason)) => report.kept.push(KeptDomain { domain, reason }),
            Ok(None) => {
                let failures: Vec<String> = resources
                    .iter()
                    .filter_map(|resource| host_resources::remove_resource(resource).err())
                    .collect();
                if failures.is_empty() {
                    report.removed.push(RemovedDomain { domain, resources: resources.len() });
                }
                report.failures.extend(failures);
            }
            Err(message) => report.failures.push(format!("{}: {message}", domain_label(&domain))),
        }
    }
    report
}

/// Why `domain` must stay, or `None` when nobody uses it.
fn keep_reason(selection: &DomainSelection, domain: &str) -> Result<Option<KeepReason>, String> {
    if let Some(pid) = selection.run_of(domain).filter(|pid| hypha_sys::process_lives(*pid)) {
        return Ok(Some(KeepReason::RunAlive(pid)));
    }
    let nodes = host_resources::domain_nodes(domain)?;
    Ok(nodes.into_iter().find_map(|node| match node {
        NodeLiveness::Alive { name, pid } => Some(KeepReason::NodeAlive { name, pid }),
        NodeLiveness::Inaccessible => Some(KeepReason::NodeInaccessible),
        NodeLiveness::Undefined => Some(KeepReason::NodeUndefined),
        NodeLiveness::Dead => None,
    }))
}

/// `domain t_1_2`, or `the domain "none"` for the empty name.
#[must_use]
pub fn domain_label(domain: &str) -> String {
    if domain.is_empty() { "the domain \"none\"".to_owned() } else { format!("domain {domain}") }
}

#[cfg(test)]
mod tests {
    use super::{
        CleanOutcome, CleanReport, DomainSelection, KeepReason, KeptDomain, RemovedDomain,
    };

    #[test]
    fn a_prefix_covers_the_named_domains_that_start_with_it_and_never_the_domain_none() {
        let selection = DomainSelection::Prefix("t_".to_owned());
        assert!(selection.covers("t_12_3"));
        assert!(!selection.covers("autoware"));
        assert!(!selection.covers(""));
        assert!(DomainSelection::Exact(String::new()).covers(""));
        assert!(!DomainSelection::Exact("t_1".to_owned()).covers("t_12"));
    }

    #[test]
    fn the_run_is_the_number_that_follows_the_prefix() {
        let selection = DomainSelection::Prefix("t_".to_owned());
        assert_eq!(selection.run_of("t_4711_3"), Some(4711));
        assert_eq!(selection.run_of("t_4711"), Some(4711));
        assert_eq!(selection.run_of("t_x_3"), None);
        assert_eq!(DomainSelection::Exact("t_4711_3".to_owned()).run_of("t_4711_3"), None);
    }

    #[test]
    fn an_invalid_prefix_is_refused() {
        for refused in ["", "Not-A-Prefix", "1abc", &"a".repeat(33)] {
            assert!(DomainSelection::prefix(refused).is_err(), "{refused}");
        }
        assert_eq!(DomainSelection::prefix("t_"), Ok(DomainSelection::Prefix("t_".to_owned())));
    }

    #[test]
    fn the_outcome_is_the_worst_thing_that_happened() {
        let removed = RemovedDomain { domain: "t_1".to_owned(), resources: 1 };
        let kept = KeptDomain { domain: "t_2".to_owned(), reason: KeepReason::RunAlive(2) };
        let mut report = CleanReport::default();
        assert_eq!(report.outcome(), CleanOutcome::NothingToDo);
        report.removed.push(removed);
        assert_eq!(report.outcome(), CleanOutcome::Removed);
        report.kept.push(kept);
        assert_eq!(report.outcome(), CleanOutcome::Kept);
        report.failures.push("cannot remove x".to_owned());
        assert_eq!(report.outcome(), CleanOutcome::Failed);
    }
}
