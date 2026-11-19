//! The reader: one MCAP file in memory, its channels, a topic selection and a cursor
//! over its messages in log-time order.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use mcap::McapError;
use mcap::records::{self, Record};
use mcap::sans_io::{LinearReadEvent, LinearReader, LinearReaderOptions};

/// The channel metadata key under which hypha's recorder stores a channel's ROS 1 MD5.
///
/// `MD5_METADATA_KEY` in `apps/hypha_tools/src/recorder.rs` must stay equal to it.
pub const MD5_METADATA_KEY: &str = "md5sum";

/// The largest file [`Reader::open`] reads, 1 GiB: the whole file is held in memory.
pub const MAX_FILE_BYTES: u64 = 1_073_741_824;

/// Why a file could not be read, or a topic not selected. Each variant names the file.
#[derive(Debug)]
pub enum Error {
    /// The file could not be opened or read.
    Io {
        /// The file.
        path: PathBuf,
        /// The operating-system error.
        source: std::io::Error,
    },
    /// The file is larger than the bound the reader holds in memory.
    TooLarge {
        /// The file.
        path: PathBuf,
        /// Its size in bytes.
        size: u64,
        /// The bound, [`MAX_FILE_BYTES`].
        limit: u64,
    },
    /// The file is not a valid MCAP file. `source` names the fault.
    Format {
        /// The file.
        path: PathBuf,
        /// The fault, as the `mcap` crate reports it.
        source: McapError,
    },
    /// A channel names a schema id that the file does not define.
    MissingSchema {
        /// The file.
        path: PathBuf,
        /// The topic of the channel.
        topic: String,
        /// The schema id it names.
        schema_id: u16,
    },
    /// Two records with the same schema id differ.
    ConflictingSchema {
        /// The file.
        path: PathBuf,
        /// The schema id.
        schema_id: u16,
    },
    /// Two records with the same channel id differ.
    ConflictingChannel {
        /// The file.
        path: PathBuf,
        /// The channel id.
        channel_id: u16,
    },
    /// A message names a channel id that no earlier channel record defines.
    UnknownChannel {
        /// The file.
        path: PathBuf,
        /// The channel id.
        channel_id: u16,
        /// The sequence number of the message.
        sequence: u32,
    },
    /// [`Reader::select`] was given a topic that no channel of the file has.
    UnknownTopic {
        /// The file.
        path: PathBuf,
        /// The topic.
        topic: String,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(f, "MCAP file '{}': cannot read it: {source}", path.display())
            }
            Self::TooLarge { path, size, limit } => write!(
                f,
                "MCAP file '{}': {size} bytes is larger than the {limit} bytes a test fixture may have",
                path.display()
            ),
            Self::Format { path, source } => {
                write!(f, "MCAP file '{}': {source}", path.display())?;
                if let Some(cause) = std::error::Error::source(source) {
                    write!(f, ": {cause}")?;
                }
                Ok(())
            }
            Self::MissingSchema { path, topic, schema_id } => write!(
                f,
                "MCAP file '{}': channel '{topic}' names schema {schema_id}, which the file does not define",
                path.display()
            ),
            Self::ConflictingSchema { path, schema_id } => write!(
                f,
                "MCAP file '{}': two different records define schema {schema_id}",
                path.display()
            ),
            Self::ConflictingChannel { path, channel_id } => write!(
                f,
                "MCAP file '{}': two different records define channel {channel_id}",
                path.display()
            ),
            Self::UnknownChannel { path, channel_id, sequence } => write!(
                f,
                "MCAP file '{}': message {sequence} names channel {channel_id}, which no earlier record defines",
                path.display()
            ),
            Self::UnknownTopic { path, topic } => {
                write!(f, "MCAP file '{}': no channel has topic '{topic}'", path.display())
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Format { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// One channel of the file, in the order the file first defines it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Channel {
    /// The topic.
    pub topic: String,
    /// The schema name (`geometry_msgs/Point`); empty for a channel without a schema.
    pub type_name: String,
    /// The schema encoding: `ros1msg` for a ROS 1 message; empty without a schema.
    pub schema_encoding: String,
    /// The message encoding (`ros1` for a ROS 1 message).
    pub message_encoding: String,
    /// The ROS 1 MD5 from the channel metadata ([`MD5_METADATA_KEY`]), empty when absent.
    pub md5: String,
    /// The number of messages of the channel in the file.
    pub message_count: u64,
}

/// One message, borrowed from the reader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Message<'a> {
    /// The index of its channel in [`Reader::channels`].
    pub channel_index: usize,
    /// Its log time in nanoseconds.
    pub log_time_ns: u64,
    /// Its publish time in nanoseconds.
    pub publish_time_ns: u64,
    /// Its payload as stored (a ROS 1 serialized message for a `ros1` channel).
    pub payload: &'a [u8],
}

/// A message held by the reader.
#[derive(Debug)]
struct Stored {
    channel_index: usize,
    log_time_ns: u64,
    publish_time_ns: u64,
    payload: Box<[u8]>,
}

/// One MCAP file in memory: its channels, the selected topics and a cursor over the
/// messages in log-time order, ties in file order.
#[derive(Debug)]
pub struct Reader {
    path: PathBuf,
    channels: Vec<Channel>,
    messages: Vec<Stored>,
    /// One flag per channel, set by [`Reader::select`].
    selected: Vec<bool>,
    /// The index in `messages` of the next message to look at.
    cursor: usize,
}

impl Reader {
    /// Read and check the whole file at `path`. No topic is selected.
    ///
    /// # Errors
    /// [`Error::Io`] when the file cannot be read, [`Error::TooLarge`] above
    /// [`MAX_FILE_BYTES`], and [`Error::Format`], [`Error::MissingSchema`],
    /// [`Error::ConflictingSchema`], [`Error::ConflictingChannel`] or
    /// [`Error::UnknownChannel`] for a file that is not a valid MCAP file.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        Self::open_with_limit(path.as_ref(), MAX_FILE_BYTES)
    }

