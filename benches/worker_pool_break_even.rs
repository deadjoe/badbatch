#![allow(
    missing_docs,
    clippy::all,
    clippy::pedantic,
    clippy::nursery,
    clippy::cargo
)]

//! WorkerPool break-even benchmark.
//!
//! Measures the handler-cost inflection point at which WorkerPool scheme A
//! (`also_partition_with`) stops being a net loss compared to a single worker.
//!
//! Design notes:
//! - Seven handler-cost tiers are scanned at runtime within a single binary,
//!   avoiding cross-build layout confounds.
//! - Each tier is self-calibrated inside the bench binary: the isolated,
//!   single-threaded handler cost is measured and reported as ns/event.
//! - Worker counts 1/2/4/8 are tested for both WorkerPool and fan-out arms;
//!   fan-out serves only as a control to isolate shared-claim contention and is
//!   not a direct throughput comparison (each fan-out consumer sees every event).
//! - The 8-worker point is reported only as a reference value because this host
//!   (Mac16,11: 10 P-core + 4 E-core) risks heterogeneous scheduling with 9
//!   busy-spin threads (8 workers + 1 producer).

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

const BUFFER_SIZE: usize = 1024;
const ITERATION_TIMEOUT: Duration = Duration::from_secs(30);

// (tier label, workload iterations, events per benchmark iteration)
const TIERS: &[(&str, u64, i64)] = &[
    ("trivial", 0, 10_000),
    ("050ns", 40, 10_000),
    ("100ns", 65, 10_000),
    ("200ns", 220, 5_000),
    ("400ns", 440, 5_000),
    ("800ns", 860, 2_500),
    ("010us", 11_000, 1_000),
];

const WORKER_COUNTS: &[usize] = &[1, 2, 4, 8];

const SELF_CALIB_REPEATS: u64 = 500_000;
const SELF_CALIB_RUNS: usize = 5;

#[inline(never)]
fn run_workload(iterations: u64, seed: u64) -> u64 {
    let mut acc = seed;
    for _ in 0..iterations {
        acc = acc.wrapping_mul(0x9E3779B97F4A7C15);
        acc = acc.wrapping_add(0x123456789ABCDEF);
        acc = std::hint::black_box(acc);
    }
    acc
}

fn make_worker_handler(
    workload_iterations: u64,
    counter: &Arc<CachePadded<AtomicI64>>,
) -> impl FnMut(&mut WorkEvent, i64, bool) + Send + 'static {
    let counter = Arc::clone(counter);
    move |event: &mut WorkEvent, _sequence: i64, _end_of_batch: bool| {
        std::hint::black_box(event.data[0]);
        let seed = std::hint::black_box(event.value) as u64;
        let result = run_workload(workload_iterations, seed);
        event.value = result as i64;
        counter.fetch_add(1, Ordering::Relaxed);
    }
}

fn make_fanout_handler(
    workload_iterations: u64,
    counter: &Arc<CachePadded<AtomicI64>>,
) -> impl FnMut(&WorkEvent, i64, bool) + Send + 'static {
    let counter = Arc::clone(counter);
    move |event: &WorkEvent, _sequence: i64, _end_of_batch: bool| {
        std::hint::black_box(event.data[0]);
        let seed = std::hint::black_box(event.value) as u64;
        let result = run_workload(workload_iterations, seed);
        std::hint::black_box(result);
        counter.fetch_add(1, Ordering::Relaxed);
    }
}

fn median_min_max(values: &mut [f64]) -> (f64, f64, f64) {
    values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let min = values[0];
    let max = values[values.len() - 1];
    let median = if values.len() % 2 == 0 {
        (values[values.len() / 2 - 1] + values[values.len() / 2]) / 2.0
    } else {
        values[values.len() / 2]
    };
    (median, min, max)
}

fn self_calibrate(workload_iterations: u64) -> (f64, f64, f64) {
    let mut values = Vec::with_capacity(SELF_CALIB_RUNS);

    for _ in 0..SELF_CALIB_RUNS {
        let counter = Arc::new(CachePadded::new(AtomicI64::new(0)));
        let mut handler = make_worker_handler(workload_iterations, &counter);
        let mut event = WorkEvent::default();

        // Warm-up: exercise the handler and the branch predictor.
        for i in 0..SELF_CALIB_REPEATS {
            let seq = i as i64;
            event.value = seq;
            handler(&mut event, seq, false);
        }

        // Measurement: fresh handler and counter to avoid warm-up state artifacts.
        let counter = Arc::new(CachePadded::new(AtomicI64::new(0)));
        let mut handler = make_worker_handler(workload_iterations, &counter);
        let mut event = WorkEvent::default();

        let start = Instant::now();
        for i in 0..SELF_CALIB_REPEATS {
            let seq = i as i64;
            event.value = seq;
            handler(&mut event, seq, false);
        }
        let elapsed = start.elapsed();

        let ns_per_call = elapsed.as_nanos() as f64 / SELF_CALIB_REPEATS as f64;
        values.push(ns_per_call);
    }

    median_min_max(&mut values)
}

/// Polls the per-worker counters infrequently while spinning.
///
/// Reading `Instant::now()` and N remote cache lines every spin iteration adds
/// a harmonic overhead that grows with the worker count and inflates the
/// measured collapse. We check progress only every `POLL_INTERVAL` spins to
/// keep the producer's interference negligible.
const POLL_INTERVAL: usize = 64;

