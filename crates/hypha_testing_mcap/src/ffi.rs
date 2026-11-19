//! The C API of the MCAP test reader: thin `extern "C"` wrappers over [`Reader`].
//!
//! A reader is used by one thread at a time; no function keeps a pointer it was given;
//! a Rust panic aborts the process. Text is UTF-8, a pointer and a length, not NUL-terminated.

use std::path::PathBuf;

use crate::reader::{Channel, Reader};

/// The result of a `hypha_mcap_*` call.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HyphaMcapStatus {
    /// The call did what it says.
    Ok = 0,
    /// `hypha_mcap_next` only: no selected message is left. Nothing was written.
    End = 1,
    /// The call was refused; `hypha_mcap_last_error` names why (no text for a null reader).
    Error = 2,
}

/// Borrowed bytes: `len` bytes at `data`. `data` is null only when `len` is 0.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct HyphaMcapBytes {
    /// The first byte.
    pub data: *const u8,
    /// The number of bytes.
    pub len: usize,
}

impl HyphaMcapBytes {
    fn of(bytes: &[u8]) -> Self {
        Self { data: bytes.as_ptr(), len: bytes.len() }
    }

    const EMPTY: Self = Self { data: std::ptr::null(), len: 0 };
}

/// One channel of an open file. Every text is borrowed from the reader and stays
/// valid until `hypha_mcap_close`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct HyphaMcapChannel {
    /// The topic.
    pub topic: HyphaMcapBytes,
    /// The schema name (`geometry_msgs/Point`); empty for a channel without a schema.
    pub type_name: HyphaMcapBytes,
    /// The schema encoding: `ros1msg` for a ROS 1 message; empty without a schema.
    pub schema_encoding: HyphaMcapBytes,
    /// The message encoding (`ros1` for a ROS 1 message).
    pub message_encoding: HyphaMcapBytes,
    /// The ROS 1 MD5 from the channel metadata key `md5sum`; empty when not recorded.
    pub md5: HyphaMcapBytes,
    /// The number of messages of the channel in the file.
    pub message_count: u64,
}

/// One message of a selected topic.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct HyphaMcapMessage {
    /// The index of its channel, as `hypha_mcap_channel` takes it.
    pub channel_index: usize,
    /// Its log time in nanoseconds.
    pub log_time_ns: u64,
    /// Its publish time in nanoseconds.
    pub publish_time_ns: u64,
    /// Its payload as stored, borrowed from the reader. Valid until the following
    /// `hypha_mcap_next`, `hypha_mcap_rewind` or `hypha_mcap_close` on the same reader.
    pub payload: HyphaMcapBytes,
}

/// An open MCAP file. Opaque: made by `hypha_mcap_open`, freed by `hypha_mcap_close`.
pub struct HyphaMcapReader {
    reader: Reader,
    /// The text of the last refused call, empty when the last call succeeded.
    last_error: String,
}

impl HyphaMcapReader {
    fn fail(&mut self, text: String) -> HyphaMcapStatus {
        self.last_error = text;
        HyphaMcapStatus::Error
    }
}

/// The longest slice Rust can describe.
const MAX_SLICE_LEN: usize = isize::MAX.unsigned_abs();

/// `len` bytes at `bytes`; `None` for null or a length no slice can have.
///
/// # Safety
/// `bytes` must be null, or point to `len` readable bytes that stay unchanged for `'a`.
unsafe fn borrow_bytes<'a>(bytes: *const u8, len: usize) -> Option<&'a [u8]> {
    if bytes.is_null() || len > MAX_SLICE_LEN {
        return None;
    }
    // SAFETY: non-null and at most `isize::MAX` long by the check above; readable by the caller.
    Some(unsafe { std::slice::from_raw_parts(bytes, len) })
}

