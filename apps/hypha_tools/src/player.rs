//! Pure core of `hypha play`: opens MCAP containers, resolves schemas & MD5s, and generates playback decisions.

use crate::approx_f64;
use crate::recorder::MD5_METADATA_KEY;
use crate::stream::StreamKey;
use crate::utc_time::UtcRange;
use hypha_node::sdk::md5::md5_from_definition;
use mcap::MessageStream;
use mcap::sans_io::indexed_reader::{IndexedReadEvent, IndexedReader, IndexedReaderOptions};
use std::collections::HashMap;
use std::fmt::Display;
use std::path::Path;

/// The message of a read failure in the recording `source`. `chunk_offset` is the file
/// offset of the chunk data the reader was in, when the reader gave one: an MCAP error
/// carries no byte offset of its own.
fn read_failure(source: &str, chunk_offset: Option<u64>, error: impl Display) -> String {
    match chunk_offset {
        Some(offset) => format!("'{source}' (chunk data at offset {offset}): {error}"),
        None => format!("'{source}': {error}"),
    }
}

/// Resolved metadata for an MCAP channel.
#[derive(Debug, Clone)]
pub struct ChannelResolution {
    /// Topic name.
    pub topic: String,
    /// The stream the channel replays on: its topic and the schema hash of its MD5, 0 when
    /// the channel has none. Two channels of one topic with two MD5s are two streams.
    pub stream: StreamKey,
    /// Data type name.
    pub data_type: String,
    /// 32-hex MD5 digest (empty if unresolved).
    pub md5: String,
    /// Full message definition text.
    pub definition: String,
    /// The largest message of the channel in the recording, from its message
    /// indexes; `None` when the recording has none.
    pub max_payload: Option<usize>,
}

/// One message of the fallback scan: its log time, its channel and its payload.
type CandidateMessage<'a> = (u64, &'a ChannelResolution, Vec<u8>);

/// A playback decision ready to be published and paced. It borrows the
/// recording; the channel's identity is in [`Player::channels`].
#[derive(Debug, Clone, Copy)]
pub struct PlaybackItem<'a> {
    /// The stream the message is published on.
    pub stream: &'a StreamKey,
    /// Payload bytes.
    pub bytes: &'a [u8],
    /// Delay in nanoseconds relative to previous delivered message.
    pub delay_nanos: u64,
    /// Epoch log time of the message in nanoseconds.
    pub log_time_nanos: u64,
}

/// Bytes of a message record before its data: opcode, record length, channel id,
/// sequence, log time and publish time.
const MESSAGE_RECORD_OVERHEAD: u64 = 1 + 8 + 2 + 4 + 8 + 8;

/// At least the largest message data of each channel, from the message indexes of
/// every chunk. No chunk is decompressed. `None` when a chunk has no message index.
fn max_payloads(storage: &[u8], summary: &mcap::Summary) -> Option<HashMap<u16, usize>> {
    let mut largest: HashMap<u16, usize> = HashMap::new();
    for chunk in &summary.chunk_indexes {
        let indexes = summary.read_message_indexes(storage, chunk).ok()?;
        let mut starts: Vec<(u64, u16)> = indexes
            .iter()
            .flat_map(|(channel, entries)| entries.iter().map(|entry| (entry.offset, channel.id)))
            .collect();
        starts.sort_unstable();
        let ends =
            starts.iter().skip(1).map(|(offset, _)| *offset).chain([chunk.uncompressed_size]);
        for ((start, channel), end) in starts.iter().zip(ends) {
            let data = end.checked_sub(*start)?.checked_sub(MESSAGE_RECORD_OVERHEAD)?;
            let data = usize::try_from(data).ok()?;
            let entry = largest.entry(*channel).or_default();
            *entry = (*entry).max(data);
        }
    }
    Some(largest)
}

fn topic_allowed(topics_filter: &[String], topic: &str) -> bool {
    topics_filter.is_empty() || topics_filter.iter().any(|allowed| allowed == topic)
}

/// Core MCAP reader and channel resolver for playback.
#[derive(Default)]
pub struct Player {
    /// The path as given, named in every read failure.
    source: String,
    storage: Option<hypha_node::sdk::sys::ReadOnlyMap>,
    channels: Vec<ChannelResolution>,
    channel_map: HashMap<u16, ChannelResolution>,
    opened: bool,
    error: String,
    total_messages: u64,
    start_log_time_nanos: u64,
    end_log_time_nanos: u64,
}

