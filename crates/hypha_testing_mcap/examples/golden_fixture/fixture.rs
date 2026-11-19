//! The golden-sample fixture of the C++ `hypha::testing::McapReplay` tests
//! (`crates/hypha-cxx/test/test_mcap_replay.cpp`), built in memory.
//!
//! Every time is a literal, in nanoseconds since the epoch, from a base of 100 s, which
//! is a whole multiple of the 50 ms period the tests replay at. Each message's publish
//! time is 3 ms before its log time, so a test can tell which of the two a replay uses.
//! The payloads are ROS 1 serialized by hand: a `std_msgs/Float64` is its `f64` and a
//! `std_msgs/UInt32` its `u32`, little-endian. The type names and MD5s are those the
//! generated C++ types carry.

use std::collections::BTreeMap;
use std::io::Cursor;

use mcap::records::MessageHeader;
use mcap::{Compression, WriteOptions, Writer};

/// The fixture file under `crates/hypha-cxx/test/data/`.
pub(crate) const PLAIN_FILE: &str = "mcap_replay_fixture.mcap";
/// The same messages, zstd-compressed in small chunks.
pub(crate) const ZSTD_FILE: &str = "mcap_replay_fixture_zstd.mcap";

/// The publish time of every message lies this far before its log time.
const PUBLISH_LEAD_NANOS: u64 = 3_000_000;

const FLOAT64: &str = "std_msgs/Float64";
const FLOAT64_MD5: &str = "fdb28210bfa9d7c91146260178d9a584";
const FLOAT64_DEFINITION: &[u8] = b"float64 data";
const UINT32: &str = "std_msgs/UInt32";
const UINT32_MD5: &str = "304a39449588c7f8ce2df6e8001c5fce";
const UINT32_DEFINITION: &[u8] = b"uint32 data";

/// One channel: topic, schema name, schema encoding, schema text, message encoding and
/// the MD5 written to the channel metadata.
struct ChannelSpec {
    topic: &'static str,
    type_name: &'static str,
    schema_encoding: &'static str,
    definition: &'static [u8],
    message_encoding: &'static str,
    md5: &'static str,
}

/// The channels, in the order the file defines them.
const CHANNELS: [ChannelSpec; 7] = [
    // 0: the Latest input of the tests.
    ChannelSpec {
        topic: "/position",
        type_name: FLOAT64,
        schema_encoding: "ros1msg",
        definition: FLOAT64_DEFINITION,
        message_encoding: "ros1",
        md5: FLOAT64_MD5,
    },
    // 1: the Arrivals input of the tests.
    ChannelSpec {
        topic: "/marks",
        type_name: UINT32,
        schema_encoding: "ros1msg",
        definition: UINT32_DEFINITION,
        message_encoding: "ros1",
        md5: UINT32_MD5,
    },
    // 2: the golden output: ten times the position, at each deadline where it is usable.
    ChannelSpec {
        topic: "/golden",
        type_name: FLOAT64,
        schema_encoding: "ros1msg",
        definition: FLOAT64_DEFINITION,
        message_encoding: "ros1",
        md5: FLOAT64_MD5,
    },
    // 3: not ROS 1.
    ChannelSpec {
        topic: "/json",
        type_name: "Thing",
        schema_encoding: "jsonschema",
        definition: b"{}",
        message_encoding: "json",
        md5: "",
    },
    // 4: a Float64 recorded with another MD5.
    ChannelSpec {
        topic: "/bad_md5",
        type_name: FLOAT64,
        schema_encoding: "ros1msg",
        definition: FLOAT64_DEFINITION,
        message_encoding: "ros1",
        md5: "00000000000000000000000000000000",
    },
    // 5: a Float64 whose one payload is 3 bytes long.
    ChannelSpec {
        topic: "/truncated",
        type_name: FLOAT64,
        schema_encoding: "ros1msg",
        definition: FLOAT64_DEFINITION,
        message_encoding: "ros1",
        md5: FLOAT64_MD5,
    },
    // 6: a Float64 channel with no message.
    ChannelSpec {
        topic: "/silent",
        type_name: FLOAT64,
        schema_encoding: "ros1msg",
        definition: FLOAT64_DEFINITION,
        message_encoding: "ros1",
        md5: FLOAT64_MD5,
    },
];