/// The reader behind `reader`, for writing; `None` for null.
///
/// # Safety
/// `reader` must be null or an open reader that no other call uses for `'a`.
unsafe fn reader_mut<'a>(reader: *mut HyphaMcapReader) -> Option<&'a mut HyphaMcapReader> {
    // SAFETY: `as_mut` checks null; live and exclusive for `'a` by the caller.
    unsafe { reader.as_mut() }
}

/// The reader behind `reader`; `None` for null.
///
/// # Safety
/// `reader` must be null or an open reader that no other call writes for `'a`.
unsafe fn reader_ref<'a>(reader: *const HyphaMcapReader) -> Option<&'a HyphaMcapReader> {
    // SAFETY: `as_ref` checks null; live and unwritten for `'a` by the caller.
    unsafe { reader.as_ref() }
}

/// Write `value` to `out` unless `out` is null; `false` for null.
///
/// # Safety
/// `out` must be null or point to writable storage for one `T`.
unsafe fn write_out<T>(out: *mut T, value: T) -> bool {
    if out.is_null() {
        return false;
    }
    // SAFETY: non-null by the check above; writable storage for one `T` by the caller.
    unsafe { out.write(value) };
    true
}

/// The size-query protocol of `hypha_ffi`: the length goes to `out_len` (when not null),
/// the bytes to `out` only when `capacity` holds them all and `out` is not null.
///
/// # Safety
/// `out_len` must be null or point to a writable `size_t`. `out` must be null or point
/// to `capacity` writable bytes.
unsafe fn write_text(text: &str, out: *mut u8, capacity: usize, out_len: *mut usize) {
    // SAFETY: the clause on `out_len` of this function's contract.
    unsafe { write_out(out_len, text.len()) };
    if out.is_null() || capacity < text.len() {
        return;
    }
    // SAFETY: non-null and `capacity >= text.len()` by the check above.
    unsafe { std::ptr::copy_nonoverlapping(text.as_ptr(), out, text.len()) };
}

/// The path that `bytes` name; `None` for bytes no path can have.
#[cfg_attr(unix, expect(clippy::unnecessary_wraps, reason = "any bytes are a POSIX path"))]
fn path_of(bytes: &[u8]) -> Option<PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Some(PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
    }
    #[cfg(not(unix))]
    {
        std::str::from_utf8(bytes).ok().map(PathBuf::from)
    }
}

/// Open the MCAP file at the `path_len` bytes of `path` (not NUL-terminated), read it
/// whole into memory and check it.
///
/// `HYPHA_MCAP_STATUS_OK`: writes the new reader to `*out_reader`; the caller owns it and
/// frees it with `hypha_mcap_close`. No topic is selected.
///
/// `HYPHA_MCAP_STATUS_ERROR`: writes null to `*out_reader` (when not null) and reports
/// the error text by the size-query protocol: its length to `*error_len` (when not null),
/// its bytes to `error` only when `error_capacity` holds them all. Refused: a null `path`
/// or `out_reader`, an unreadable file, a file above 1 GiB, and any malformed or
/// unsupported content (lz4 and zstd chunks are supported).
///
/// # Safety
/// `path` must be null or point to `path_len` readable bytes. `out_reader` must be null
/// or point to writable storage for one pointer. `error` must be null or point to
/// `error_capacity` writable bytes. `error_len` must be null or point to a writable
/// `size_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_mcap_open(
    path: *const u8,
    path_len: usize,
    out_reader: *mut *mut HyphaMcapReader,
    error: *mut u8,
    error_capacity: usize,
    error_len: *mut usize,
) -> HyphaMcapStatus {
    // SAFETY: the clause on `out_reader` of this function's contract.
    let out_reader_writable = unsafe { write_out(out_reader, std::ptr::null_mut()) };
    // SAFETY: the clause on `path` of this function's contract.
    let path_bytes = unsafe { borrow_bytes(path, path_len) };
    let result = match (out_reader_writable, path_bytes.and_then(path_of)) {
        (false, _) => Err("hypha_mcap_open: out_reader is null".to_owned()),
        (true, None) => Err("hypha_mcap_open: path is null or not a path".to_owned()),
        (true, Some(path)) => Reader::open(path).map_err(|error| error.to_string()),
    };
    match result {
        Ok(reader) => {
            let reader = Box::new(HyphaMcapReader { reader, last_error: String::new() });
            // SAFETY: `out_reader` is non-null (written above) and writable by the caller.
            unsafe { write_out(out_reader, Box::into_raw(reader)) };
            HyphaMcapStatus::Ok
        }
        Err(text) => {
            // SAFETY: the clauses on `error` and `error_len` of this function's contract.
            unsafe { write_text(&text, error, error_capacity, error_len) };
            HyphaMcapStatus::Error
        }
    }
}

