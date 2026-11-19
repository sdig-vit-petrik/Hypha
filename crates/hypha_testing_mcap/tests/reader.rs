//! The Rust API of the MCAP test reader.

#![expect(clippy::expect_used, reason = "a failed test setup stops the test with its message")]

mod common;

use common::{
    ChannelSpec, POINT_MD5, TempFile, find, interleaved, message, options, ros1_channels,
    write_mcap,
};
use hypha_testing_mcap::{Error, Reader};
use mcap::{Compression, WriteOptions};

/// One message as the tests compare it: topic, log time, publish time, payload.
type Seen = (String, u64, u64, Vec<u8>);

fn drain(reader: &mut Reader) -> Vec<Seen> {
    let channels = reader.channels().to_vec();
    let mut seen = Vec::new();
    while let Some(message) = reader.next_message() {
        let topic = channels.get(message.channel_index).expect("channel").topic.clone();
        seen.push((topic, message.log_time_ns, message.publish_time_ns, message.payload.to_vec()));
    }
    seen
}

fn open(file: &TempFile) -> Reader {
    Reader::open(file.path()).expect("open")
}

fn open_error(bytes: &[u8], name: &str) -> (Error, TempFile) {
    let file = TempFile::with_bytes(name, bytes);
    let error = Reader::open(file.path()).expect_err("open refused");
    (error, file)
}

fn log_times(seen: &[Seen]) -> Vec<(String, u64)> {
    seen.iter().map(|(topic, log_time, _, _)| (topic.clone(), *log_time)).collect()
}

fn names(pairs: &[(&str, u64)]) -> Vec<(String, u64)> {
    pairs.iter().map(|(topic, time)| ((*topic).to_owned(), *time)).collect()
}

#[test]
fn uncompressed_lz4_and_zstd_files_read_back_in_log_time_order() {
    for compression in [None, Some(Compression::Lz4), Some(Compression::Zstd)] {
        let file = TempFile::with_bytes("interleaved", &interleaved(compression));
        let mut reader = open(&file);
        reader.select("/position").expect("select");
        reader.select("/switches").expect("select");
        let seen = drain(&mut reader);
        assert_eq!(
            log_times(&seen),
            names(&[
                ("/position", 10_000_000),
                ("/switches", 20_000_000),
                ("/position", 30_000_000),
                ("/switches", 40_000_000),
                ("/position", 50_000_000),
            ]),
            "{compression:?}"
        );
        let (_, log_time, publish_time, payload) = seen.first().expect("first");
        assert_eq!((*log_time, *publish_time), (10_000_000, 9_999_999));
        assert_eq!(payload.as_slice(), b"payload-0-10000000");
    }
}

#[test]
fn channels_list_topic_type_md5_encodings_and_message_count() {
    let file = TempFile::with_bytes("channels", &interleaved(Some(Compression::Lz4)));
    let reader = open(&file);
    let channels = reader.channels();
    assert_eq!(channels.len(), 2);
    let position = channels.first().expect("position");
    assert_eq!(position.topic, "/position");
    assert_eq!(position.type_name, "geometry_msgs/Point");
    assert_eq!(position.schema_encoding, "ros1msg");
    assert_eq!(position.message_encoding, "ros1");
    assert_eq!(position.md5, POINT_MD5);
    assert_eq!(position.message_count, 3);
    let switches = channels.get(1).expect("switches");
    assert_eq!(switches.topic, "/switches");
    assert_eq!(switches.md5, "", "no MD5 in the channel metadata");
    assert_eq!(switches.message_count, 2);
}

#[test]
fn equal_log_times_keep_file_order() {
    let messages = [message(1, 5), message(0, 5), message(1, 5), message(0, 3)];
    let bytes = write_mcap(options(None, None), &ros1_channels(), &messages);
    let file = TempFile::with_bytes("ties", &bytes);
    let mut reader = open(&file);
    reader.select("/position").expect("select");
    reader.select("/switches").expect("select");
    let seen = drain(&mut reader);
    assert_eq!(
        log_times(&seen),
        names(&[("/position", 3), ("/switches", 5), ("/position", 5), ("/switches", 5)])
    );
}