fn wait_for_counters(
    counters: &[Arc<CachePadded<AtomicI64>>],
    target: i64,
    deadline: Instant,
) -> bool {
    let mut spins: usize = 0;
    loop {
        if spins % POLL_INTERVAL == 0 {
            let sum: i64 = counters.iter().map(|c| c.load(Ordering::Relaxed)).sum();
            if sum >= target {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
        } else {
            std::hint::spin_loop();
        }
        spins = spins.wrapping_add(1);
    }
}

fn worker_pool_benchmark(c: &mut Criterion) {
    for &(tier_label, workload_iterations, events_per_iter) in TIERS {
        let (cal_median, cal_min, cal_max) = self_calibrate(workload_iterations);
        eprintln!(
            "[worker_pool_break_even] self-calibration: tier={} iter={} median={:.2}ns min={:.2}ns max={:.2}ns range={:.2}ns",
            tier_label,
            workload_iterations,
            cal_median,
            cal_min,
            cal_max,
            cal_max - cal_min
        );

        let mut group = c.benchmark_group(format!("wp_break_even/{tier_label}"));
        group.throughput(Throughput::Elements(events_per_iter as u64));
        group.measurement_time(Duration::from_secs(5));
        group.warm_up_time(Duration::from_secs(1));

        for &worker_count in WORKER_COUNTS {
            // WorkerPool arm.
            group.bench_function(
                BenchmarkId::new("worker_pool", worker_count),
                |b| {
                    let counters: Vec<Arc<CachePadded<AtomicI64>>> = (0..worker_count)
                        .map(|_| Arc::new(CachePadded::new(AtomicI64::new(0))))
                        .collect();

                    let factory = || WorkEvent::default();
                    let wait_strategy = BusySpinWaitStrategy::new();

                    let mut builder = build_single_producer(BUFFER_SIZE, factory, wait_strategy)
                        .handle_events_with(make_worker_handler(workload_iterations, &counters[0]));

                    for i in 1..worker_count {
                        builder = builder.also_partition_with(make_worker_handler(
                            workload_iterations,
                            &counters[i],
                        ));
                    }

                    let mut disruptor = builder.build();

                    b.iter_custom(|iters| {
                        let start = Instant::now();
                        for _ in 0..iters {
                            let start_sum: i64 = counters
                                .iter()
                                .map(|c| c.load(Ordering::Relaxed))
                                .sum();
                            let target = start_sum + events_per_iter;

                            for i in 0..events_per_iter {
                                disruptor
                                    .publish(|event| {
                                        event.value = i;
                                    })
                                    .unwrap_or_else(|e| {
                                        panic!("Failed to publish event {i}: {e:?}")
                                    });
                            }

                            let deadline = Instant::now() + ITERATION_TIMEOUT;
                            if !wait_for_counters(&counters, target, deadline) {
                                let actual: i64 = counters
                                    .iter()
                                    .map(|c| c.load(Ordering::Relaxed))
                                    .sum();
                                panic!(
                                    "Timeout waiting for {events_per_iter} events (target {target}, got {actual})"
                                );
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
                },
            );

            // Fan-out arm (only for >=2 consumers; 1-consumer fan-out is identical
            // to a single worker and does not isolate shared-claim effects).
            if worker_count >= 2 {
                group.bench_function(BenchmarkId::new("fan_out", worker_count), |b| {
                    let counters: Vec<Arc<CachePadded<AtomicI64>>> = (0..worker_count)
                        .map(|_| Arc::new(CachePadded::new(AtomicI64::new(0))))
                        .collect();

                    let factory = || WorkEvent::default();
                    let wait_strategy = BusySpinWaitStrategy::new();

                    let mut builder = build_single_producer(BUFFER_SIZE, factory, wait_strategy)
                        .fan_out_events_with(make_fanout_handler(
                            workload_iterations,
                            &counters[0],
                        ));

                    for i in 1..worker_count {
                        builder = builder.fan_out_events_with(make_fanout_handler(
                            workload_iterations,
                            &counters[i],
                        ));
                    }

                    let mut disruptor = builder.build();

                    b.iter_custom(|iters| {
                        let start = Instant::now();
                        for _ in 0..iters {
                            let start_sum: i64 =
                                counters.iter().map(|c| c.load(Ordering::Relaxed)).sum();
                            let target = start_sum + (worker_count as i64 * events_per_iter);

                            for i in 0..events_per_iter {
                                disruptor
                                    .publish(|event| {
                                        event.value = i;
                                    })
                                    .unwrap_or_else(|e| {
                                        panic!("Failed to publish event {i}: {e:?}")
                                    });
                            }

                            let deadline = Instant::now() + ITERATION_TIMEOUT;
                            if !wait_for_counters(&counters, target, deadline) {
                                let actual: i64 =
                                    counters.iter().map(|c| c.load(Ordering::Relaxed)).sum();
                                panic!(
                                    "Timeout waiting for {} events (target {target}, got {actual})",
                                    worker_count as i64 * events_per_iter
                                );
                            }

                            let actual: i64 =
                                counters.iter().map(|c| c.load(Ordering::Relaxed)).sum();
                            assert_eq!(
                                actual, target,
                                "Fan-out handlers must each process every event"
                            );
                        }
                        start.elapsed()
                    });

                    let _ = disruptor.shutdown();
                });
            }
        }

        group.finish();
    }
}

criterion_group!(benches, worker_pool_benchmark);
criterion_main!(benches);