/// The number of channels of the file; 0 for a null reader.
///
/// # Safety
/// `reader` must be null or an open reader that no other thread uses during the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_mcap_channel_count(reader: *const HyphaMcapReader) -> usize {
    // SAFETY: the clause on `reader` of this function's contract.
    unsafe { reader_ref(reader) }.map_or(0, |reader| reader.reader.channels().len())
}

/// Describe channel `index` (`0 <= index < hypha_mcap_channel_count`) into `*out`. Its
/// texts are borrowed from the reader and stay valid until `hypha_mcap_close`.
///
/// `HYPHA_MCAP_STATUS_ERROR` for a null reader (no error text), a null `out` or an
/// `index` out of range; nothing is written to `*out` then.
///
/// # Safety
/// `reader` must be null or an open reader that no other thread uses during the call.
/// `out` must be null or point to writable storage for one `HyphaMcapChannel`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_mcap_channel(
    reader: *mut HyphaMcapReader,
    index: usize,
    out: *mut HyphaMcapChannel,
) -> HyphaMcapStatus {
    // SAFETY: the clause on `reader` of this function's contract.
    let Some(reader) = (unsafe { reader_mut(reader) }) else {
        return HyphaMcapStatus::Error;
    };
    reader.last_error.clear();
    let count = reader.reader.channels().len();
    let Some(channel) = reader.reader.channels().get(index) else {
        return reader.fail(format!(
            "hypha_mcap_channel: index {index} is out of range; the file has {count} channels"
        ));
    };
    let description = describe(channel);
    // SAFETY: the clause on `out` of this function's contract.
    if unsafe { write_out(out, description) } {
        HyphaMcapStatus::Ok
    } else {
        reader.fail("hypha_mcap_channel: out is null".to_owned())
    }
}

fn describe(channel: &Channel) -> HyphaMcapChannel {
    HyphaMcapChannel {
        topic: HyphaMcapBytes::of(channel.topic.as_bytes()),
        type_name: HyphaMcapBytes::of(channel.type_name.as_bytes()),
        schema_encoding: HyphaMcapBytes::of(channel.schema_encoding.as_bytes()),
        message_encoding: HyphaMcapBytes::of(channel.message_encoding.as_bytes()),
        md5: HyphaMcapBytes::of(channel.md5.as_bytes()),
        message_count: channel.message_count,
    }
}

/// Add every channel whose topic is the `topic_len` bytes of `topic` (not
/// NUL-terminated) to the selection that `hypha_mcap_next` returns. Does not move the cursor.
///
/// `HYPHA_MCAP_STATUS_ERROR` for a null reader (no error text), a null `topic`, a topic
/// that is not UTF-8, or a topic no channel has; the selection is unchanged then.
///
/// # Safety
/// `reader` must be null or an open reader that no other thread uses during the call.
/// `topic` must be null or point to `topic_len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_mcap_select(
    reader: *mut HyphaMcapReader,
    topic: *const u8,
    topic_len: usize,
) -> HyphaMcapStatus {
    // SAFETY: the clause on `reader` of this function's contract.
    let Some(reader) = (unsafe { reader_mut(reader) }) else {
        return HyphaMcapStatus::Error;
    };
    reader.last_error.clear();
    // SAFETY: the clause on `topic` of this function's contract.
    let Some(bytes) = (unsafe { borrow_bytes(topic, topic_len) }) else {
        return reader.fail("hypha_mcap_select: topic is null".to_owned());
    };
    let Ok(name) = std::str::from_utf8(bytes) else {
        return reader.fail("hypha_mcap_select: topic is not UTF-8".to_owned());
    };
    match reader.reader.select(name) {
        Ok(()) => HyphaMcapStatus::Ok,
        Err(error) => reader.fail(error.to_string()),
    }
}

