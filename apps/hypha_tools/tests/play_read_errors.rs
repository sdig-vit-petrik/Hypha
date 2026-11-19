//! `hypha play` stops on a read error and refuses a topic that cannot be published.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

#[allow(dead_code, reason = "each test binary uses part of the shared helpers")]
mod common;

use hypha_tools::player::Player;
use hypha_tools::utc_time::UtcRange;
use mcap::records::MessageHeader;
use mcap::{Compression, Summary, WriteOptions, Writer};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

const START: &str = "2026-10-08T12:00:00Z";
const DATA_TOPIC: &str = "/read_error_data";
/// Bytes of a zstd chunk record before its compressed records: opcode, length, the
/// message start and end times, the uncompressed size and CRC, the compression name
/// (`zstd`) and the records length.
const CHUNK_RECORD_HEADER: u64 = 1 + 8 + 8 + 8 + 8 + 4 + 4 + 4 + 8;
/// The longest `hypha play` may take to exit.
const EXIT_LIMIT: Duration = Duration::from_secs(20);

fn scratch(test: &str) -> PathBuf {
    std::env::temp_dir().join(format!("play_read_errors_{test}_{}.mcap", std::process::id()))
}

/// Six messages, each in its own zstd chunk, one millisecond apart on [`DATA_TOPIC`].
/// With `schema` the channel carries a `std_msgs/UInt8` definition; without it the
/// channel has no schema, so it has no MD5 and cannot be published.
fn write_chunked_recording(path: &Path, schema: bool) {
    let start = hypha_tools::utc_time::parse_utc_nanos(START).expect("the UTC start");
    let options = WriteOptions::new()
        .profile("ros1")
        .use_chunks(true)
        .compression(Some(Compression::Zstd))
        .chunk_size(Some(1));
    let mut writer = Writer::with_options(File::create(path).expect("the recording file"), options)
        .expect("a writer");
    let (schema_id, metadata) = if schema {
        let schema_id =
            writer.add_schema("std_msgs/UInt8", "ros1msg", b"uint8 data\n").expect("schema");
        let md5 = hypha_node::sdk::md5::md5_from_definition("std_msgs/UInt8", "uint8 data\n")
            .expect("MD5");
        (schema_id, BTreeMap::from([("md5sum".to_string(), md5)]))
    } else {
        (0, BTreeMap::new())
    };
    let channel = writer.add_channel(schema_id, DATA_TOPIC, "ros1", &metadata).expect("channel");
    for sequence in 0_u32..6 {
        let offset = u64::from(sequence).checked_mul(1_000_000).expect("a short offset");
        let time = start.checked_add(offset).expect("a time in the window");
        let header =
            MessageHeader { channel_id: channel, sequence, log_time: time, publish_time: time };
        writer
            .write_to_known_channel(&header, &[u8::try_from(sequence).expect("small")])
            .expect("message");
    }
    writer.finish().expect("finish");
    drop(writer);
}

/// Overwrites the compressed records of the chunk that holds message `message` (zero
/// based) with bytes that are not a zstd frame, so the reader fails when it reaches
/// that chunk. The recording's first chunk holds only the schema and the channel, so
/// message `m` is in chunk index `m + 1`. Returns the file offset of the chunk's data,
/// which is the offset the reader reports.
fn corrupt_chunk_of_message(path: &Path, message: usize) -> u64 {
    let mut bytes = std::fs::read(path).expect("the recording");
    let summary = Summary::read(&bytes).expect("a readable summary").expect("a summary");
    let chunk_number = message.checked_add(1).expect("a small message number");
    let chunk = summary.chunk_indexes.get(chunk_number).expect("the message's chunk").clone();
    let data_start = chunk.chunk_start_offset.checked_add(CHUNK_RECORD_HEADER).expect("a chunk");
    let data_end = data_start.checked_add(chunk.compressed_size).expect("a chunk");
    for position in data_start..data_end {
        let index = usize::try_from(position).expect("an offset in the file");
        *bytes.get_mut(index).expect("an offset in the file") = 0xFF;
    }
    std::fs::write(path, &bytes).expect("write the corrupted recording");
    data_start
}

