//! The C API of the MCAP test reader, called through the `extern "C"` functions.

#![expect(unsafe_code, reason = "these tests call the C functions with raw pointers")]
#![expect(clippy::expect_used, reason = "a failed test setup stops the test with its message")]

mod common;

use std::os::unix::ffi::OsStrExt;
use std::ptr;

use common::{POINT_MD5, TempFile, interleaved};
use hypha_testing_mcap::{
    HyphaMcapBytes, HyphaMcapChannel, HyphaMcapMessage, HyphaMcapReader, HyphaMcapStatus,
    hypha_mcap_channel, hypha_mcap_channel_count, hypha_mcap_close, hypha_mcap_last_error,
    hypha_mcap_next, hypha_mcap_open, hypha_mcap_rewind, hypha_mcap_select,
};
use mcap::Compression;

/// The bytes a `HyphaMcapBytes` borrows, copied.
fn bytes_of(bytes: HyphaMcapBytes) -> Vec<u8> {
    if bytes.len == 0 {
        return Vec::new();
    }
    // SAFETY: a non-empty `HyphaMcapBytes` points to `len` bytes valid while the reader is open.
    unsafe { std::slice::from_raw_parts(bytes.data, bytes.len) }.to_vec()
}

fn text_of(bytes: HyphaMcapBytes) -> String {
    String::from_utf8(bytes_of(bytes)).expect("UTF-8")
}

/// The outcome of `hypha_mcap_open` with an error buffer of `capacity` bytes.
struct Opened {
    status: HyphaMcapStatus,
    reader: *mut HyphaMcapReader,
    error_len: usize,
    error: Vec<u8>,
}

fn open_bytes(path: &[u8], capacity: usize) -> Opened {
    let mut reader: *mut HyphaMcapReader = ptr::dangling_mut();
    let mut error = vec![0_u8; capacity];
    let mut error_len = usize::MAX;
    // SAFETY: `path` is `path.len()` readable bytes, `reader` and `error_len` are
    // writable locals and `error` holds `capacity` writable bytes.
    let status = unsafe {
        hypha_mcap_open(
            path.as_ptr(),
            path.len(),
            &raw mut reader,
            error.as_mut_ptr(),
            capacity,
            &raw mut error_len,
        )
    };
    Opened { status, reader, error_len, error }
}

/// A reader of the file at `file`, which must open.
fn open(file: &TempFile) -> *mut HyphaMcapReader {
    let opened = open_bytes(file.path().as_os_str().as_bytes(), 0);
    assert_eq!(opened.status, HyphaMcapStatus::Ok);
    assert!(!opened.reader.is_null());
    opened.reader
}

fn close(reader: *mut HyphaMcapReader) {
    // SAFETY: `reader` is null or open, and no test uses it afterwards.
    unsafe { hypha_mcap_close(reader) };
}

fn select(reader: *mut HyphaMcapReader, topic: &str) -> HyphaMcapStatus {
    // SAFETY: `reader` is null or open; `topic` is `topic.len()` readable bytes.
    unsafe { hypha_mcap_select(reader, topic.as_ptr(), topic.len()) }
}

fn last_error(reader: *mut HyphaMcapReader) -> String {
    // SAFETY: `reader` is open; the text is copied before the next call on it.
    text_of(unsafe { hypha_mcap_last_error(reader) })
}

fn channel(reader: *mut HyphaMcapReader, index: usize) -> (HyphaMcapStatus, HyphaMcapChannel) {
    let empty = HyphaMcapBytes { data: ptr::null(), len: 0 };
    let mut out = HyphaMcapChannel {
        topic: empty,
        type_name: empty,
        schema_encoding: empty,
        message_encoding: empty,
        md5: empty,
        message_count: u64::MAX,
    };
    // SAFETY: `reader` is null or open; `out` is a writable local.
    let status = unsafe { hypha_mcap_channel(reader, index, &raw mut out) };
    (status, out)
}

/// One message as the tests compare it: channel index, log time, publish time, payload.
type Seen = (usize, u64, u64, Vec<u8>);

fn next(reader: *mut HyphaMcapReader) -> (HyphaMcapStatus, Option<Seen>) {
    let mut out = HyphaMcapMessage {
        channel_index: usize::MAX,
        log_time_ns: 0,
        publish_time_ns: 0,
        payload: HyphaMcapBytes { data: ptr::null(), len: 0 },
    };
    // SAFETY: `reader` is null or open; `out` is a writable local.
    let status = unsafe { hypha_mcap_next(reader, &raw mut out) };
    let seen = (status == HyphaMcapStatus::Ok)
        .then(|| (out.channel_index, out.log_time_ns, out.publish_time_ns, bytes_of(out.payload)));
    (status, seen)
}