#[test]
fn log_time_order_holds_whatever_the_chunk_order() {
    // One message per chunk, written out of log-time order.
    let messages = [message(0, 30), message(1, 10), message(0, 20)];
    let bytes = write_mcap(options(Some(Compression::Zstd), Some(1)), &ros1_channels(), &messages);
    let file = TempFile::with_bytes("chunk-order", &bytes);
    let mut reader = open(&file);
    reader.select("/position").expect("select");
    reader.select("/switches").expect("select");
    let seen = drain(&mut reader);
    assert_eq!(log_times(&seen), names(&[("/switches", 10), ("/position", 20), ("/position", 30)]));
}

#[test]
fn only_selected_topics_are_returned() {
    let file = TempFile::with_bytes("select", &interleaved(None));
    let mut reader = open(&file);
    reader.select("/switches").expect("select");
    let seen = drain(&mut reader);
    assert_eq!(log_times(&seen), names(&[("/switches", 20_000_000), ("/switches", 40_000_000)]));
}

#[test]
fn selecting_no_topic_yields_end_at_once() {
    let file = TempFile::with_bytes("none", &interleaved(None));
    let mut reader = open(&file);
    assert!(reader.next_message().is_none());
}

#[test]
fn next_after_end_stays_end() {
    let file = TempFile::with_bytes("end", &interleaved(None));
    let mut reader = open(&file);
    reader.select("/position").expect("select");
    assert_eq!(drain(&mut reader).len(), 3);
    assert!(reader.next_message().is_none());
    assert!(reader.next_message().is_none());
}

#[test]
fn rewind_restarts_from_the_first_message() {
    let file = TempFile::with_bytes("rewind", &interleaved(Some(Compression::Lz4)));
    let mut reader = open(&file);
    reader.select("/position").expect("select");
    let first = drain(&mut reader);
    reader.rewind();
    assert_eq!(drain(&mut reader), first);
}

#[test]
fn selecting_an_unknown_topic_is_an_error_naming_it_and_the_path() {
    let file = TempFile::with_bytes("unknown-topic", &interleaved(None));
    let mut reader = open(&file);
    let error = reader.select("/missing").expect_err("refused");
    assert!(matches!(error, Error::UnknownTopic { .. }), "{error:?}");
    let text = error.to_string();
    assert!(text.contains("/missing"), "{text}");
    assert!(text.contains(&file.path().display().to_string()), "{text}");
}

#[test]
fn a_channel_that_is_not_ros1msg_is_listed_with_its_encoding() {
    let channels = [
        ChannelSpec { topic: "/json", type_name: "Pose", schema_encoding: "jsonschema", md5: None },
        ChannelSpec { topic: "/raw", type_name: "", schema_encoding: "", md5: None },
    ];
    let bytes = write_mcap(options(None, None), &channels, &[message(0, 1), message(1, 2)]);
    let file = TempFile::with_bytes("encodings", &bytes);
    let mut reader = open(&file);
    let json = reader.channels().first().expect("json");
    assert_eq!((json.type_name.as_str(), json.schema_encoding.as_str()), ("Pose", "jsonschema"));
    let raw = reader.channels().get(1).expect("raw");
    assert_eq!((raw.type_name.as_str(), raw.schema_encoding.as_str()), ("", ""));
    reader.select("/raw").expect("a channel without a schema can still be selected");
    assert_eq!(log_times(&drain(&mut reader)), names(&[("/raw", 2)]));
}

#[test]
fn a_missing_file_is_refused_with_its_path() {
    let file = TempFile::new("never-written");
    let error = Reader::open(file.path()).expect_err("refused");
    assert!(matches!(error, Error::Io { .. }), "{error:?}");
    assert!(error.to_string().contains(&file.path().display().to_string()), "{error}");
}

