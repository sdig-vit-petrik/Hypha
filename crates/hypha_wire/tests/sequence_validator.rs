//! Sequence validator coverage.

use core::num::NonZeroUsize;

use hypha_wire::integrity::{MAX_TRACKED_PUBLISHERS, SequenceValidator, next_sequence};

#[test]
fn accepts_increasing_sequences_per_publisher() {
    let mut validator = SequenceValidator::new();
    assert!(validator.accept(1, 0));
    assert!(validator.accept(1, 1));
    assert!(!validator.accept(1, 1));
    assert!(!validator.accept(1, 0));
}

#[test]
fn tracks_publishers_independently() {
    let mut validator = SequenceValidator::new();
    assert!(validator.accept(1, 4));
    assert!(validator.accept(2, 4));
    assert!(!validator.accept(1, 3));
    assert!(validator.accept(2, 5));
}

#[test]
fn a_duplicate_or_an_older_sequence_after_the_wrap_is_refused() {
    let mut validator = SequenceValidator::new();
    assert!(validator.accept(1, u64::MAX - 1));
    assert!(validator.accept(1, u64::MAX));
    assert!(validator.accept(1, 0));
    assert!(!validator.accept(1, 0), "duplicate");
    assert!(!validator.accept(1, u64::MAX), "older, from before the wrap");
    assert!(!validator.accept(1, u64::MAX - 1), "older, from before the wrap");
}

#[test]
fn lost_messages_across_the_wrap_do_not_stop_the_stream() {
    let mut validator = SequenceValidator::new();
    assert!(validator.accept(1, u64::MAX - 5));
    assert!(validator.accept(1, 3), "a gap that spans the wrap is still forward");
    assert!(validator.accept(1, 4));
}

#[test]
fn counts_missing_forward_sequences_but_not_refused_frames() {
    let mut validator = SequenceValidator::new();
    assert!(validator.accept(1, 10));
    assert_eq!(validator.missing_sequences(), 0);
    assert!(validator.accept(1, 13));
    assert_eq!(validator.missing_sequences(), 2);
    assert!(!validator.accept(1, 13), "a duplicate is refused, not counted as loss");
    assert_eq!(validator.missing_sequences(), 2);
}

#[test]
fn counts_missing_forward_sequences_across_wrap() {
    let mut validator = SequenceValidator::new();
    assert!(validator.accept(1, u64::MAX - 1));
    assert!(validator.accept(1, 1));
    assert_eq!(validator.missing_sequences(), 2);
}

#[test]
fn a_sequence_half_the_range_or_more_ahead_is_not_newer() {
    let half_range: u64 = 1 << 63;
    let mut validator = SequenceValidator::new();
    assert!(validator.accept(1, 10));
    assert!(!validator.accept(1, 10 + half_range), "exactly half the range is ambiguous");
    assert!(validator.accept(1, 10 + half_range - 1), "just under half the range is newer");
}

#[test]
fn a_sender_wraps_to_zero_after_the_maximum_value() {
    assert_eq!(next_sequence(u64::MAX - 1), u64::MAX);
    assert_eq!(next_sequence(u64::MAX), 0);
    assert_eq!(next_sequence(0), 1);
}

#[test]
fn a_receiver_accepts_every_sequence_a_sender_sends_across_the_wrap() {
    let mut validator = SequenceValidator::new();
    let mut sequence = u64::MAX - 3;
    for _ in 0..8 {
        sequence = next_sequence(sequence);
        assert!(validator.accept(1, sequence), "sequence {sequence} was refused");
    }
    assert_eq!(sequence, 4);
}

fn capacity(publishers: usize) -> NonZeroUsize {
    NonZeroUsize::new(publishers).unwrap_or(NonZeroUsize::MIN)
}

#[test]
fn a_full_validator_forgets_the_publisher_that_sent_least_recently() {
    let mut validator = SequenceValidator::with_capacity(capacity(2));
    assert!(validator.accept(1, 5));
    assert!(validator.accept(2, 5));
    assert!(validator.accept(1, 6), "publisher 2 is now the least recent");

    assert!(validator.accept(3, 1));

    assert!(!validator.accept(1, 6), "publisher 1 is still tracked, so this is a duplicate");
    assert!(validator.accept(2, 1), "publisher 2 was forgotten, so this is its first frame");
}

#[test]
fn a_refused_frame_does_not_keep_a_publisher_tracked() {
    let mut validator = SequenceValidator::with_capacity(capacity(2));
    assert!(validator.accept(1, 5));
    assert!(validator.accept(2, 5));
    assert!(!validator.accept(1, 5), "a duplicate is not a sign of life");

    assert!(validator.accept(3, 1));

    assert!(validator.accept(1, 1), "publisher 1 sent least recently and was forgotten");
}

#[test]
#[cfg_attr(miri, ignore = "a scale test: years of restarts take Miri more than ten minutes")]
fn publishers_that_restart_for_years_keep_the_table_bounded() {
    let mut validator = SequenceValidator::new();
    let restarts = MAX_TRACKED_PUBLISHERS.saturating_add(1_000);
    for publisher_id in 0..restarts {
        assert!(validator.accept(u64::try_from(publisher_id).unwrap_or(u64::MAX), 1));
    }

    // The newest publishers are still tracked: a repeat of their first frame is a duplicate.
    let newest = u64::try_from(restarts.saturating_sub(1)).unwrap_or(u64::MAX);
    assert!(!validator.accept(newest, 1));
    let oldest_kept =
        u64::try_from(restarts.saturating_sub(MAX_TRACKED_PUBLISHERS)).unwrap_or(u64::MAX);
    assert!(!validator.accept(oldest_kept, 1));
    // The oldest were forgotten, so the table stayed bounded.
    assert!(validator.accept(0, 1));
}
