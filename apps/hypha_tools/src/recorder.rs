//! Pure core of `hypha record`: writes MCAP containers with schemas, channels, and message records.

use crate::stream::{stream_title, type_label};
use mcap::records::MessageHeader;
use mcap::{Compression, WriteOptions, Writer};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

/// Profile name for ROS 1 MCAP recordings.
pub const MCAP_PROFILE: &str = "ros1";
/// Standard ROS 1 message encoding identifier.
pub const ROS1_MESSAGE_ENCODING: &str = "ros1";
/// Standard ROS 1 schema encoding identifier.
pub const ROS1_SCHEMA_ENCODING: &str = "ros1msg";
/// Key for MD5 hash in channel metadata. `hypha_testing_mcap::MD5_METADATA_KEY`, the
/// golden-sample reader's copy, must stay equal to it.
pub const MD5_METADATA_KEY: &str = "md5sum";

/// Chunk compression of an MCAP recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum McapCompression {
    /// LZ4 chunk compression (default).
    #[default]
    Lz4,
    /// Zstandard chunk compression.
    Zstd,
    /// Uncompressed chunks.
    None,
}

/// A decoded bus sample ready to be recorded into MCAP.
#[derive(Debug, Clone)]
pub struct RecordSample<'a> {
    /// Bus topic name.
    pub topic: &'a str,
    /// Message data type name (e.g. `"std_msgs/Float64"`).
    pub data_type: &'a str,
    /// 32-hex MD5 digest.
    pub md5: &'a str,
    /// Full definition text (empty if unresolved).
    pub definition: &'a str,
    /// Serialized payload bytes.
    pub bytes: &'a [u8],
    /// Actual capture UTC, in nanoseconds since 1970-01-01.
    pub log_time_nanos: u64,
    /// Transport send time mapped into UTC, in the same epoch.
    pub publish_time_nanos: u64,
}

/// One MCAP channel: the stream of one schema on one topic.
#[derive(Debug)]
struct ChannelState {
    channel_id: u16,
    sequence: u32,
    messages: u64,
    /// The channel's type as a line names it, `type (md5 start)`.
    label: String,
}

/// What a recording holds of one channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelCount {
    /// The topic name.
    pub topic: String,
    /// The channel's type as a line names it: `type (md5 start)`.
    pub label: String,
    /// Messages written to the channel.
    pub messages: u64,
}

impl ChannelCount {
    /// The channel as a line names it: its topic, and its type and MD5 start when `all`
    /// holds another channel of the topic.
    #[must_use]
    pub fn title(&self, all: &[Self]) -> String {
        let shared = all.iter().filter(|other| other.topic == self.topic).take(2).count() > 1;
        stream_title(&self.topic, &self.label, shared)
    }
}

/// The stream a sample is written to: its topic and MD5, not case sensitive.
type ChannelKey = (String, String);

/// MCAP recorder managing channels, schemas, and thread-safe chunked recording.
pub struct Recorder {
    writer: Option<Writer<BufWriter<File>>>,
    opened: bool,
    error: String,
    state: RecorderState,
}

#[derive(Debug, Default)]
struct RecorderState {
    channels: BTreeMap<ChannelKey, ChannelState>,
    channel_order: Vec<ChannelKey>,
}

impl Recorder {
    /// Creates a new `Recorder` opening the destination file with default (LZ4) compression.
    pub fn new<P: AsRef<Path>>(path: P) -> Self {
        Self::with_compression(path, McapCompression::Lz4)
    }

    /// A recorder that failed to open, with the reason.
    fn failed(error: String) -> Self {
        Self { writer: None, opened: false, error, state: RecorderState::default() }
    }

    /// Creates a new `Recorder` with specified compression algorithm.
    pub fn with_compression<P: AsRef<Path>>(path: P, compression: McapCompression) -> Self {
        let file_res = File::create(path.as_ref());
        match file_res {
            Ok(file) => {
                let buf = BufWriter::new(file);
                let mut write_options = WriteOptions::new().profile(MCAP_PROFILE);
                write_options = match compression {
                    McapCompression::Zstd => write_options.compression(Some(Compression::Zstd)),
                    McapCompression::None => write_options.compression(None),
                    McapCompression::Lz4 => write_options.compression(Some(Compression::Lz4)),
                };

                match Writer::with_options(buf, write_options) {
                    Ok(writer) => Self {
                        writer: Some(writer),
                        opened: true,
                        error: String::new(),
                        state: RecorderState::default(),
                    },
                    Err(e) => Self::failed(e.to_string()),
                }
            }
            Err(e) => Self::failed(e.to_string()),
        }
    }

    /// Whether the writer was successfully opened.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.opened
    }

    /// Returns error message if `is_open()` is false.
    #[must_use]
    pub fn open_error(&self) -> &str {
        &self.error
    }

    /// Records a single sample, declaring schema and channel if its stream, the topic and the
    /// MD5, is first seen. One topic with two schemas is two channels.
    pub fn record(&mut self, sample: &RecordSample<'_>) -> bool {
        let Some(ref mut writer) = self.writer else {
            return false;
        };

        let state = &mut self.state;
        let key = (sample.topic.to_owned(), sample.md5.to_ascii_lowercase());
        let (channel_id, sequence) = if let Some(ts) = state.channels.get_mut(&key) {
            let seq = ts.sequence;
            ts.sequence = ts.sequence.saturating_add(1);
            ts.messages = ts.messages.saturating_add(1);
            (ts.channel_id, seq)
        } else {
            let Ok(schema_id) = writer.add_schema(
                sample.data_type,
                ROS1_SCHEMA_ENCODING,
                sample.definition.as_bytes(),
            ) else {
                return false;
            };

            let mut metadata = BTreeMap::new();
            metadata.insert(MD5_METADATA_KEY.to_string(), sample.md5.to_owned());

            let Ok(channel_id) =
                writer.add_channel(schema_id, sample.topic, ROS1_MESSAGE_ENCODING, &metadata)
            else {
                return false;
            };

            state.channel_order.push(key.clone());
            let label = type_label(sample.data_type, sample.md5);
            state
                .channels
                .insert(key, ChannelState { channel_id, sequence: 1, messages: 1, label });

            (channel_id, 0)
        };

        let header = MessageHeader {
            channel_id,
            sequence,
            log_time: sample.log_time_nanos,
            publish_time: sample.publish_time_nanos,
        };

        writer.write_to_known_channel(&header, sample.bytes).is_ok()
    }

    /// Finalizes the MCAP file, writing the summary section and footer. Only the first
    /// call writes them; a later one does nothing.
    ///
    /// # Errors
    /// The writer's error when the summary or the footer cannot be written.
    pub fn finish(&mut self) -> Result<(), String> {
        match self.writer.take() {
            Some(mut writer) => writer
                .finish()
                .map(|_| ())
                .map_err(|e| format!("failed to finalize mcap index: {e}")),
            None => Ok(()),
        }
    }

    /// Returns per-channel recorded message counts in order of discovery.
    #[must_use]
    pub fn counters(&self) -> Vec<ChannelCount> {
        let state = &self.state;
        state
            .channel_order
            .iter()
            .filter_map(|key| {
                let channel = state.channels.get(key)?;
                Some(ChannelCount {
                    topic: key.0.clone(),
                    label: channel.label.clone(),
                    messages: channel.messages,
                })
            })
            .collect()
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        if let Err(error) = self.finish() {
            eprintln!("hypha record: {error}");
        }
    }
}
