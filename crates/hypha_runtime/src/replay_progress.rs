//! The replay wire contract of the player, and the progress a node reports on each
//! `/clock` sample.

use std::collections::BTreeMap;

use hypha_wire::codec::CodecError;

/// The resolved definition of `hypha_msgs/ReplayProgress`.
pub const REPLAY_PROGRESS_DEFINITION: &str =
    "string node\nuint64 clock_nanos\nuint64 clock_sent_nanos";

/// The topic a node reports its replay progress on.
pub const PROGRESS_TOPIC: &str = "/clock_progress";

/// The reports the progress input keeps per publisher; iceoryx2 gives each publisher
/// connection its own queue.
pub const PROGRESS_INPUT_DEPTH: usize = 8;

/// Bytes of a `ReplayProgress` payload besides its node name.
const PROGRESS_FIXED_BYTES: usize = 20;

/// One `/clock` sample as the node read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockRead {
    /// The simulated time the sample carries, in nanoseconds.
    pub nanos: u64,
    /// The transport send time of the frame, on the host monotonic clock.
    pub sent_nanos: u64,
}

/// The newest `/clock` sample that a node has read, the newest it has reported, and the
/// newest offer of each `WaitSet` that runs a replay or triggered cycle.
#[derive(Debug, Default)]
pub struct ProgressReporter {
    /// The newest sample read by any `WaitSet` of the node. `None` before the first.
    newest: Option<ClockRead>,
    /// The send time and the clock of the newest report. `None` before the first one.
    last: Option<(u64, u64)>,
    /// The send time of the `/clock` sample a restarted process rejoined on; zero for a
    /// process that did not restart.
    epoch_sent: u64,
    /// The newest offer of each `WaitSet` with a replay or triggered cycle, by address.
    offers: BTreeMap<usize, ClockRead>,
}

/// What a `WaitSet` turn does with its progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressTurn {
    /// Nothing to offer: no sample is read yet, or it is not due on this turn.
    Hold,
    /// The `WaitSet` has no cycle whose steps the progress reports: forget its offer.
    Withdraw,
    /// Offer this clock as the progress of the `WaitSet`.
    Offer(ClockRead),
}

/// The progress turn of a `WaitSet`, given the node's newest `/clock` sample `snapshot`,
/// taken before the clock was read as `now_nanos`. A sample past `now_nanos` waits for
/// a later turn; the offer is capped by `frontier` (see [`reportable_clock`]).
#[must_use]
pub fn progress_turn(
    snapshot: Option<ClockRead>,
    now_nanos: i64,
    reports: impl FnOnce() -> bool,
    frontier: impl FnOnce() -> Option<i64>,
) -> ProgressTurn {
    let Some(read) = snapshot else {
        return ProgressTurn::Hold;
    };
    if !reports() {
        return ProgressTurn::Withdraw;
    }
    if !u64::try_from(now_nanos).is_ok_and(|now| read.nanos <= now) {
        return ProgressTurn::Hold;
    }
    let nanos = reportable_clock(read.nanos, frontier());
    ProgressTurn::Offer(ClockRead { nanos, sent_nanos: read.sent_nanos })
}

impl ProgressReporter {
    /// Record a `/clock` read.
    pub fn note_read(&mut self, read: ClockRead) {
        self.newest = Some(read);
    }

    /// Record `read`, the sample a restarted process rejoins the replay on.
    pub fn rejoin(&mut self, read: ClockRead) {
        self.newest = Some(read);
        self.epoch_sent = read.sent_nanos;
    }

    /// The send time of the sample this process rejoined on, or zero.
    #[must_use]
    pub fn epoch(&self) -> u64 {
        self.epoch_sent
    }

    /// The newest sample read so far, if any.
    #[must_use]
    pub fn newest(&self) -> Option<ClockRead> {
        self.newest
    }

    /// Whether `read` is covered by the newest report: it answers an earlier sample, or
    /// the same sample at a clock no later than the reported one.
    #[must_use]
    pub fn already_reported(&self, read: ClockRead) -> bool {
        self.last.is_some_and(|(sent, clock)| {
            read.sent_nanos < sent || (read.sent_nanos == sent && read.nanos <= clock)
        })
    }

    /// Record `read` as the newest report.
    pub fn mark_reported(&mut self, read: ClockRead) {
        self.last = Some((read.sent_nanos, read.nanos));
    }

    /// Replace the offer of the `WaitSet` keyed `waitset` with `read`.
    pub fn offer(&mut self, waitset: usize, read: ClockRead) {
        self.offers.insert(waitset, read);
    }