#[test]
fn a_corrupt_chunk_stops_the_replay_at_that_chunk_and_names_the_file() {
    let path = scratch("corrupt_chunk");
    write_chunked_recording(&path, true);
    let data_offset = corrupt_chunk_of_message(&path, 2);

    let player = Player::new(&path);
    assert!(player.is_open(), "the summary is intact: {}", player.open_error());
    let mut delivered_times = Vec::new();
    let result = player.for_each_item(&[], UtcRange::default(), |item| {
        delivered_times.push(item.log_time_nanos);
        true
    });
    std::fs::remove_file(&path).expect("remove the recording");

    let error = result.expect_err("the corrupt chunk must stop the replay");
    assert_eq!(delivered_times.len(), 2, "the messages before the corrupt chunk go out");
    assert!(error.contains(&*path.display().to_string()), "names the file: {error}");
    assert!(error.contains(&format!("chunk data at offset {data_offset}")), "{error}");
}

#[test]
fn a_truncated_recording_does_not_open_and_names_the_file() {
    let path = scratch("truncated");
    write_chunked_recording(&path, true);
    let bytes = std::fs::read(&path).expect("the recording");
    std::fs::write(&path, &bytes[..bytes.len() / 2]).expect("truncate the recording");

    let player = Player::new(&path);
    std::fs::remove_file(&path).expect("remove the recording");

    assert!(!player.is_open(), "a truncated recording must not replay");
    assert!(
        player.open_error().contains(&*path.display().to_string()),
        "the open error names the file: {}",
        player.open_error()
    );
}

/// Starts `hypha play` on `path` at maximum rate, with stdout and stderr piped.
fn spawn_player(path: &Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_hypha"))
        .arg("play")
        .arg("--input")
        .arg(path)
        .args(["--start-utc", START, "--rate", "max"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("hypha_play")
}

/// Waits for the child to exit, and kills it when it is still running after [`EXIT_LIMIT`].
fn wait_for_exit(child: &mut Child) -> ExitStatus {
    let began = Instant::now();
    loop {
        if let Some(status) = child.try_wait().expect("the child status") {
            return status;
        }
        if began.elapsed() >= EXIT_LIMIT {
            let _ = child.kill();
            let _ = child.wait();
            panic!("hypha_play did not exit within {EXIT_LIMIT:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn read_pipe(pipe: Option<impl Read>) -> String {
    let mut text = String::new();
    if let Some(mut pipe) = pipe {
        pipe.read_to_string(&mut text).expect("the child's output");
    }
    text
}

#[test]
fn an_unpublishable_topic_refuses_the_start() {
    let path = scratch("unpublishable");
    write_chunked_recording(&path, false);

    let mut child = spawn_player(&path);
    let status = wait_for_exit(&mut child);
    let stdout = read_pipe(child.stdout.take());
    let stderr = read_pipe(child.stderr.take());
    std::fs::remove_file(&path).expect("remove the recording");

    assert_eq!(status.code(), Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert!(
        stdout.contains(&format!(
            "hypha play: '{DATA_TOPIC}' cannot be published: no resolvable md5"
        )),
        "the line names the topic: {stdout}"
    );
    assert!(stdout.contains("done, 0 message(s) published"), "no message goes out: {stdout}");
    assert!(stderr.contains("refusing to start"), "the refusal is the error: {stderr}");
}

#[test]
fn a_corrupt_chunk_exits_one_after_the_messages_before_it() {
    let path = scratch("corrupt_binary");
    write_chunked_recording(&path, true);
    corrupt_chunk_of_message(&path, 2);

    let mut child = spawn_player(&path);
    let status = wait_for_exit(&mut child);
    let stdout = read_pipe(child.stdout.take());
    let stderr = read_pipe(child.stderr.take());
    std::fs::remove_file(&path).expect("remove the recording");

    assert_eq!(status.code(), Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert!(stdout.contains("published 2 messages"), "the messages before it: {stdout}");
    assert!(stderr.contains("chunk data at offset"), "the error names the chunk: {stderr}");
}