/// Move the cursor back to the first message; the selection is kept. Ends the validity
/// of the last message's payload.
///
/// `HYPHA_MCAP_STATUS_ERROR` for a null reader (no error text).
///
/// # Safety
/// `reader` must be null or an open reader that no other thread uses during the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_mcap_rewind(reader: *mut HyphaMcapReader) -> HyphaMcapStatus {
    // SAFETY: the clause on `reader` of this function's contract.
    let Some(reader) = (unsafe { reader_mut(reader) }) else {
        return HyphaMcapStatus::Error;
    };
    reader.last_error.clear();
    reader.reader.rewind();
    HyphaMcapStatus::Ok
}

/// Write the next message of a selected topic to `*out`, in non-decreasing log time,
/// ties in file order. The payload is valid until the following `hypha_mcap_next`,
/// `hypha_mcap_rewind` or `hypha_mcap_close` on this reader.
///
/// `HYPHA_MCAP_STATUS_END` when no selected message is left (at once with no topic
/// selected), until `hypha_mcap_rewind`; nothing is written then.
/// `HYPHA_MCAP_STATUS_ERROR` for a null reader (no error text) or a null `out`.
///
/// # Safety
/// `reader` must be null or an open reader that no other thread uses during the call.
/// `out` must be null or point to writable storage for one `HyphaMcapMessage`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_mcap_next(
    reader: *mut HyphaMcapReader,
    out: *mut HyphaMcapMessage,
) -> HyphaMcapStatus {
    // SAFETY: the clause on `reader` of this function's contract.
    let Some(reader) = (unsafe { reader_mut(reader) }) else {
        return HyphaMcapStatus::Error;
    };
    reader.last_error.clear();
    if out.is_null() {
        return reader.fail("hypha_mcap_next: out is null".to_owned());
    }
    let Some(message) = reader.reader.next_message() else {
        return HyphaMcapStatus::End;
    };
    let message = HyphaMcapMessage {
        channel_index: message.channel_index,
        log_time_ns: message.log_time_ns,
        publish_time_ns: message.publish_time_ns,
        payload: HyphaMcapBytes::of(message.payload),
    };
    // SAFETY: the clause on `out` of this function's contract; non-null by the check above.
    unsafe { write_out(out, message) };
    HyphaMcapStatus::Ok
}

/// The text of the last call on this reader that returned `HYPHA_MCAP_STATUS_ERROR`,
/// borrowed from the reader: empty when the last call succeeded or returned
/// `HYPHA_MCAP_STATUS_END`, and `{ NULL, 0 }` for a null reader. Valid until the next
/// call on this reader other than `hypha_mcap_last_error` and `hypha_mcap_channel_count`.
///
/// # Safety
/// `reader` must be null or an open reader that no other thread uses during the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_mcap_last_error(reader: *const HyphaMcapReader) -> HyphaMcapBytes {
    // SAFETY: the clause on `reader` of this function's contract.
    unsafe { reader_ref(reader) }
        .map_or(HyphaMcapBytes::EMPTY, |reader| HyphaMcapBytes::of(reader.last_error.as_bytes()))
}

/// Free a reader and everything borrowed from it. A null reader does nothing.
///
/// # Safety
/// `reader` must be null or an open reader that no other thread uses during the call.
/// It must not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_mcap_close(reader: *mut HyphaMcapReader) {
    if reader.is_null() {
        return;
    }
    // SAFETY: non-null; from `Box::into_raw` in `hypha_mcap_open`, not used afterwards.
    drop(unsafe { Box::from_raw(reader) });
}