    /// Forget the offer of the `WaitSet` keyed `waitset`. Returns whether it had one.
    pub fn withdraw(&mut self, waitset: usize) -> bool {
        self.offers.remove(&waitset).is_some()
    }

    /// The report that is due now: the lowest offered clock, once every offer answers the
    /// same sample and the newest report does not cover it.
    #[must_use]
    pub fn due(&self) -> Option<ClockRead> {
        let mut offers = self.offers.values();
        let first = *offers.next()?;
        let mut lowest = first.nanos;
        for offered in offers {
            if offered.sent_nanos != first.sent_nanos {
                return None;
            }
            lowest = lowest.min(offered.nanos);
        }
        let read = ClockRead { nanos: lowest, sent_nanos: first.sent_nanos };
        (!self.already_reported(read)).then_some(read)
    }
}

/// The payload length of a `ReplayProgress` that names `name`, or `None` when it is not
/// representable.
#[must_use]
pub fn progress_payload_len(name: &str) -> Option<usize> {
    name.len().checked_add(PROGRESS_FIXED_BYTES)
}

/// ROS 1 layout of `ReplayProgress`, written straight into `payload`.
///
/// # Errors
/// Returns a [`CodecError`] when `payload` is too short for the message.
pub fn encode_progress(payload: &mut [u8], name: &str, read: ClockRead) -> Result<(), CodecError> {
    let mut writer = hypha_wire::slice_writer::SliceWriter::new(payload);
    writer.write_str(name)?;
    writer.write_u64(read.nanos)?;
    writer.write_u64(read.sent_nanos)
}

/// The clock a replay node may report for `read`: `read` when `frontier` is `None`,
/// else one nanosecond before `frontier`, never below zero.
#[must_use]
pub fn reportable_clock(read: u64, frontier: Option<i64>) -> u64 {
    let Some(frontier) = frontier else {
        return read;
    };
    let before = frontier.checked_sub(1).and_then(|nanos| u64::try_from(nanos).ok());
    read.min(before.unwrap_or(0))
}