fn drain(reader: *mut HyphaMcapReader) -> Vec<Seen> {
    let mut seen = Vec::new();
    while let (HyphaMcapStatus::Ok, Some(message)) = next(reader) {
        seen.push(message);
    }
    seen
}

#[test]
fn c_api_lists_channels_and_returns_selected_messages_in_log_time_order() {
    let file = TempFile::with_bytes("c-interleaved", &interleaved(Some(Compression::Zstd)));
    let reader = open(&file);
    // SAFETY: `reader` is open.
    assert_eq!(unsafe { hypha_mcap_channel_count(reader) }, 2);
    let (status, position) = channel(reader, 0);
    assert_eq!(status, HyphaMcapStatus::Ok);
    assert_eq!(text_of(position.topic), "/position");
    assert_eq!(text_of(position.type_name), "geometry_msgs/Point");
    assert_eq!(text_of(position.schema_encoding), "ros1msg");
    assert_eq!(text_of(position.message_encoding), "ros1");
    assert_eq!(text_of(position.md5), POINT_MD5);
    assert_eq!(position.message_count, 3);
    let (_, switches) = channel(reader, 1);
    assert_eq!(text_of(switches.topic), "/switches");
    assert_eq!(text_of(switches.md5), "");
    assert_eq!(switches.message_count, 2);

    assert_eq!(select(reader, "/position"), HyphaMcapStatus::Ok);
    assert_eq!(select(reader, "/switches"), HyphaMcapStatus::Ok);
    let seen = drain(reader);
    let order: Vec<(usize, u64)> = seen.iter().map(|(index, time, _, _)| (*index, *time)).collect();
    assert_eq!(
        order,
        [(0, 10_000_000), (1, 20_000_000), (0, 30_000_000), (1, 40_000_000), (0, 50_000_000)]
    );
    assert_eq!(
        seen.first().expect("first"),
        &(0, 10_000_000, 9_999_999, b"payload-0-10000000".to_vec())
    );
    close(reader);
}

#[test]
fn c_api_next_after_end_stays_end_and_rewind_restarts() {
    let file = TempFile::with_bytes("c-rewind", &interleaved(None));
    let reader = open(&file);
    assert_eq!(select(reader, "/switches"), HyphaMcapStatus::Ok);
    let first = drain(reader);
    assert_eq!(first.len(), 2);
    assert_eq!(next(reader).0, HyphaMcapStatus::End);
    assert_eq!(next(reader).0, HyphaMcapStatus::End);
    // SAFETY: `reader` is open.
    assert_eq!(unsafe { hypha_mcap_rewind(reader) }, HyphaMcapStatus::Ok);
    assert_eq!(drain(reader), first);
    close(reader);
}

#[test]
fn c_api_selecting_no_topic_yields_end_at_once() {
    let file = TempFile::with_bytes("c-none", &interleaved(None));
    let reader = open(&file);
    assert_eq!(next(reader), (HyphaMcapStatus::End, None));
    close(reader);
}

#[test]
fn c_api_unknown_topic_is_an_error_whose_text_clears_on_the_next_good_call() {
    let file = TempFile::with_bytes("c-unknown", &interleaved(None));
    let reader = open(&file);
    assert_eq!(select(reader, "/missing"), HyphaMcapStatus::Error);
    let text = last_error(reader);
    assert!(text.contains("/missing"), "{text}");
    assert!(text.contains(&file.path().display().to_string()), "{text}");
    assert_eq!(select(reader, "/position"), HyphaMcapStatus::Ok);
    assert_eq!(last_error(reader), "");
    close(reader);
}

