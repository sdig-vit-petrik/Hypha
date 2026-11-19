//! The subscriber's frame validation on a byte-coded stream of frames.
//! Input: one frame per three bytes and a payload: a control byte, a sequence code and a
//! payload length; then that many payload bytes, at least one.
#![no_main]

use hypha_core::frame_validation::{FrameRejection, FrameValidator};
use hypha_wire::frame::{FrameMetadata, seal};
use libfuzzer_sys::fuzz_target;

const EXPECTED_HASH: u64 = 7;
const NOW: u64 = 1_000;
const PUBLISHERS: usize = 4;

fn sequence(code: u8) -> u64 {
    match code % 8 {
        6 => u64::MAX,
        7 => 1 << 63,
        small => u64::from(small),
    }
}

fn sent(control: u8, code: u8) -> u64 {
    match (control >> 5) & 3 {
        0 | 1 => u64::from(code).min(NOW) * 4,
        2 => NOW + 1,
        _ => u64::MAX,
    }
}

fuzz_target!(|input: &[u8]| {
    let validator = FrameValidator::new(Some(EXPECTED_HASH));
    let mut last_accepted = [None::<u64>; PUBLISHERS];
    let mut refused = 0_u64;
    let mut rest = input;
    while let [control, sequence_code, len, tail @ ..] = rest {
        let len = usize::from(len % 8) + 1;
        let Some((payload, tail)) = tail.split_at_checked(len) else {
            break;
        };
        rest = tail;

        let publisher = usize::from(control & 3);
        let other_schema = control & 4 != 0;
        let bad_magic = control & 8 != 0;
        let flip_payload = control & 16 != 0;
        let sequence = sequence(*sequence_code);
        let sent = sent(*control, *sequence_code);
        let metadata = FrameMetadata {
            schema_hash: if other_schema { EXPECTED_HASH + 1 } else { EXPECTED_HASH },
            publisher_id: publisher as u64,
            sequence,
            transport_sent_nanos: sent,
            framework_sent_nanos: sent,
        };
        let mut header = seal(topic(), metadata, 0, payload);
        if bad_magic {
            header.magic = header.magic.wrapping_add(1);
        }
        let mut payload = payload.to_vec();
        if flip_payload {
            payload[0] ^= 1;
        }

        let newer = last_accepted[publisher].is_none_or(|last| {
            let distance = sequence.wrapping_sub(last);
            distance != 0 && distance < 1 << 63
        });
        let expected = if bad_magic {
            Err(FrameRejection::InvalidFrame)
        } else if flip_payload {
            Err(FrameRejection::CorruptPayload)
        } else if other_schema {
            Err(FrameRejection::SchemaMismatch)
        } else if sent > NOW {
            Err(FrameRejection::Future)
        } else if !newer {
            Err(FrameRejection::Sequence)
        } else {
            Ok(sent)
        };
        let result = validator.validate(topic(), &header, &payload, NOW);
        assert_eq!(result, expected, "{header:?}");
        if result.is_ok() {
            last_accepted[publisher] = Some(sequence);
        } else {
            refused += 1;
        }
    }
    let stats = validator.stats();
    let counted = stats.invalid_frame
        + stats.corrupt_payload
        + stats.schema_mismatch
        + stats.sequence_gap
        + stats.invalid_timestamp;
    assert_eq!(counted, refused, "{stats:?}");
});

/// The topic every frame of this file is sealed for.
fn topic() -> hypha_wire::frame::TopicCode {
    hypha_wire::frame::TopicCode::of("/test")
}
