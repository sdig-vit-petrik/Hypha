//! Property tests of frame validation: each frame is accepted or refused for exactly
//! one counted reason, the first rule that applies.

use hypha_core::frame_validation::{FrameRejection, FrameValidator};
use hypha_wire::frame::{FrameMetadata, seal};
use proptest::collection::vec;
use proptest::prelude::*;
use proptest::test_runner::FileFailurePersistence;

/// Shrunk failures are kept here, relative to the crate directory, and replayed first.
const REGRESSIONS: &str = "proptest-regressions/frame_validation_properties.txt";

const EXPECTED_HASH: u64 = 7;
const NOW: u64 = 1_000;

/// One frame of the stream and what is done to it after sealing.
#[derive(Debug, Clone)]
struct Arrival {
    publisher_id: u64,
    sequence: u64,
    sent: u64,
    other_schema: bool,
    bad_magic: bool,
    flip_payload: bool,
    payload: Vec<u8>,
}

fn arrival() -> impl Strategy<Value = Arrival> {
    (
        0..3_u64,
        prop_oneof![0..6_u64, Just(u64::MAX), Just(1 << 63)],
        prop_oneof![4 => 0..=NOW, 1 => Just(NOW.saturating_add(1)), 1 => Just(u64::MAX)],
        prop::bool::weighted(0.1),
        prop::bool::weighted(0.1),
        prop::bool::weighted(0.1),
        vec(any::<u8>(), 1..8),
    )
        .prop_map(
            |(publisher_id, sequence, sent, other_schema, bad_magic, flip_payload, payload)| {
                Arrival {
                    publisher_id,
                    sequence,
                    sent,
                    other_schema,
                    bad_magic,
                    flip_payload,
                    payload,
                }
            },
        )
}

proptest! {
    #![proptest_config(ProptestConfig {
        // Miri interprets every step: a handful of cases.
        cases: if cfg!(miri) { 4 } else { 512 },
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(REGRESSIONS))),
        ..ProptestConfig::default()
    })]

    #[test]
    fn each_frame_is_accepted_or_refused_for_the_first_rule_that_applies(
        arrivals in vec(arrival(), 0..32),
    ) {
        let validator = FrameValidator::new(Some(EXPECTED_HASH));
        let mut last_accepted: [Option<u64>; 3] = [None; 3];
        let mut refused = 0_u64;
        for arrival in &arrivals {
            let schema_hash = if arrival.other_schema { EXPECTED_HASH.wrapping_add(1) } else { EXPECTED_HASH };
            let metadata = FrameMetadata {
                schema_hash,
                publisher_id: arrival.publisher_id,
                sequence: arrival.sequence,
                transport_sent_nanos: arrival.sent,
                framework_sent_nanos: arrival.sent,
            };
            let mut header = seal(metadata, 0, &arrival.payload);
            if arrival.bad_magic {
                header.magic = header.magic.wrapping_add(1);
            }
            let mut payload = arrival.payload.clone();
            if arrival.flip_payload
                && let Some(byte) = payload.first_mut()
            {
                *byte ^= 1;
            }
            let slot = usize::try_from(arrival.publisher_id).ok().and_then(|index| last_accepted.get_mut(index));
            let Some(slot) = slot else {
                return Err(TestCaseError::fail("publisher out of range"));
            };
            let newer = slot.is_none_or(|last| {
                let distance = arrival.sequence.wrapping_sub(last);
                distance != 0 && distance < 1 << 63
            });
            let expected = if arrival.bad_magic {
                Err(FrameRejection::InvalidFrame)
            } else if arrival.flip_payload {
                Err(FrameRejection::CorruptPayload)
            } else if arrival.other_schema {
                Err(FrameRejection::SchemaMismatch)
            } else if arrival.sent > NOW {
                Err(FrameRejection::Future)
            } else if !newer {
                Err(FrameRejection::Sequence)
            } else {
                Ok(arrival.sent)
            };
            let result = validator.validate(&header, &payload, NOW);
            prop_assert_eq!(result, expected, "{:?}", arrival);
            if result.is_ok() {
                *slot = Some(arrival.sequence);
            } else {
                refused = refused.saturating_add(1);
            }
        }
        let stats = validator.stats();
        let counted = [stats.invalid_frame, stats.corrupt_payload, stats.schema_mismatch, stats.sequence_gap, stats.invalid_timestamp]
            .into_iter()
            .fold(0_u64, u64::saturating_add);
        prop_assert_eq!(counted, refused);
    }
}
