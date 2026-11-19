//! MCAP fixtures for the reader tests.

#![allow(dead_code, reason = "each test binary uses a different part of the helpers")]

use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use mcap::records::MessageHeader;
use mcap::{Compression, WriteOptions, Writer};

/// The MD5 of `geometry_msgs/Point`, as the recorder stores it.
pub(crate) const POINT_MD5: &str = "4a842b65f413084dc2b10fb484ea7f17";

/// A file in the temporary directory, removed when dropped.
pub(crate) struct TempFile {
    path: PathBuf,
}

impl TempFile {
    /// A fresh path for `name`, unique to this process and call.
    pub(crate) fn new(name: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let file = format!("hypha_testing_mcap_{}_{serial}_{name}.mcap", std::process::id());
        Self { path: std::env::temp_dir().join(file) }
    }

    /// `bytes` written to a fresh path for `name`.
    pub(crate) fn with_bytes(name: &str, bytes: &[u8]) -> Self {
        let file = Self::new(name);
        std::fs::write(&file.path, bytes).expect("fixture written");
        file
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        // A file a test never wrote is not there to remove.
        let _ = std::fs::remove_file(&self.path);
    }
}

/// One channel of a fixture.
pub(crate) struct ChannelSpec {
    pub(crate) topic: &'static str,
    /// Empty for a channel without a schema.
    pub(crate) type_name: &'static str,
    pub(crate) schema_encoding: &'static str,
    /// The MD5 written to the channel metadata, as the recorder writes it.
    pub(crate) md5: Option<&'static str>,
}

/// One message of a fixture.
pub(crate) struct MessageSpec {
    pub(crate) channel: usize,
    pub(crate) log_time: u64,
    pub(crate) publish_time: u64,
    pub(crate) payload: Vec<u8>,
}

/// A message of `channel` logged at `log_time`, published 1 ns earlier, whose
/// payload names both.
pub(crate) fn message(channel: usize, log_time: u64) -> MessageSpec {
    MessageSpec {
        channel,
        log_time,
        publish_time: log_time.saturating_sub(1),
        payload: format!("payload-{channel}-{log_time}").into_bytes(),
    }
}

/// The two ROS 1 channels most fixtures use: `/position` with its MD5 in the channel
/// metadata, `/switches` without one.
pub(crate) fn ros1_channels() -> Vec<ChannelSpec> {
    vec![
        ChannelSpec {
            topic: "/position",
            type_name: "geometry_msgs/Point",
            schema_encoding: "ros1msg",
            md5: Some(POINT_MD5),
        },
        ChannelSpec {
            topic: "/switches",
            type_name: "std_msgs/Bool",
            schema_encoding: "ros1msg",
            md5: None,
        },
    ]
}

/// Writer options: chunked with `compression`, at most `chunk_size` bytes per chunk.
pub(crate) fn options(compression: Option<Compression>, chunk_size: Option<u64>) -> WriteOptions {
    WriteOptions::new().compression(compression).chunk_size(chunk_size)
}

/// An MCAP file of `channels` and `messages`, written with `options`.
pub(crate) fn write_mcap(
    options: WriteOptions,
    channels: &[ChannelSpec],
    messages: &[MessageSpec],
) -> Vec<u8> {
    let mut writer: Writer<Cursor<Vec<u8>>> =
        options.create(Cursor::new(Vec::new())).expect("writer");
    let mut ids = Vec::new();
    for channel in channels {
        // No type name: a channel without a schema (schema id 0).
        let schema = if channel.type_name.is_empty() {
            0
        } else {
            writer
                .add_schema(channel.type_name, channel.schema_encoding, b"float64 x\n")
                .expect("schema")
        };
        let mut metadata = BTreeMap::new();
        if let Some(md5) = channel.md5 {
            metadata.insert(hypha_testing_mcap::MD5_METADATA_KEY.to_owned(), md5.to_owned());
        }
        ids.push(writer.add_channel(schema, channel.topic, "ros1", &metadata).expect("channel"));
    }
    for (sequence, message) in (0_u32..).zip(messages) {
        let header = MessageHeader {
            channel_id: *ids.get(message.channel).expect("channel index"),
            sequence,
            log_time: message.log_time,
            publish_time: message.publish_time,
        };
        writer.write_to_known_channel(&header, &message.payload).expect("message");
    }
    writer.finish().expect("finish");
    writer.into_inner().into_inner()
}

/// Two topics interleaved, written with `compression`: `/position` at 10, 30, 50 ms
/// and `/switches` at 20, 40 ms.
pub(crate) fn interleaved(compression: Option<Compression>) -> Vec<u8> {
    let messages = [
        message(0, 10_000_000),
        message(1, 20_000_000),
        message(0, 30_000_000),
        message(1, 40_000_000),
        message(0, 50_000_000),
    ];
    write_mcap(options(compression, None), &ros1_channels(), &messages)
}

/// The offset of the first occurrence of `needle` in `haystack`.
pub(crate) fn find(haystack: &[u8], needle: &[u8]) -> usize {
    haystack.windows(needle.len()).position(|window| window == needle).expect("needle in file")
}