    /// [`Reader::open`] with another size bound.
    fn open_with_limit(path: &Path, limit: u64) -> Result<Self, Error> {
        let bytes = read_bounded(path, limit)?;
        let mut scan = Scan::new(path);
        scan.run(&bytes)?;
        let Scan { channels, mut messages, .. } = scan;
        // A stable sort: messages with equal log times keep their file order.
        messages.sort_by_key(|message| message.log_time_ns);
        let selected = vec![false; channels.len()];
        Ok(Self { path: path.to_path_buf(), channels, messages, selected, cursor: 0 })
    }

    /// The file this reader read.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The channels, in the order the file first defines them.
    #[must_use]
    pub fn channels(&self) -> &[Channel] {
        &self.channels
    }

    /// Add every channel with `topic` to the selection. The cursor does not move.
    ///
    /// # Errors
    /// [`Error::UnknownTopic`] when no channel has `topic`; the selection is unchanged.
    pub fn select(&mut self, topic: &str) -> Result<(), Error> {
        let mut found = false;
        for (channel, selected) in self.channels.iter().zip(self.selected.iter_mut()) {
            if channel.topic == topic {
                *selected = true;
                found = true;
            }
        }
        if found {
            Ok(())
        } else {
            Err(Error::UnknownTopic { path: self.path.clone(), topic: topic.to_owned() })
        }
    }

    /// Move the cursor back to the first message. The selection is kept.
    pub fn rewind(&mut self) {
        self.cursor = 0;
    }

    /// The next message of a selected topic in log-time order, ties in file order;
    /// `None` at the end until [`Reader::rewind`], and at once with no topic selected.
    pub fn next_message(&mut self) -> Option<Message<'_>> {
        let remaining = self.messages.get(self.cursor..)?;
        let offset = remaining
            .iter()
            .position(|stored| self.selected.get(stored.channel_index).copied().unwrap_or(false));
        let Some(offset) = offset else {
            self.cursor = self.messages.len();
            return None;
        };
        let index = self.cursor.checked_add(offset)?;
        // `index < messages.len()`, so `index + 1` cannot overflow.
        self.cursor = index.saturating_add(1);
        let stored = self.messages.get(index)?;
        Some(Message {
            channel_index: stored.channel_index,
            log_time_ns: stored.log_time_ns,
            publish_time_ns: stored.publish_time_ns,
            payload: &stored.payload,
        })
    }
}

/// The bytes of `path`, refused above `limit`.
fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, Error> {
    let io_error = |source| Error::Io { path: path.to_path_buf(), source };
    let file = File::open(path).map_err(io_error)?;
    let size = file.metadata().map_err(io_error)?.len();
    if size > limit {
        return Err(Error::TooLarge { path: path.to_path_buf(), size, limit });
    }
    let mut bytes = Vec::new();
    // `take` bounds a file that grows after the size check; the scan refuses it as truncated.
    file.take(limit).read_to_end(&mut bytes).map_err(io_error)?;
    Ok(bytes)
}

/// A schema as the file defines it.
#[derive(PartialEq, Eq)]
struct SchemaRecord {
    name: String,
    encoding: String,
    data: Vec<u8>,
}

/// A channel as the file defines it, and its index in the channel list.
struct ChannelRecord {
    schema_id: u16,
    topic: String,
    message_encoding: String,
    metadata: BTreeMap<String, String>,
    index: usize,
}

/// The state of one pass over the file's records.
struct Scan<'p> {
    path: &'p Path,
    schemas: BTreeMap<u16, SchemaRecord>,
    channel_records: BTreeMap<u16, ChannelRecord>,
    channels: Vec<Channel>,
    messages: Vec<Stored>,
}

impl<'p> Scan<'p> {
    fn new(path: &'p Path) -> Self {
        Self {
            path,
            schemas: BTreeMap::new(),
            channel_records: BTreeMap::new(),
            channels: Vec::new(),
            messages: Vec::new(),
        }
    }

    fn format(&self, source: McapError) -> Error {
        Error::Format { path: self.path.to_path_buf(), source }
    }

