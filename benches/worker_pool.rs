#![allow(
    missing_docs,
    clippy::all,
    clippy::pedantic,
    clippy::nursery,
    clippy::cargo
)]

//! WorkerPool throughput benchmark.
//!
//! Measures the same-stage parallel-consumer (WorkerPool scheme A) path that is
//! exercised by `also_partition_with`. This path is not covered by any other
//! benchmark suite, so it provides a baseline for changes that touch the
//! work-processor claim loop (e.g. `consumer_engine.rs`).
//!
//! Design notes:
//! - Each worker owns its own `CachePadded<AtomicI64>` counter; there is no
//!   shared atomic contention between handlers.
//! - The producer waits for the sum of per-worker counters to reach the
//!   per-iteration target before continuing.
//! - Worker counts 1/2/4/8: 1 worker is a negative control (no inter-worker
//!   contention), ≥2 workers exercise the CAS-claim path.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use badbatch::disruptor::{build_single_producer, BusySpinWaitStrategy};
use crossbeam_utils::CachePadded;

#[derive(Debug, Default, Clone, Copy)]
struct WorkEvent {
    value: i64,
    data: [i64; 4],
}

const EVENTS_PER_ITER: i64 = 10_000;
const BUFFER_SIZE: usize = 1024;
const ITERATION_TIMEOUT: Duration = Duration::from_secs(10);

fn make_handler(
    counter: &Arc<CachePadded<AtomicI64>>,
) -> impl FnMut(&mut WorkEvent, i64, bool) + Send + 'static {
    let counter = Arc::clone(counter);
    move |event: &mut WorkEvent, _sequence: i64, _end_of_batch: bool| {
        // Prevent the compiler from optimizing away the event entirely.
        std::hint::black_box(event.data[0]);
        event.value = std::hint::black_box(event.value);
        counter.fetch_add(1, Ordering::Relaxed);
    }
}

fn wait_for_counters(
    counters: &[Arc<CachePadded<AtomicI64>>],
    target: i64,
    deadline: Instant,
) -> bool {
    while Instant::now() < deadline {
        let sum: i64 = counters.iter().map(|c| c.load(Ordering::Relaxed)).sum();
        if sum >= target {
            return true;
        }
        std::hint::spin_loop();
    }
    false
}

fn worker_pool_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("worker_pool");
    group.measurement_time(Duration::from_secs(5));
    group.warm_up_time(Duration::from_secs(2));

    for worker_count in [1usize, 2, 4, 8] {
        let benchmark_id = BenchmarkId::new("busyspin_workers", worker_count);
        group.throughput(Throughput::Elements(EVENTS_PER_ITER as u64));

        group.bench_function(benchmark_id, |b| {
            let counters: Vec<Arc<CachePadded<AtomicI64>>> = (0..worker_count)
                .map(|_| Arc::new(CachePadded::new(AtomicI64::new(0))))
                .collect();

            let factory = || WorkEvent::default();
            let wait_strategy = BusySpinWaitStrategy::new();

            let mut builder = build_single_producer(BUFFER_SIZE, factory, wait_strategy)
                .handle_events_with(make_handler(&counters[0]));

            for i in 1..worker_count {
                builder = builder.also_partition_with(make_handler(&counters[i]));
            }

            let mut disruptor = builder.build();

            b.iter_custom(|iters| {
                let start = Instant::now();
                for _ in 0..iters {
                    let start_sum: i64 = counters
                        .iter()
                        .map(|c| c.load(Ordering::Relaxed))
                        .sum();
                    let target = start_sum + EVENTS_PER_ITER;

                    for i in 0..EVENTS_PER_ITER {
                        disruptor
                            .publish(|event| {
                                event.value = i;
                            })
                            .unwrap_or_else(|e| panic!("Failed to publish event {i}: {e:?}"));
                    }

                    let deadline = Instant::now() + ITERATION_TIMEOUT;
                    if !wait_for_counters(&counters, target, deadline) {
                        let actual: i64 = counters
                            .iter()
                            .map(|c| c.load(Ordering::Relaxed))
                            .sum();
                        panic!("Timeout waiting for {EVENTS_PER_ITER} events (target {target}, got {actual})");
                    }

                    let actual: i64 = counters
                        .iter()
                        .map(|c| c.load(Ordering::Relaxed))
                        .sum();
                    assert_eq!(
                        actual, target,
                        "WorkerPool workers must process each event exactly once"
                    );
                }
                start.elapsed()
            });

            let _ = disruptor.shutdown();
        });
    }

    group.finish();
}

criterion_group!(benches, worker_pool_throughput);
criterion_main!(benches);
