//! Cost of the integrity code (`seal`, one pass of CRC-32C over the payload), with
//! `memcpy` of the same bytes as the yardstick.
//!
//! - warm: the same buffer every time, in cache up to the cache's size;
//! - cold: the next buffer of an arena much larger than the last-level cache, so the
//!   bytes come from memory, as a camera frame does.
//!
//! Sizes 64 bytes (a small message) to 15 MiB (a camera frame). Run with
//! `cargo bench -p hypha_wire --bench integrity`. Not a CI gate.
#![expect(clippy::expect_used, reason = "a size that does not fit u64 stops the run")]

use std::hint::black_box;

use criterion::measurement::WallTime;
use criterion::{BenchmarkGroup, Criterion, Throughput, criterion_group, criterion_main};
use hypha_wire::frame::{FrameMetadata, TopicCode, seal};

const KIB: usize = 1024;
const MIB: usize = 1024 * KIB;
const SIZES: [(usize, &str); 6] = [
    (64, "64B"),
    (4 * KIB, "4KiB"),
    (64 * KIB, "64KiB"),
    (MIB, "1MiB"),
    (5 * MIB, "5MiB"),
    (15 * MIB, "15MiB"),
];
/// Far larger than any last-level cache of the machines measured.
const ARENA: usize = 512 * MIB;

const METADATA: FrameMetadata = FrameMetadata {
    schema_hash: 1,
    publisher_id: 2,
    sequence: 3,
    transport_sent_nanos: 4,
    framework_sent_nanos: 5,
};

/// The buffers of `size` bytes in `arena`, taken in turn.
struct Rotation<'a> {
    buffers: Vec<&'a [u8]>,
    next: usize,
}

impl<'a> Rotation<'a> {
    fn new(arena: &'a [u8], size: usize) -> Self {
        Self { buffers: arena.chunks_exact(size).collect(), next: 0 }
    }

    fn next(&mut self) -> &'a [u8] {
        self.next = self.next.wrapping_add(1).checked_rem(self.buffers.len()).unwrap_or(0);
        self.buffers.get(self.next).copied().unwrap_or_default()
    }
}

fn throughput(group: &mut BenchmarkGroup<'_, WallTime>, size: usize) {
    group.throughput(Throughput::Bytes(u64::try_from(size).expect("a size fits u64")));
}

fn integrity(c: &mut Criterion) {
    let topic = TopicCode::of("/camera/image_raw");
    let arena: Vec<u8> = (0..ARENA).map(|index| index.to_le_bytes()[0] ^ 0x5A).collect();
    let mut destination = vec![0_u8; 15 * MIB];
    for (size, name) in SIZES {
        let mut group = c.benchmark_group(format!("integrity/{name}"));
        group.sample_size(20);
        throughput(&mut group, size);
        let warm = arena.get(..size).unwrap_or_default();
        let target = destination.get_mut(..size).unwrap_or_default();
        group.bench_function("seal/warm", |b| {
            b.iter(|| black_box(seal(topic, METADATA, 0, black_box(warm))));
        });
        group.bench_function("memcpy/warm", |b| {
            b.iter(|| {
                target.copy_from_slice(black_box(warm));
                black_box(&target);
            });
        });
        let mut cold = Rotation::new(&arena, size);
        group.bench_function("seal/cold", |b| {
            b.iter(|| black_box(seal(topic, METADATA, 0, black_box(cold.next()))));
        });
        group.bench_function("memcpy/cold", |b| {
            b.iter(|| {
                target.copy_from_slice(black_box(cold.next()));
                black_box(&target);
            });
        });
        group.finish();
    }
}

criterion_group!(benches, integrity);
criterion_main!(benches);