    /// Read every record of `bytes`, with every check the `mcap` reader offers.
    fn run(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let options = LinearReaderOptions::default()
            .with_check_finishes_after_end_magic(true)
            .with_validate_chunk_crcs(true)
            .with_validate_data_section_crc(true)
            .with_validate_summary_section_crc(true)
            .with_record_length_limit(bytes.len());
        let mut reader = LinearReader::new_with_options(options);
        let mut rest = bytes;
        while let Some(event) = reader.next_event() {
            match event.map_err(|source| self.format(source))? {
                LinearReadEvent::ReadRequest(wanted) => {
                    let (head, tail) = rest.split_at(wanted.min(rest.len()));
                    reader.insert(head.len()).copy_from_slice(head);
                    // Zero bytes tell the reader the file has ended.
                    reader.notify_read(head.len());
                    rest = tail;
                }
                LinearReadEvent::Record { data, opcode } => {
                    let record =
                        mcap::parse_record(opcode, data).map_err(|source| self.format(source))?;
                    self.add(record)?;
                }
            }
        }
        Ok(())
    }

    fn add(&mut self, record: Record<'_>) -> Result<(), Error> {
        match record {
            Record::Schema { header, data } => self.add_schema(header, &data),
            Record::Channel(channel) => self.add_channel(channel),
            Record::Message { header, data } => self.add_message(&header, &data),
            _ => Ok(()),
        }
    }

    fn add_schema(&mut self, header: records::SchemaHeader, data: &[u8]) -> Result<(), Error> {
        if header.id == 0 {
            return Err(self.format(McapError::InvalidSchemaId));
        }
        let schema =
            SchemaRecord { name: header.name, encoding: header.encoding, data: data.to_vec() };
        match self.schemas.get(&header.id) {
            Some(known) if *known == schema => Ok(()),
            Some(_) => Err(Error::ConflictingSchema {
                path: self.path.to_path_buf(),
                schema_id: header.id,
            }),
            None => {
                self.schemas.insert(header.id, schema);
                Ok(())
            }
        }
    }

    fn add_channel(&mut self, channel: records::Channel) -> Result<(), Error> {
        if let Some(known) = self.channel_records.get(&channel.id) {
            let same = known.schema_id == channel.schema_id
                && known.topic == channel.topic
                && known.message_encoding == channel.message_encoding
                && known.metadata == channel.metadata;
            return if same {
                Ok(())
            } else {
                Err(Error::ConflictingChannel {
                    path: self.path.to_path_buf(),
                    channel_id: channel.id,
                })
            };
        }
        // Schema id 0 is a channel without a schema.
        let (type_name, schema_encoding) = if channel.schema_id == 0 {
            (String::new(), String::new())
        } else {
            let Some(schema) = self.schemas.get(&channel.schema_id) else {
                return Err(Error::MissingSchema {
                    path: self.path.to_path_buf(),
                    topic: channel.topic,
                    schema_id: channel.schema_id,
                });
            };
            (schema.name.clone(), schema.encoding.clone())
        };
        let index = self.channels.len();
        self.channels.push(Channel {
            topic: channel.topic.clone(),
            type_name,
            schema_encoding,
            message_encoding: channel.message_encoding.clone(),
            md5: channel.metadata.get(MD5_METADATA_KEY).cloned().unwrap_or_default(),
            message_count: 0,
        });
        self.channel_records.insert(
            channel.id,
            ChannelRecord {
                schema_id: channel.schema_id,
                topic: channel.topic,
                message_encoding: channel.message_encoding,
                metadata: channel.metadata,
                index,
            },
        );
        Ok(())
    }

    fn add_message(&mut self, header: &records::MessageHeader, data: &[u8]) -> Result<(), Error> {
        let Some(index) = self.channel_records.get(&header.channel_id).map(|known| known.index)
        else {
            return Err(Error::UnknownChannel {
                path: self.path.to_path_buf(),
                channel_id: header.channel_id,
                sequence: header.sequence,
            });
        };
        if let Some(channel) = self.channels.get_mut(index) {
            channel.message_count = channel.message_count.saturating_add(1);
        }
        self.messages.push(Stored {
            channel_index: index,
            log_time_ns: header.log_time,
            publish_time_ns: header.publish_time,
            payload: data.into(),
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[expect(clippy::expect_used, reason = "a failed test setup stops the test with its message")]
    fn a_file_above_the_size_bound_is_refused_with_its_size_and_path() {
        let path = std::env::temp_dir()
            .join(format!("hypha_testing_mcap_{}_too_large.mcap", std::process::id()));
        std::fs::write(&path, [0_u8; 64]).expect("fixture written");
        let result = Reader::open_with_limit(&path, 16);
        std::fs::remove_file(&path).expect("fixture removed");
        let error = result.expect_err("refused");
        assert!(matches!(error, Error::TooLarge { size: 64, limit: 16, .. }), "{error:?}");
        let text = error.to_string();
        assert!(text.contains("64 bytes"), "{text}");
        assert!(text.contains(&path.display().to_string()), "{text}");
    }

    #[test]
    fn the_size_bound_is_one_gibibyte() {
        assert_eq!(MAX_FILE_BYTES, 1024 * 1024 * 1024);
    }
}