impl Player {
    /// Opens an MCAP file from path.
    pub fn new<P: AsRef<Path>>(path: P) -> Self {
        let source = path.as_ref().display().to_string();
        // Mapped, because a recording can be larger than memory. The map is sound only
        // for a complete recording that nothing changes while it is replayed.
        let storage_res = std::fs::File::open(path.as_ref())
            .and_then(|file| hypha_node::sdk::sys::map_read_only(&file));

        match storage_res {
            Ok(storage) => {
                let mut player = Self { source, storage: Some(storage), ..Self::default() };
                // A recording whose messages cannot be read does not open: a replay
                // would stop at the same point, after publishing part of it.
                match player.resolve_channels() {
                    Ok(()) => player.opened = true,
                    Err(error) => player.error = error,
                }
                player
            }
            Err(e) => Self { source, error: e.to_string(), ..Self::default() },
        }
    }

    /// Whether the file opened successfully.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.opened
    }

    /// Open failure error message if `is_open()` is false.
    #[must_use]
    pub fn open_error(&self) -> &str {
        &self.error
    }

    /// Returns list of resolved channels.
    #[must_use]
    pub fn channels(&self) -> &[ChannelResolution] {
        &self.channels
    }

    /// Total messages in recording (if summary available), or 0.
    #[must_use]
    pub fn total_messages(&self) -> u64 {
        self.total_messages
    }

    /// Total duration in seconds, or 0.0.
    #[must_use]
    pub fn duration_seconds(&self) -> f64 {
        if self.end_log_time_nanos >= self.start_log_time_nanos {
            let duration_nanos = self.end_log_time_nanos.saturating_sub(self.start_log_time_nanos);
            approx_f64(duration_nanos) / 1e9
        } else {
            0.0
        }
    }

    /// Resolves the channels from the summary, or from a sequential scan when the
    /// summary is missing or unreadable. A failing scan is an error.
    fn resolve_channels(&mut self) -> Result<(), String> {
        let Some(storage) = self.storage.as_deref() else {
            return Ok(());
        };

        let summary_opt = match mcap::Summary::read(storage) {
            Ok(Some(s)) => Some(s),
            _ => None,
        };
        let max_payloads = summary_opt
            .as_ref()
            .and_then(|summary| max_payloads(storage, summary))
            .unwrap_or_default();

        let mut process_channel = |ch: &mcap::Channel<'_>| {
            if self.channel_map.contains_key(&ch.id) {
                return;
            }

            let (data_type, definition) = if let Some(ref schema) = ch.schema {
                (schema.name.clone(), String::from_utf8_lossy(&schema.data).into_owned())
            } else {
                (String::new(), String::new())
            };

            let mut md5 = ch.metadata.get(MD5_METADATA_KEY).cloned().unwrap_or_default();

            if md5.is_empty()
                && !definition.is_empty()
                && !data_type.is_empty()
                && let Some(derived) = md5_from_definition(&data_type, &definition)
            {
                md5 = derived;
            }

            let max_payload = max_payloads.get(&ch.id).copied();
            let schema_hash = hypha_node::sdk::schema_hash_from_md5(&md5).unwrap_or(0);
            let resolution = ChannelResolution {
                topic: ch.topic.clone(),
                stream: StreamKey { topic: ch.topic.clone(), schema_hash },
                data_type,
                md5,
                definition,
                max_payload,
            };

            self.channels.push(resolution.clone());
            self.channel_map.insert(ch.id, resolution);
        };

        if let Some(summary) = &summary_opt {
            if let Some(stats) = &summary.stats {
                self.total_messages = stats.message_count;
                self.start_log_time_nanos = stats.message_start_time;
                self.end_log_time_nanos = stats.message_end_time;
            }
            // The summary keeps the channels in a hash map: by id, they come in the
            // order the recording made them, the same in every process.
            let mut channels: Vec<_> = summary.channels.iter().collect();
            channels.sort_unstable_by_key(|(id, _)| **id);
            for (_, ch) in channels {
                process_channel(ch.as_ref());
            }
        } else {
            // Fallback: sequential scan
            let messages = MessageStream::new(storage)
                .map_err(|error| read_failure(&self.source, None, error))?;
            for message in messages {
                let msg = message.map_err(|error| read_failure(&self.source, None, error))?;
                process_channel(&msg.channel);
            }
        }
        Ok(())
    }

    /// One resolved, filtered message read off disk, held only for the
    /// fallback path used by unindexed recordings.
    fn collect_candidate_messages(
        &self,
        topics_filter: &[String],
        range: UtcRange,
    ) -> Result<Vec<CandidateMessage<'_>>, String> {
        let mut candidates: Vec<CandidateMessage<'_>> = Vec::new();

        let Some(storage) = self.storage.as_deref() else {
            return Ok(candidates);
        };
        let messages =
            MessageStream::new(storage).map_err(|error| read_failure(&self.source, None, error))?;

        for msg_res in messages {
            let m = msg_res.map_err(|error| read_failure(&self.source, None, error))?;
            let Some(ch) = self.channel_map.get(&m.channel.id) else {
                continue;
            };
            let channel_unresolved = ch.md5.is_empty();
            if channel_unresolved {
                continue;
            }
            if !topic_allowed(topics_filter, &ch.topic) {
                continue;
            }

            if range.contains(m.log_time) {
                candidates.push((m.log_time, ch, m.data.into_owned()));
            }
        }

        Ok(candidates)
    }

    /// Replays through the message index. `Ok(None)` when the recording has no usable
    /// index, so the caller scans it instead.
    fn for_each_indexed_item<F>(
        &self,
        topics_filter: &[String],
        range: UtcRange,
        on_item: &mut F,
    ) -> Result<Option<usize>, String>
    where
        F: FnMut(&PlaybackItem<'_>) -> bool,
    {
        let Some(storage) = self.storage.as_deref() else {
            return Ok(None);
        };
        let Ok(Some(summary)) = mcap::Summary::read(storage) else {
            return Ok(None);
        };
        if summary.chunk_indexes.is_empty() {
            return Ok(None);
        }
        let mut options = IndexedReaderOptions::default();
        if let Some(start) = range.start() {
            options = options.log_time_on_or_after(start);
        }
        if let Some(end) = range.end() {
            options = options.log_time_before(end);
        }
        let mut reader = IndexedReader::new_with_options(&summary, options)
            .map_err(|error| read_failure(&self.source, None, error))?;

        let mut delivered = 0usize;
        let mut previous_log_time = 0u64;
        let mut first = true;
        // The chunk the reader is reading messages from, once it has asked for one.
        let mut current_chunk: Option<u64> = None;
        while let Some(event) = reader.next_event() {
            match event {
                Ok(IndexedReadEvent::Message { header, data }) => {
                    let Some(channel) = self.channel_map.get(&header.channel_id) else {
                        continue;
                    };
                    if channel.md5.is_empty() || !topic_allowed(topics_filter, &channel.topic) {
                        continue;
                    }

                    let delay_nanos = if !first && header.log_time > previous_log_time {
                        header.log_time.saturating_sub(previous_log_time)
                    } else {
                        0
                    };
                    previous_log_time = header.log_time;
                    first = false;

                    let item = PlaybackItem {
                        stream: &channel.stream,
                        bytes: data,
                        delay_nanos,
                        log_time_nanos: header.log_time,
                    };
                    delivered = delivered.saturating_add(1);
                    if !on_item(&item) {
                        break;
                    }
                }
                Ok(IndexedReadEvent::ReadChunkRequest { offset, length }) => {
                    current_chunk = Some(offset);
                    let Ok(start) = usize::try_from(offset) else {
                        let message = "the chunk offset exceeds this platform's address space";
                        return Err(read_failure(&self.source, Some(offset), message));
                    };
                    let Some(end) = start.checked_add(length) else {
                        let message = "the chunk length overflows this platform's address space";
                        return Err(read_failure(&self.source, Some(offset), message));
                    };
                    let Some(chunk) = storage.get(start..end) else {
                        let message = "the index requested a chunk outside the recording";
                        return Err(read_failure(&self.source, Some(offset), message));
                    };
                    reader
                        .insert_chunk_record_data(offset, chunk)
                        .map_err(|error| read_failure(&self.source, Some(offset), error))?;
                }
                Err(error) => {
                    return Err(read_failure(&self.source, current_chunk, error));
                }
            }
        }

        Ok(Some(delivered))
    }

    /// Iterate over the requested UTC window in log-time order.
    /// Start is inclusive; end is exclusive. An unbounded range selects every message.
    ///
    /// Returns the number of messages delivered to `on_item`.
    /// # Errors
    /// Returns the read failure, or the open failure when the recording did not open.
    pub fn for_each_item<F>(
        &self,
        topics_filter: &[String],
        range: UtcRange,
        mut on_item: F,
    ) -> Result<usize, String>
    where
        F: FnMut(&PlaybackItem<'_>) -> bool,
    {
        if !self.opened {
            return Err(self.error.clone());
        }

        if let Some(delivered) = self.for_each_indexed_item(topics_filter, range, &mut on_item)? {
            return Ok(delivered);
        }

        let mut candidates = self.collect_candidate_messages(topics_filter, range)?;
        candidates.sort_by_key(|(log_time, _channel, _bytes)| *log_time);

        let mut delivered: usize = 0;
        let mut previous_log_time = 0u64;
        let mut first = true;

        for (log_time, channel, bytes) in candidates {
            let delay_nanos = if !first && log_time > previous_log_time {
                log_time.saturating_sub(previous_log_time)
            } else {
                0
            };
            previous_log_time = log_time;
            first = false;

            let item = PlaybackItem {
                stream: &channel.stream,
                bytes: &bytes,
                delay_nanos,
                log_time_nanos: log_time,
            };

            delivered = delivered.saturating_add(1);
            if !on_item(&item) {
                break;
            }
        }

        Ok(delivered)
    }
}
