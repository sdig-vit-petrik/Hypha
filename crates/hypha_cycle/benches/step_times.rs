//! The cost of counting one step: `StepTimes::record` alone and behind the lock the node
//! takes, and the cost of the summary a `~/state` message needs.
//!
//! Run with `cargo bench -p hypha_cycle --bench step_times`. Not a CI gate.

use std::hint::black_box;
use std::sync::Mutex;

use criterion::{Criterion, criterion_group, criterion_main};
use hypha_cycle::StepTimes;

/// Step durations between 20 us and 2 ms, cycling through a fixed list so the buckets vary.
const DURATIONS: [u64; 8] =
    [20_000, 90_000, 150_000, 400_000, 600_000, 900_000, 1_400_000, 2_000_000];

fn record(criterion: &mut Criterion) {
    let mut times = StepTimes::default();
    let mut now = 0_u64;
    let mut durations = DURATIONS.iter().copied().cycle();
    criterion.bench_function("record", |bench| {
        bench.iter(|| {
            now = now.wrapping_add(1_000_000);
            let duration = durations.next().unwrap_or(0);
            times.record(black_box(duration), black_box(now));
        });
    });

    let locked = Mutex::new(StepTimes::default());
    criterion.bench_function("record behind a mutex", |bench| {
        bench.iter(|| {
            now = now.wrapping_add(1_000_000);
            let duration = durations.next().unwrap_or(0);
            locked.lock().map(|mut guard| guard.record(black_box(duration), black_box(now))).ok();
        });
    });

    criterion.bench_function("summary", |bench| bench.iter(|| black_box(times.summary())));
}

criterion_group!(benches, record);
criterion_main!(benches);
