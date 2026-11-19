//! Property tests of the frame, the schema record, the MD5 schema hash and the clock payload.

use hypha_wire::frame::{FrameHeader, FrameMetadata, seal, verify};
use hypha_wire::ros_time::RosTime;
use hypha_wire::schema_record::{
    MAX_SCHEMA_RECORD_LEN, parse_schema_record, schema_record_frame_len, write_schema_record,
};
use hypha_wire::{is_valid_md5, schema_hash_from_md5};
use proptest::collection::vec;
use proptest::prelude::*;
use proptest::test_runner::FileFailurePersistence;

/// Shrunk failures are kept here, relative to the crate directory, and replayed first.
const REGRESSIONS: &str = "proptest-regressions/frame_properties.txt";

/// Miri interprets every step: a handful of cases still walks each code path.
const MIRI_CASES: u32 = 4;

fn config(cases: u32) -> ProptestConfig {
    ProptestConfig {
        cases: if cfg!(miri) { MIRI_CASES } else { cases },
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(REGRESSIONS))),
        ..ProptestConfig::default()
    }
}

/// A schema record around an arbitrary payload, with the magic and the CRC-32C right,
/// so the parse reaches the fields inside.
fn record_around(header_tail: &[u8; 40], payload: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&0x3558_4F49_u32.to_le_bytes());
    bytes.extend_from_slice(header_tail);
    bytes.extend_from_slice(payload);
    let checksum = crc32c::crc32c(&bytes);
    bytes.extend_from_slice(&checksum.to_le_bytes());
    bytes
}

fn md5_text() -> impl Strategy<Value = String> {
    "[0-9a-f]{32}"
}

fn metadata() -> impl Strategy<Value = FrameMetadata> {
    (any::<u64>(), any::<u64>(), any::<u64>(), any::<u64>(), any::<u64>()).prop_map(
        |(schema_hash, publisher_id, sequence, transport_sent_nanos, framework_sent_nanos)| {
            FrameMetadata {
                schema_hash,
                publisher_id,
                sequence,
                transport_sent_nanos,
                framework_sent_nanos,
            }
        },
    )
}

/// `header` with byte `index` of its covered fields (magic, flags, metadata, crc, in
/// that order, little-endian) changed by `delta`.
fn corrupt_header(header: &FrameHeader, index: usize, delta: u8) -> FrameHeader {
    let m = header.metadata;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&header.magic.to_le_bytes());
    bytes.extend_from_slice(&header.flags.to_le_bytes());
    for field in
        [m.schema_hash, m.publisher_id, m.sequence, m.transport_sent_nanos, m.framework_sent_nanos]
    {
        bytes.extend_from_slice(&field.to_le_bytes());
    }
    bytes.extend_from_slice(&header.crc.to_le_bytes());
    if let Some(byte) = bytes.get_mut(index) {
        *byte = byte.wrapping_add(delta);
    }
    let (words, _) = bytes.as_chunks::<4>();
    let (longs, _) = bytes.get(8..48).unwrap_or_default().as_chunks::<8>();
    let word = |at: usize| words.get(at).copied().map_or(0, u32::from_le_bytes);
    let long = |at: usize| longs.get(at).copied().map_or(0, u64::from_le_bytes);
    FrameHeader {
        magic: word(0),
        flags: word(1),
        metadata: FrameMetadata {
            schema_hash: long(0),
            publisher_id: long(1),
            sequence: long(2),
            transport_sent_nanos: long(3),
            framework_sent_nanos: long(4),
        },
        crc: word(12),
    }
}

