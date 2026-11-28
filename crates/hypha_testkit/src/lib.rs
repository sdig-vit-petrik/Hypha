//! Helpers shared by the tests of the workspace.

mod env;

pub use env::{remove_env, set_env};

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// The last suffix [`unique_name`] gave in this process.
static LAST: AtomicU64 = AtomicU64::new(0);

/// A name no other test of this run and no leftover of an earlier run holds:
/// `<label>_<process id>_<n>`, where `n` is the UTC time in nanoseconds, kept unique
/// within this process.
#[must_use]
pub fn unique_name(label: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX));
    let mut previous = LAST.load(Ordering::Relaxed);
    let suffix = loop {
        let next = nanos.max(previous.saturating_add(1));
        match LAST.compare_exchange_weak(previous, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break next,
            Err(current) => previous = current,
        }
    };
    format!("{label}_{}_{suffix}", std::process::id())
}

#[cfg(test)]
mod tests {
    use super::unique_name;

    #[test]
    fn two_names_of_one_label_differ_and_keep_the_label() {
        let first = unique_name("node");
        let second = unique_name("node");
        assert_ne!(first, second);
        assert!(first.starts_with(&format!("node_{}_", std::process::id())), "{first}");
    }
}
