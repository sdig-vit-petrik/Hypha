//! The replay progress that the other nodes report on `/clock_progress`.
//!
//!
//! The node's own reports are never kept.

use std::collections::BTreeMap;

use hypha_core::message::DecodeOwned as _;
use hypha_msgs::hypha_msgs::ReplayProgress;
use hypha_wire::codec::CodecError;

/// One node's newest report: the clock it reached, and the send time of the sample
/// it answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Report {
    /// The simulated time the node reached, in nanoseconds.
    pub clock_nanos: u64,
    /// The send time of the `/clock` sample the report answers.
    pub sent_nanos: u64,
}

/// The newest report of each other node, by node name, and the missing-sequence count
/// the progress input last logged.
#[derive(Default)]
pub struct UpstreamReports {
    reports: BTreeMap<String, Report>,
    missing_logged: u64,
}

impl UpstreamReports {
    /// Keep `report` as the newest one of `node`.
    pub fn store(&mut self, node: String, report: Report) {
        self.reports.insert(node, report);
    }

    /// The clock `name` reported for a sample sent at or after `epoch`, if it has.
    #[must_use]
    pub fn valid_clock(&self, name: &str, epoch: u64) -> Option<u64> {
        self.reports
            .get(name)
            .filter(|report| report.sent_nanos >= epoch)
            .map(|report| report.clock_nanos)
    }

    /// Log an error when the progress input has missed more reports than it logged last.
    /// A missed report means a downstream may wait for a clock it never receives.
    pub fn note_missing(&mut self, missing: u64) {
        if missing > self.missing_logged {
            self.missing_logged = missing;
            log::error!(target: "hypha", "hypha: the replay progress input has missed {missing} report(s) to a sequence gap, so a node may wait for a clock it did not receive");
        }
    }
}

/// Decode a report payload. It returns the sending node and its report, or `None` when
/// the report is `own_name`'s own.
///
/// # Errors
/// Returns the decoding error of a payload that is not a `ReplayProgress`.
pub fn decode_report(
    own_name: &str,
    payload: &[u8],
) -> Result<Option<(String, Report)>, CodecError> {
    let report = ReplayProgress::decode_from(payload)?;
    if report.node == own_name {
        return Ok(None);
    }
    let kept = Report { clock_nanos: report.clock_nanos, sent_nanos: report.clock_sent_nanos };
    Ok(Some((report.node, kept)))
}

#[cfg(test)]
#[allow(clippy::arithmetic_side_effects, clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use hypha_core::message::EncodeInto as _;

    use super::*;

    #[test]
    fn a_report_counts_only_when_it_was_sent_at_or_after_the_epoch() {
        let mut reports = UpstreamReports::default();
        assert_eq!(reports.valid_clock("up", 0), None);
        reports.store("up".to_owned(), Report { clock_nanos: 500, sent_nanos: 40 });
        assert_eq!(reports.valid_clock("up", 0), Some(500));
        assert_eq!(reports.valid_clock("up", 40), Some(500));
        assert_eq!(reports.valid_clock("up", 41), None);
        reports.store("up".to_owned(), Report { clock_nanos: 600, sent_nanos: 50 });
        assert_eq!(reports.valid_clock("up", 41), Some(600));
        assert_eq!(reports.valid_clock("other", 0), None);
    }

    #[test]
    fn a_report_of_another_node_is_kept_and_its_own_is_dropped() {
        let message = ReplayProgress { node: "up".to_owned(), clock_nanos: 7, clock_sent_nanos: 9 };
        let mut payload = vec![0; message.encoded_len()];
        message.encode_into(&mut payload).expect("generated encoder");
        let kept = decode_report("me", &payload).unwrap();
        assert_eq!(kept, Some(("up".to_owned(), Report { clock_nanos: 7, sent_nanos: 9 })));
        assert_eq!(decode_report("up", &payload).unwrap(), None);
        assert!(decode_report("me", &[1, 2]).is_err());
    }
}
