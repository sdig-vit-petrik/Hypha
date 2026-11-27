//! Frame verification and the schema record on arbitrary bytes.
//! Input: the first 56 bytes are a frame header, the rest the transport's slice. The whole
//! input is also parsed as a schema record, raw and wrapped in a valid magic and
//! checksum.
#![no_main]

use hypha_wire::frame::{FRAME_MAGIC, FrameHeader, FrameMetadata, seal, verify};
use hypha_wire::is_valid_md5;
use hypha_wire::ros_time::RosTime;
use hypha_wire::schema_record::{
    ParsedSchemaRecord, parse_schema_record, schema_record_frame_len, write_schema_record,
};
use libfuzzer_sys::fuzz_target;

const HEADER_LEN: usize = 56;
const RECORD_MAGIC: u32 = 0x3558_4F49;
const RECORD_HEADER_LEN: usize = 44;

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    let mut word = [0_u8; 4];
    word.copy_from_slice(&bytes[at..at + 4]);
    u32::from_le_bytes(word)
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    let mut word = [0_u8; 8];
    word.copy_from_slice(&bytes[at..at + 8]);
    u64::from_le_bytes(word)
}

fn header_from(bytes: &[u8]) -> FrameHeader {
    FrameHeader {
        magic: u32_at(bytes, 0),
        flags: u32_at(bytes, 4),
        metadata: FrameMetadata {
            schema_hash: u64_at(bytes, 8),
            publisher_id: u64_at(bytes, 16),
            sequence: u64_at(bytes, 24),
            transport_sent_nanos: u64_at(bytes, 32),
            framework_sent_nanos: u64_at(bytes, 40),
        },
        crc: u32_at(bytes, 48),
        payload_len: u32_at(bytes, 52),
    }
}

fn check_frame(header: &FrameHeader, slice: &[u8]) {
    // The message is the first payload_len bytes of the slice, when it has them.
    let message = usize::try_from(header.payload_len).ok().and_then(|len| slice.get(..len));
    let expected_ok = message.is_some_and(|payload| {
        let sealed = seal(topic(), header.metadata, header.flags, payload);
        header.magic == FRAME_MAGIC && header.crc == sealed.crc
    });
    match verify(topic(), header, slice) {
        Ok(frame) => {
            assert!(expected_ok, "verify accepted {header:?}");
            assert_eq!(frame.schema_hash(), header.metadata.schema_hash);
            assert_eq!(frame.publisher_id(), header.metadata.publisher_id);
            assert_eq!(frame.sequence(), header.metadata.sequence);
            assert_eq!(frame.transport_sent_nanos(), header.metadata.transport_sent_nanos);
            assert_eq!(frame.framework_sent_nanos(), header.metadata.framework_sent_nanos);
            assert_eq!(Some(frame.payload()), message);
        }
        Err(_) => assert!(!expected_ok, "verify refused a sealed frame"),
    }
    let sealed = seal(topic(), header.metadata, header.flags, slice);
    assert!(verify(topic(), &sealed, slice).is_ok(), "a sealed header does not verify");
}

/// What a parsed record must satisfy against the payload length it came from.
fn check_record(parsed: &ParsedSchemaRecord<'_>, payload_len: Option<usize>) {
    assert!(!parsed.data_type().is_empty());
    assert!(is_valid_md5(parsed.md5()));
    assert!(!parsed.definition().is_empty());
    let fields = 2 + parsed.data_type().len() + parsed.md5().len() + parsed.definition().len();
    if let Some(payload_len) = payload_len {
        assert_eq!(fields, payload_len, "the fields do not cover the payload");
    }
    let len = schema_record_frame_len(
        parsed.data_type().len(),
        parsed.md5().len(),
        parsed.definition().len(),
    )
    .expect("a parsed record has a length");
    let mut buffer = vec![0_u8; len];
    let written =
        write_schema_record(&mut buffer, parsed.data_type(), parsed.md5(), parsed.definition())
            .expect("a parsed record writes back");
    assert_eq!(written, len);
    let again = parse_schema_record(&buffer).expect("a written record parses");
    assert_eq!(&again, parsed, "a written record parses to other fields");
}

fuzz_target!(|input: &[u8]| {
    if input.len() >= HEADER_LEN {
        let (header, payload) = input.split_at(HEADER_LEN);
        check_frame(&header_from(header), payload);
    }

    if let Ok(parsed) = parse_schema_record(input) {
        check_record(&parsed, None);
    }

    // The input as the payload of a record whose magic and checksum are right.
    let mut record = Vec::with_capacity(input.len() + RECORD_HEADER_LEN + 4);
    record.extend_from_slice(&RECORD_MAGIC.to_le_bytes());
    record.resize(RECORD_HEADER_LEN, 0);
    record.extend_from_slice(input);
    let checksum = crc32c::crc32c(&record);
    record.extend_from_slice(&checksum.to_le_bytes());
    if let Ok(parsed) = parse_schema_record(&record) {
        check_record(&parsed, Some(input.len()));
    }

    let clock = RosTime::decode_clock(input);
    let is_clock = input.len() == 8 && u32_at(input, 4) < 1_000_000_000;
    assert_eq!(clock.is_some(), is_clock, "decode_clock of {input:?}");
});

/// The topic every frame of this file is sealed for.
fn topic() -> hypha_wire::frame::TopicCode {
    hypha_wire::frame::TopicCode::of("/test")
}