/// A payload of the fixture.
enum Payload {
    Float64(f64),
    UInt32(u32),
    Bytes(&'static [u8]),
}

impl Payload {
    fn encode(&self) -> Vec<u8> {
        match self {
            Self::Float64(value) => value.to_le_bytes().to_vec(),
            Self::UInt32(value) => value.to_le_bytes().to_vec(),
            Self::Bytes(bytes) => bytes.to_vec(),
        }
    }
}

/// The messages: channel index, log time, payload. Written in this order, which is
/// log-time order.
const MESSAGES: [(usize, u64, Payload); 16] = [
    (0, 100_010_000_000, Payload::Float64(1.0)),
    (1, 100_020_000_000, Payload::UInt32(1)),
    (5, 100_030_000_000, Payload::Bytes(&[1, 2, 3])),
    (4, 100_040_000_000, Payload::Float64(7.0)),
    (2, 100_050_000_000, Payload::Float64(10.0)),
    (0, 100_060_000_000, Payload::Float64(2.0)),
    (1, 100_070_000_000, Payload::UInt32(2)),
    (1, 100_090_000_000, Payload::UInt32(3)),
    // Exactly on the deadline at 100.100 s.
    (0, 100_100_000_000, Payload::Float64(3.0)),
    (2, 100_100_000_000, Payload::Float64(20.0)),
    (2, 100_150_000_000, Payload::Float64(30.0)),
    (2, 100_200_000_000, Payload::Float64(30.0)),
    (1, 100_210_000_000, Payload::UInt32(4)),
    (0, 100_260_000_000, Payload::Float64(4.0)),
    (2, 100_300_000_000, Payload::Float64(40.0)),
    (3, 100_300_000_000, Payload::Bytes(b"{}")),
];

/// The plain fixture: chunked, uncompressed.
///
/// # Errors
/// Returns the writer's error text.
pub(crate) fn plain() -> Result<Vec<u8>, String> {
    write(deterministic().compression(None))
}

/// The compressed copy: zstd, in chunks of at most 128 bytes.
///
/// # Errors
/// Returns the writer's error text.
pub(crate) fn zstd() -> Result<Vec<u8>, String> {
    write(deterministic().compression(Some(Compression::Zstd)).chunk_size(Some(128)))
}

/// Writer options that give the same bytes on every run. The `mcap` writer repeats the
/// schemas and channels in the summary in hash-map order, so the summary holds neither;
/// the data section defines each of them once, before its first message.
fn deterministic() -> WriteOptions {
    WriteOptions::new().repeat_schemas(false).repeat_channels(false)
}

fn write(options: WriteOptions) -> Result<Vec<u8>, String> {
    let failed = |error: mcap::McapError| error.to_string();
    let mut writer: Writer<Cursor<Vec<u8>>> =
        options.create(Cursor::new(Vec::new())).map_err(failed)?;
    let mut ids = Vec::new();
    for channel in &CHANNELS {
        let schema = writer
            .add_schema(channel.type_name, channel.schema_encoding, channel.definition)
            .map_err(failed)?;
        let mut metadata = BTreeMap::new();
        if !channel.md5.is_empty() {
            metadata
                .insert(hypha_testing_mcap::MD5_METADATA_KEY.to_owned(), channel.md5.to_owned());
        }
        let id = writer
            .add_channel(schema, channel.topic, channel.message_encoding, &metadata)
            .map_err(failed)?;
        ids.push(id);
    }
    for (sequence, (channel, log_time, payload)) in (0_u32..).zip(&MESSAGES) {
        let channel_id = *ids.get(*channel).ok_or("a message names no channel")?;
        let publish_time =
            log_time.checked_sub(PUBLISH_LEAD_NANOS).ok_or("a log time before the lead")?;
        let header = MessageHeader { channel_id, sequence, log_time: *log_time, publish_time };
        writer.write_to_known_channel(&header, &payload.encode()).map_err(failed)?;
    }
    writer.finish().map_err(failed)?;
    Ok(writer.into_inner().into_inner())
}
