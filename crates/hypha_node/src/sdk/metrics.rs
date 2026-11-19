//! Reads the named counters that a node or subscriber emits.

/// Collect every metric `read` emits, in order, as owned names and values.
pub(crate) fn collect(read: impl FnOnce(&mut dyn FnMut(&str, u64))) -> Vec<(String, u64)> {
    let mut list: Vec<(String, u64)> = Vec::new();
    let mut push = |name: &str, value: u64| list.push((name.to_owned(), value));
    read(&mut push);
    list
}