#[cfg(test)]
#[allow(clippy::arithmetic_side_effects, clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use hypha_core::message::EncodeInto as _;
    use hypha_msgs::hypha_msgs::ReplayProgress;

    use super::*;

    /// Encode a `ReplayProgress` with `message`'s generated encoder, for the tests.
    fn generated_progress(name: &str, read: ClockRead) -> Vec<u8> {
        let message = ReplayProgress {
            node: name.to_owned(),
            clock_nanos: read.nanos,
            clock_sent_nanos: read.sent_nanos,
        };
        let mut payload = vec![0; message.encoded_len()];
        message.encode_into(&mut payload).expect("generated encoder");
        payload
    }

    #[test]
    fn borrowed_progress_encoding_matches_the_generated_wire_layout() {
        for name in ["", "tracker", "node_\u{3bb}"] {
            for (nanos, sent_nanos) in [(0, 0), (u64::MAX, 42), (42, u64::MAX)] {
                let read = ClockRead { nanos, sent_nanos };
                let expected = generated_progress(name, read);
                let mut actual = vec![0; expected.len()];
                encode_progress(&mut actual, name, read).expect("borrowed encoder");
                assert_eq!(actual, expected);
                actual.pop();
                assert!(encode_progress(&mut actual, name, read).is_err());
            }
        }
    }

    /// The report of the longest node name fits [`PROGRESS_MAX_PAYLOAD_BYTES`].
    #[test]
    fn the_longest_node_name_fits_the_progress_payload_limit() {
        use crate::clock_barrier::PROGRESS_MAX_PAYLOAD_BYTES;
        let name = "n".repeat(hypha_core::port::MAX_NODE_NAME_LENGTH);
        let read = ClockRead { nanos: u64::MAX, sent_nanos: u64::MAX };
        let encoded = generated_progress(&name, read);
        assert_eq!(encoded.len(), PROGRESS_MAX_PAYLOAD_BYTES);
        assert_eq!(progress_payload_len(&name), Some(PROGRESS_MAX_PAYLOAD_BYTES));
    }

    #[test]
    fn the_payload_length_covers_the_name_and_the_fixed_fields() {
        for name in ["", "tracker", "node_\u{3bb}"] {
            let read = ClockRead { nanos: 1, sent_nanos: 2 };
            assert_eq!(progress_payload_len(name), Some(generated_progress(name, read).len()));
        }
    }

    #[test]
    fn only_a_rejoin_starts_an_epoch() {
        let mut reporter = ProgressReporter::default();
        reporter.note_read(ClockRead { nanos: 200, sent_nanos: 20 });
        reporter.note_read(ClockRead { nanos: 50, sent_nanos: 30 });
        assert_eq!(reporter.epoch(), 0, "a lower value read is no new timeline");
        reporter.rejoin(ClockRead { nanos: 50, sent_nanos: 40 });
        assert_eq!(reporter.epoch(), 40);
        assert_eq!(reporter.newest(), Some(ClockRead { nanos: 50, sent_nanos: 40 }));
    }

    #[test]
    fn a_read_is_reported_once_and_an_older_or_equal_one_is_already_covered() {
        let mut reporter = ProgressReporter::default();
        let first = ClockRead { nanos: 100, sent_nanos: 10 };
        assert!(!reporter.already_reported(first));
        reporter.mark_reported(first);
        assert!(reporter.already_reported(first));
        assert!(reporter.already_reported(ClockRead { nanos: 90, sent_nanos: 10 }));
        assert!(reporter.already_reported(ClockRead { nanos: 1, sent_nanos: 9 }));
        assert!(!reporter.already_reported(ClockRead { nanos: 101, sent_nanos: 10 }));
        assert!(!reporter.already_reported(ClockRead { nanos: 1, sent_nanos: 11 }));
    }

    #[test]
    fn the_reportable_clock_stops_one_nanosecond_before_the_earliest_unrun_deadline() {
        assert_eq!(reportable_clock(500, None), 500);
        assert_eq!(reportable_clock(500, Some(1_000)), 500);
        assert_eq!(reportable_clock(500, Some(300)), 299);
        assert_eq!(reportable_clock(500, Some(501)), 500);
        assert_eq!(reportable_clock(500, Some(1)), 0);
        assert_eq!(reportable_clock(500, Some(0)), 0);
        assert_eq!(reportable_clock(500, Some(i64::MIN)), 0);
    }

    #[test]
    fn the_node_reports_the_lowest_offer_once_every_offer_answers_one_sample() {
        let mut reporter = ProgressReporter::default();
        let early = ClockRead { nanos: 200, sent_nanos: 10 };
        reporter.offer(1, ClockRead { nanos: 300, sent_nanos: 10 });
        reporter.offer(2, early);
        assert_eq!(reporter.due(), Some(early));
        reporter.mark_reported(early);
        assert_eq!(reporter.due(), None, "the lowest offer is already reported");
        assert!(reporter.withdraw(2));
        assert_eq!(reporter.due(), Some(ClockRead { nanos: 300, sent_nanos: 10 }));
        assert!(!reporter.withdraw(2), "a withdrawn offer is gone");
    }

    #[test]
    fn offers_on_different_samples_wait_for_one_another() {
        let mut reporter = ProgressReporter::default();
        assert_eq!(reporter.due(), None, "no offer, nothing to report");
        reporter.offer(1, ClockRead { nanos: 100, sent_nanos: 10 });
        reporter.offer(2, ClockRead { nanos: 110, sent_nanos: 11 });
        assert_eq!(reporter.due(), None);
        reporter.offer(2, ClockRead { nanos: 110, sent_nanos: 10 });
        assert_eq!(reporter.due(), Some(ClockRead { nanos: 100, sent_nanos: 10 }));
    }

    #[test]
    fn a_progress_turn_holds_withdraws_or_offers_the_capped_snapshot() {
        let read = ClockRead { nanos: 500, sent_nanos: 20 };
        let no_frontier = || None;
        let reports = || true;
        let silent = || false;
        assert_eq!(progress_turn(None, 1_000, reports, no_frontier), ProgressTurn::Hold);
        assert_eq!(progress_turn(None, 1_000, silent, no_frontier), ProgressTurn::Hold);
        assert_eq!(progress_turn(Some(read), 1_000, silent, no_frontier), ProgressTurn::Withdraw);
        assert_eq!(progress_turn(Some(read), 499, reports, no_frontier), ProgressTurn::Hold);
        assert_eq!(progress_turn(Some(read), -1, reports, no_frontier), ProgressTurn::Hold);
        assert_eq!(
            progress_turn(Some(read), 1_000, reports, no_frontier),
            ProgressTurn::Offer(read)
        );
        assert_eq!(
            progress_turn(Some(read), 1_000, reports, || Some(300)),
            ProgressTurn::Offer(ClockRead { nanos: 299, sent_nanos: 20 })
        );
    }
}