#[test]
fn a_bad_magic_is_refused_naming_the_fault_and_the_path() {
    let mut bytes = interleaved(None);
    *bytes.first_mut().expect("magic") = b'X';
    let (error, file) = open_error(&bytes, "bad-magic");
    assert!(matches!(error, Error::Format { source: mcap::McapError::BadMagic, .. }), "{error:?}");
    let text = error.to_string();
    assert!(text.contains("magic"), "{text}");
    assert!(text.contains(&file.path().display().to_string()), "{text}");
}

#[test]
fn a_truncated_file_is_refused() {
    let bytes = interleaved(Some(Compression::Lz4));
    let half = bytes.get(..bytes.len() / 2).expect("half");
    let (error, file) = open_error(half, "truncated");
    assert!(
        matches!(error, Error::Format { source: mcap::McapError::UnexpectedEof, .. }),
        "{error:?}"
    );
    let text = error.to_string();
    assert!(text.contains("ended in the middle of a record"), "{text}");
    assert!(text.contains(&file.path().display().to_string()), "{text}");
}

#[test]
fn a_chunk_crc_failure_is_refused() {
    let mut bytes = interleaved(None);
    let at = find(&bytes, b"payload-0-30000000");
    *bytes.get_mut(at).expect("payload byte") = b'P';
    let (error, file) = open_error(&bytes, "crc");
    assert!(
        matches!(error, Error::Format { source: mcap::McapError::BadChunkCrc { .. }, .. }),
        "{error:?}"
    );
    let text = error.to_string();
    assert!(text.contains("CRC"), "{text}");
    assert!(text.contains(&file.path().display().to_string()), "{text}");
}

#[test]
fn an_unsupported_compression_is_refused_by_name() {
    let mut bytes = interleaved(Some(Compression::Lz4));
    let at = find(&bytes, b"lz4");
    bytes.get_mut(at..at + 3).expect("compression name").copy_from_slice(b"xz9");
    let (error, file) = open_error(&bytes, "compression");
    assert!(
        matches!(error, Error::Format { source: mcap::McapError::UnsupportedCompression(_), .. }),
        "{error:?}"
    );
    let text = error.to_string();
    assert!(text.contains("xz9"), "{text}");
    assert!(text.contains(&file.path().display().to_string()), "{text}");
}

#[test]
fn a_channel_whose_schema_the_file_does_not_define_is_refused() {
    let channels = [ChannelSpec {
        topic: "/orphan",
        type_name: "std_msgs/Bool",
        schema_encoding: "ros1msg",
        md5: None,
    }];
    let writer_options = WriteOptions::new()
        .use_chunks(false)
        .emit_summary_records(false)
        .calculate_data_section_crc(false);
    let mut bytes = write_mcap(writer_options, &channels, &[message(0, 1)]);
    // The channel record holds `id: u16, schema_id: u16, topic: u32 length + bytes`.
    let topic = find(&bytes, b"/orphan");
    bytes.get_mut(topic - 6..topic - 4).expect("schema id").copy_from_slice(&99_u16.to_le_bytes());
    let (error, file) = open_error(&bytes, "orphan");
    assert!(matches!(error, Error::MissingSchema { schema_id: 99, .. }), "{error:?}");
    let text = error.to_string();
    assert!(text.contains("/orphan"), "{text}");
    assert!(text.contains("99"), "{text}");
    assert!(text.contains(&file.path().display().to_string()), "{text}");
}

#[test]
fn the_md5_metadata_key_is_the_one_the_recorder_writes() {
    // apps/hypha_tools/src/recorder.rs `MD5_METADATA_KEY` must stay equal to this.
    assert_eq!(hypha_testing_mcap::MD5_METADATA_KEY, "md5sum");
}