#[test]
fn c_api_channel_index_out_of_range_and_null_outputs_are_errors() {
    let file = TempFile::with_bytes("c-range", &interleaved(None));
    let reader = open(&file);
    let (status, untouched) = channel(reader, 2);
    assert_eq!(status, HyphaMcapStatus::Error);
    assert_eq!(untouched.message_count, u64::MAX, "nothing written");
    assert!(last_error(reader).contains("index 2"), "{}", last_error(reader));
    // SAFETY: `reader` is open; a null `out` is refused before any write.
    assert_eq!(unsafe { hypha_mcap_channel(reader, 0, ptr::null_mut()) }, HyphaMcapStatus::Error);
    assert_eq!(select(reader, "/position"), HyphaMcapStatus::Ok);
    // SAFETY: `reader` is open; a null `out` is refused before any write.
    assert_eq!(unsafe { hypha_mcap_next(reader, ptr::null_mut()) }, HyphaMcapStatus::Error);
    assert!(last_error(reader).contains("out is null"), "{}", last_error(reader));
    // The refused `next` did not move the cursor.
    assert_eq!(drain(reader).len(), 3);
    // SAFETY: `reader` is open; a null `topic` is refused before any read.
    assert_eq!(unsafe { hypha_mcap_select(reader, ptr::null(), 4) }, HyphaMcapStatus::Error);
    let not_utf8 = [0xff_u8];
    // SAFETY: `reader` is open; `not_utf8` is one readable byte.
    let status = unsafe { hypha_mcap_select(reader, not_utf8.as_ptr(), not_utf8.len()) };
    assert_eq!(status, HyphaMcapStatus::Error);
    assert!(last_error(reader).contains("not UTF-8"), "{}", last_error(reader));
    close(reader);
}

#[test]
fn c_api_open_refusal_reports_its_text_by_the_size_query_protocol() {
    let mut bytes = interleaved(None);
    *bytes.first_mut().expect("magic") = b'X';
    let file = TempFile::with_bytes("c-bad-magic", &bytes);
    let path = file.path().as_os_str().as_bytes();

    let short = open_bytes(path, 4);
    assert_eq!(short.status, HyphaMcapStatus::Error);
    assert!(short.reader.is_null(), "null written to out_reader");
    assert!(short.error_len > 4, "the needed length is reported");
    assert_eq!(short.error, [0_u8; 4], "nothing is written to a short buffer");

    let full = open_bytes(path, short.error_len);
    assert_eq!(full.status, HyphaMcapStatus::Error);
    assert_eq!(full.error_len, short.error_len);
    let text = String::from_utf8(full.error).expect("UTF-8");
    assert!(text.contains("magic"), "{text}");
    assert!(text.contains(&file.path().display().to_string()), "{text}");
}

#[test]
fn c_api_open_refuses_a_null_path_or_out_reader() {
    let mut reader: *mut HyphaMcapReader = ptr::dangling_mut();
    let mut error_len = 0_usize;
    // SAFETY: a null `path` is refused before any read; the other pointers are writable.
    let status = unsafe {
        hypha_mcap_open(ptr::null(), 3, &raw mut reader, ptr::null_mut(), 0, &raw mut error_len)
    };
    assert_eq!(status, HyphaMcapStatus::Error);
    assert!(reader.is_null());
    assert!(error_len > 0);

    let path = b"/nonexistent.mcap";
    // SAFETY: `path` is `path.len()` readable bytes; a null `out_reader` is refused.
    let status = unsafe {
        hypha_mcap_open(
            path.as_ptr(),
            path.len(),
            ptr::null_mut(),
            ptr::null_mut(),
            0,
            ptr::null_mut(),
        )
    };
    assert_eq!(status, HyphaMcapStatus::Error);

    let missing = open_bytes(path, 512);
    assert_eq!(missing.status, HyphaMcapStatus::Error);
    let text = String::from_utf8(missing.error.get(..missing.error_len).expect("text").to_vec());
    assert!(text.expect("UTF-8").contains("/nonexistent.mcap"));
}

#[test]
fn c_api_null_reader_is_refused_and_close_accepts_null() {
    let null = ptr::null_mut();
    // SAFETY: a null reader is allowed by each function.
    assert_eq!(unsafe { hypha_mcap_channel_count(null) }, 0);
    assert_eq!(channel(null, 0).0, HyphaMcapStatus::Error);
    assert_eq!(select(null, "/position"), HyphaMcapStatus::Error);
    // SAFETY: a null reader is allowed.
    assert_eq!(unsafe { hypha_mcap_rewind(null) }, HyphaMcapStatus::Error);
    assert_eq!(next(null).0, HyphaMcapStatus::Error);
    // SAFETY: a null reader is allowed.
    let error = unsafe { hypha_mcap_last_error(null) };
    assert!(error.data.is_null());
    assert_eq!(error.len, 0);
    close(null);
}