proptest! {
    #![proptest_config(config(512))]

    #[test]
    fn a_sealed_frame_verifies_with_its_own_fields(
        metadata in metadata(),
        flags in any::<u32>(),
        payload in vec(any::<u8>(), 0..64),
    ) {
        let header = seal(metadata, flags, &payload);
        let parsed = verify(&header, &payload)
            .map_err(|error| TestCaseError::fail(format!("sealed frame refused: {error}")))?;
        prop_assert_eq!(parsed.schema_hash(), metadata.schema_hash);
        prop_assert_eq!(parsed.publisher_id(), metadata.publisher_id);
        prop_assert_eq!(parsed.sequence(), metadata.sequence);
        prop_assert_eq!(parsed.transport_sent_nanos(), metadata.transport_sent_nanos);
        prop_assert_eq!(parsed.framework_sent_nanos(), metadata.framework_sent_nanos);
        prop_assert_eq!(parsed.latched(), flags & 1 == 1);
        prop_assert_eq!(parsed.payload(), payload.as_slice());
    }

    #[test]
    fn a_frame_with_one_changed_byte_is_refused(
        metadata in metadata(),
        flags in any::<u32>(),
        payload in vec(any::<u8>(), 1..64),
        in_header in any::<bool>(),
        at in 0..64_usize,
        delta in 1..=255_u8,
    ) {
        let header = seal(metadata, flags, &payload);
        if in_header {
            // 52 bytes: magic, flags, five metadata fields and the crc.
            let corrupted = corrupt_header(&header, at % 52, delta);
            prop_assert!(verify(&corrupted, &payload).is_err());
        } else {
            let mut changed = payload.clone();
            if let Some(byte) = changed.get_mut(at.checked_rem(payload.len()).unwrap_or(0)) {
                *byte = byte.wrapping_add(delta);
            }
            prop_assert!(verify(&header, &changed).is_err());
        }
    }

    #[test]
    fn a_schema_record_round_trips(
        data_type in "[a-z_]{1,12}/[A-Za-z0-9_]{1,24}|\\PC{1,40}",
        md5 in md5_text(),
        definition in "\\PC{1,200}",
        spare in 0..16_usize,
    ) {
        let len = schema_record_frame_len(data_type.len(), md5.len(), definition.len())
            .ok_or_else(|| TestCaseError::fail("record length overflows"))?;
        let mut buffer = vec![0_u8; len.saturating_add(spare)];
        let written = write_schema_record(&mut buffer, &data_type, &md5, &definition)
            .map_err(|error| TestCaseError::fail(format!("valid record refused: {error}")))?;
        prop_assert_eq!(written, len);
        let record = buffer.get(..written).unwrap_or_default();
        let parsed = parse_schema_record(record)
            .map_err(|error| TestCaseError::fail(format!("written record refused: {error}")))?;
        prop_assert_eq!(parsed.data_type(), data_type.as_str());
        prop_assert_eq!(parsed.md5(), md5.as_str());
        prop_assert_eq!(parsed.definition(), definition.as_str());
        prop_assert_eq!(schema_hash_from_md5(&md5).is_some(), true);
    }

    #[test]
    fn a_too_small_buffer_or_a_bad_field_is_refused_when_writing(
        data_type in "\\PC{0,8}",
        md5 in "[0-9a-fA-F]{30,34}",
        definition in "\\PC{0,16}",
        short_by in 1..8_usize,
    ) {
        let len = schema_record_frame_len(data_type.len(), md5.len(), definition.len())
            .ok_or_else(|| TestCaseError::fail("record length overflows"))?;
        let mut exact = vec![0_u8; len];
        let fields_valid = !data_type.is_empty() && is_valid_md5(&md5) && !definition.is_empty();
        prop_assert_eq!(write_schema_record(&mut exact, &data_type, &md5, &definition).is_ok(), fields_valid);
        let mut short = vec![0_u8; len.saturating_sub(short_by)];
        prop_assert!(write_schema_record(&mut short, &data_type, &md5, &definition).is_err());
    }

    #[test]
    fn a_schema_record_with_one_changed_or_missing_byte_is_refused(
        md5 in md5_text(),
        definition in "[ -~\n]{1,120}",
        at_permille in 0..1000_usize,
        delta in 1..=255_u8,
        truncate in any::<bool>(),
    ) {
        let data_type = "pkg/Type";
        let len = schema_record_frame_len(data_type.len(), md5.len(), definition.len())
            .ok_or_else(|| TestCaseError::fail("record length overflows"))?;
        let mut record = vec![0_u8; len];
        write_schema_record(&mut record, data_type, &md5, &definition)
            .map_err(|error| TestCaseError::fail(format!("valid record refused: {error}")))?;
        let at = len.saturating_mul(at_permille) / 1000;
        if truncate {
            record.truncate(at);
        } else if let Some(byte) = record.get_mut(at) {
            *byte = byte.wrapping_add(delta);
        }
        prop_assert!(parse_schema_record(&record).is_err());
    }

    #[test]
    fn any_record_payload_parses_to_valid_fields_or_an_error(
        header_tail in any::<[u8; 40]>(),
        payload in prop_oneof![
            vec(any::<u8>(), 0..96),
            (any::<u16>(), "\\PC{0,8}", "[0-9a-fA-F]{32}|\\PC{0,40}", "\\PC{0,40}").prop_map(
                |(dt_len, dt, md5, def)| {
                    let mut bytes = (dt_len % 16).to_le_bytes().to_vec();
                    bytes.extend_from_slice(dt.as_bytes());
                    bytes.extend_from_slice(md5.as_bytes());
                    bytes.extend_from_slice(def.as_bytes());
                    bytes
                }
            ),
        ],
    ) {
        let record = record_around(&header_tail, &payload);
        if let Ok(parsed) = parse_schema_record(&record) {
            prop_assert!(!parsed.data_type().is_empty());
            prop_assert!(is_valid_md5(parsed.md5()));
            prop_assert!(!parsed.definition().is_empty());
            let fields = 2_usize
                .saturating_add(parsed.data_type().len())
                .saturating_add(parsed.md5().len())
                .saturating_add(parsed.definition().len());
            prop_assert_eq!(fields, payload.len());
        }
        // The raw bytes, with no valid magic or checksum, are refused or parse alike.
        let _ = parse_schema_record(&payload);
    }

    #[test]
    fn an_md5_has_a_schema_hash_exactly_when_it_is_valid(
        md5 in prop_oneof![md5_text(), "[0-9a-fA-F]{32}", "\\PC{0,40}"],
    ) {
        let hash = schema_hash_from_md5(&md5);
        prop_assert_eq!(hash.is_some(), is_valid_md5(&md5));
        if let Some(hash) = hash {
            let prefix = md5.get(..16).unwrap_or_default();
            let mut expected = [0_u8; 8];
            for (slot, at) in expected.iter_mut().zip((0..16_usize).step_by(2)) {
                let digits = prefix.get(at..at.saturating_add(2)).unwrap_or_default();
                *slot = u8::from_str_radix(digits, 16).unwrap_or_default();
            }
            prop_assert_eq!(hash, u64::from_le_bytes(expected));
        }
    }

    #[test]
    fn a_clock_payload_is_exactly_one_time_below_one_second_of_nanoseconds(
        bytes in vec(any::<u8>(), 0..16),
    ) {
        let clock = RosTime::decode_clock(&bytes);
        let nsecs = bytes.get(4..8).map(|word| {
            let mut array = [0_u8; 4];
            array.copy_from_slice(word);
            u32::from_le_bytes(array)
        });
        let is_clock = bytes.len() == 8 && nsecs.is_some_and(|nsecs| nsecs < 1_000_000_000);
        prop_assert_eq!(clock.is_some(), is_clock);
    }
}

#[test]
fn a_schema_record_past_the_size_limit_is_refused_when_writing() {
    let definition = "x".repeat(MAX_SCHEMA_RECORD_LEN);
    let mut buffer = vec![0_u8; MAX_SCHEMA_RECORD_LEN.saturating_mul(2)];
    let result = write_schema_record(&mut buffer, "pkg/Type", &"0".repeat(32), &definition);
    assert!(result.is_err(), "a record past the limit is written: {result:?}");
}
