//! Encode and decode cost of the ROS 1 primitive codec.
//!
//! - small: a `std_msgs/Header` (`seq`, `stamp`, `frame_id`), 25 bytes;
//! - large: a header and a `float32[]` of 16384 elements, 64 KiB of payload;
//! - frame: sealing and verifying the frame of the small message, the per-frame work of a
//!   publish and of a receive besides the transport.
//!
//! Run with `cargo bench -p hypha_wire --bench codec`. Not a CI gate.
#![expect(clippy::expect_used, reason = "a codec error in the fixed benchmark input stops the run")]

use std::hint::black_box;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use hypha_wire::codec::{Reader, Result};
use hypha_wire::slice_writer::SliceWriter;

const FRAME_ID: &str = "base_link";
/// The element count of the large message's `float32[]`.
const LARGE_ELEMENTS: usize = 16 * 1024;
/// `seq` (4), `stamp` (8), `frame_id` length (4) and its bytes.
const HEADER_BYTES: usize = 4 + 8 + 4 + FRAME_ID.len();
/// The header, the array length (4) and four bytes per element.
const LARGE_BYTES: usize = HEADER_BYTES + 4 + 4 * LARGE_ELEMENTS;

/// A decoded header: what a reader of the message keeps.
struct Header<'a> {
    seq: u32,
    stamp: (u32, u32),
    frame_id: &'a [u8],
}

fn write_header(writer: &mut SliceWriter<'_>, seq: u32) -> Result<()> {
    writer.write_u32(seq)?;
    writer.write_time(1_700_000_000, 500)?;
    writer.write_str(FRAME_ID)
}

fn read_header<'a>(reader: &mut Reader<'a>) -> Result<Header<'a>> {
    let seq = reader.read_u32()?;
    let stamp = reader.read_time()?;
    let frame_id = reader.read_string()?;
    Ok(Header { seq, stamp, frame_id })
}

fn write_large(writer: &mut SliceWriter<'_>, values: &[f32]) -> Result<()> {
    write_header(writer, 7)?;
    writer.write_array_len(values.len())?;
    values.iter().try_for_each(|value| writer.write_f32(*value))
}

/// Reads the large message and returns the sum of its elements, so no read is dead.
fn read_large(buffer: &[u8]) -> Result<f32> {
    let mut reader = Reader::new(buffer);
    let header = read_header(&mut reader)?;
    let count = reader.read_array_len_bounded(4)?;
    let mut sum = f32::from(u16::try_from(header.frame_id.len()).unwrap_or(u16::MAX));
    for _ in 0..count {
        sum += reader.read_f32()?;
    }
    Ok(sum)
}

fn small(c: &mut Criterion) {
    let mut group = c.benchmark_group("codec/small");
    group.throughput(Throughput::Bytes(u64::try_from(HEADER_BYTES).expect("a small size")));
    let mut buffer = [0_u8; HEADER_BYTES];
    group.bench_function("encode", |b| {
        b.iter(|| {
            let mut writer = SliceWriter::new(&mut buffer);
            write_header(&mut writer, black_box(42)).expect("the header fits");
            black_box(writer.written())
        });
    });
    write_header(&mut SliceWriter::new(&mut buffer), 42).expect("the header fits");
    group.bench_function("decode", |b| {
        b.iter(|| {
            let mut reader = Reader::new(black_box(&buffer));
            let header = read_header(&mut reader).expect("a valid header");
            black_box((header.seq, header.stamp, header.frame_id.len()))
        });
    });
    group.finish();
}

fn large(c: &mut Criterion) {
    let mut group = c.benchmark_group("codec/large");
    group.throughput(Throughput::Bytes(u64::try_from(LARGE_BYTES).expect("a small size")));
    let values: Vec<f32> = (0..LARGE_ELEMENTS)
        .map(|index| f32::from(u16::try_from(index % 1000).unwrap_or(0)))
        .collect();
    let mut buffer = vec![0_u8; LARGE_BYTES];
    group.bench_function("encode", |b| {
        b.iter(|| {
            let mut writer = SliceWriter::new(&mut buffer);
            write_large(&mut writer, black_box(&values)).expect("the message fits");
            black_box(writer.written())
        });
    });
    write_large(&mut SliceWriter::new(&mut buffer), &values).expect("the message fits");
    group.bench_function("decode", |b| {
        b.iter(|| black_box(read_large(black_box(&buffer)).expect("a valid message")));
    });
    group.finish();
}

fn frame(c: &mut Criterion) {
    use hypha_wire::frame::{FrameMetadata, TopicCode, seal, verify};
    let topic = TopicCode::of("/bench/frame");
    let metadata = FrameMetadata {
        schema_hash: 1,
        publisher_id: 2,
        sequence: 3,
        transport_sent_nanos: 4,
        framework_sent_nanos: 5,
    };
    let payload = [7_u8; HEADER_BYTES];
    let header = seal(topic, metadata, 0, &payload);
    let mut group = c.benchmark_group("frame");
    group.bench_function("seal", |b| {
        b.iter(|| seal(topic, black_box(metadata), 0, black_box(&payload)));
    });
    group.bench_function("verify", |b| {
        b.iter(|| verify(topic, black_box(&header), black_box(&payload)).is_ok());
    });
    group.finish();
}

criterion_group!(benches, small, large, frame);
criterion_main!(benches);
