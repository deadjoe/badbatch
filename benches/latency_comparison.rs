#![allow(
    missing_docs,
    clippy::all,
    clippy::pedantic,
    clippy::nursery,
    clippy::cargo
)]

//! Latency Comparison Benchmarks
//!
//! This benchmark suite compares the latency characteristics of the BadBatch
//! Disruptor against other concurrency primitives like channels.

use criterion::measurement::WallTime;
use criterion::{
    criterion_group, criterion_main, BenchmarkGroup, BenchmarkId, Criterion, Throughput,
};
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

use badbatch::disruptor::{
    event_translator::ClosureEventTranslator, BusySpinWaitStrategy, DefaultEventFactory, Disruptor,
    EventHandler, ProducerType, Result as DisruptorResult,
};

// Benchmark configuration constants
const BUFFER_SIZE: usize = 1024;
const BATCH_SIZE: usize = 500; // Batch size per Criterion iteration

#[derive(Debug, Default, Clone, Copy)]
struct LatencyEvent {
    id: i64,
    send_time: u64,
}

/// Shared state for recording per-event latencies.
struct LatencyState {
    latencies: Arc<Vec<AtomicI64>>,
    write_index: Arc<AtomicUsize>,
    processed: Arc<AtomicUsize>,
}

impl LatencyState {
    fn new(capacity: usize) -> Self {
        let latencies: Vec<AtomicI64> = (0..capacity).map(|_| AtomicI64::new(0)).collect();

        Self {
            latencies: Arc::new(latencies),
            write_index: Arc::new(AtomicUsize::new(0)),
            processed: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn latencies(&self) -> Arc<Vec<AtomicI64>> {
        Arc::clone(&self.latencies)
    }

    fn write_index(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.write_index)
    }

    fn processed(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.processed)
    }

    fn capacity(&self) -> usize {
        self.latencies.len()
    }
}

/// Disruptor event handler that records end-to-end publish→on_event latency.
struct LatencyHandler {
    state: LatencyState,
}

impl LatencyHandler {
    fn new(state: LatencyState) -> Self {
        Self { state }
    }
}

impl EventHandler<LatencyEvent> for LatencyHandler {
    fn on_event(
        &mut self,
        event: &mut LatencyEvent,
        _sequence: i64,
        _end_of_batch: bool,
    ) -> DisruptorResult<()> {
        let process_time = get_timestamp_nanos();
        let latency = process_time - event.send_time;

        let capacity = self.state.capacity();
        if capacity == 0 {
            return Ok(());
        }

        let index = self.state.write_index.fetch_add(1, Ordering::Relaxed) % capacity;
        self.state.latencies[index].store(latency as i64, Ordering::Relaxed);
        self.state.processed.fetch_add(1, Ordering::Release);
        Ok(())
    }
}

/// Get current timestamp in nanoseconds using high-resolution timer
fn get_timestamp_nanos() -> u64 {
    // Use Instant for high-resolution monotonic timing instead of SystemTime
    // Convert to nanoseconds from a fixed reference point
    static START_TIME: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let start = START_TIME.get_or_init(std::time::Instant::now);
    start.elapsed().as_nanos() as u64
}

/// Calculate latency statistics
fn calculate_latency_stats(latencies: &[i64]) -> (f64, f64, f64, f64, f64) {
    let mut sorted_latencies = latencies.to_vec();
    sorted_latencies.sort_unstable();

    let len = sorted_latencies.len();
    if len == 0 {
        return (0.0, 0.0, 0.0, 0.0, 0.0);
    }

    let sum: i64 = sorted_latencies.iter().sum();
    let mean = sum as f64 / len as f64;

    let median = if len % 2 == 0 {
        (sorted_latencies[len / 2 - 1] + sorted_latencies[len / 2]) as f64 / 2.0
    } else {
        sorted_latencies[len / 2] as f64
    };

    let p95_index = (95.0 * len as f64 / 100.0).floor() as usize;
    let p95 = sorted_latencies[p95_index.min(len - 1)] as f64;

    let p99_index = (99.0 * len as f64 / 100.0).floor() as usize;
    let p99 = sorted_latencies[p99_index.min(len - 1)] as f64;

    let max = sorted_latencies[len - 1] as f64;

    (mean, median, p95, p99, max)
}

fn wait_for_processed(processed: &AtomicUsize, target: usize) {
    while processed.load(Ordering::Acquire) < target {
        std::hint::spin_loop();
    }
}

fn collect_latencies(state: &LatencyState, expected: usize) -> Vec<i64> {
    let count = state.write_index.load(Ordering::Acquire).min(expected);
    state
        .latencies
        .iter()
        .take(count)
        .map(|lat| lat.load(Ordering::Acquire))
        .filter(|&lat| lat > 0)
        .collect()
}

fn print_latency_stats(label: &str, latencies: &[i64]) {
    let (mean, median, p95, p99, max) = calculate_latency_stats(latencies);
    println!("\n{label} Latency Statistics (nanoseconds):");
    println!("  Mean: {mean:.2}");
    println!("  Median: {median:.2}");
    println!("  95th percentile: {p95:.2}");
    println!("  99th percentile: {p99:.2}");
    println!("  Max: {max:.2}");
}

/// Benchmark Disruptor latency with BusySpinWaitStrategy
fn benchmark_disruptor_latency(group: &mut BenchmarkGroup<WallTime>) {
    let factory = DefaultEventFactory::<LatencyEvent>::new();
    let state = LatencyState::new(BATCH_SIZE);
    let latencies = state.latencies();
    let processed = state.processed();
    let write_index = state.write_index();
    let handler = LatencyHandler::new(state);

    let mut disruptor = Disruptor::new(
        factory,
        BUFFER_SIZE,
        ProducerType::Single,
        Box::new(BusySpinWaitStrategy::new()),
    )
    .unwrap()
    .handle_events_with(handler)
    .build();

    disruptor.start().unwrap();

    let benchmark_id = BenchmarkId::new("Disruptor", "BusySpin");

    group.throughput(Throughput::Elements(BATCH_SIZE as u64));
    group.bench_function(benchmark_id, |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let start_processed = processed.load(Ordering::Acquire);
                let target = start_processed + BATCH_SIZE;

                let iter_start = Instant::now();
                for sample in 0..BATCH_SIZE {
                    let send_time = get_timestamp_nanos();
                    let sample_id = sample as i64;
                    disruptor
                        .publish_event(ClosureEventTranslator::new(
                            move |event: &mut LatencyEvent, _seq: i64| {
                                event.id = std::hint::black_box(sample_id);
                                event.send_time = send_time;
                            },
                        ))
                        .unwrap();
                }

                wait_for_processed(&processed, target);

                total += iter_start.elapsed();
            }
            total
        });
    });

    // Collect a fresh latency distribution sample outside the timed section.
    write_index.store(0, Ordering::Relaxed);
    let start_processed = processed.load(Ordering::Acquire);
    let target = start_processed + BATCH_SIZE;
    for sample in 0..BATCH_SIZE {
        let send_time = get_timestamp_nanos();
        let sample_id = sample as i64;
        disruptor
            .publish_event(ClosureEventTranslator::new(
                move |event: &mut LatencyEvent, _seq: i64| {
                    event.id = std::hint::black_box(sample_id);
                    event.send_time = send_time;
                },
            ))
            .unwrap();
    }
    wait_for_processed(&processed, target);
    let collected_latencies = collect_latencies(
        &LatencyState {
            latencies,
            write_index,
            processed,
        },
        BATCH_SIZE,
    );
    print_latency_stats("Disruptor", &collected_latencies);

    disruptor.shutdown().unwrap();
}

/// Benchmark std::sync::mpsc::sync_channel latency (steady-state, no per-iter thread spawn).
fn benchmark_std_mpsc_latency(group: &mut BenchmarkGroup<WallTime>) {
    let benchmark_id = BenchmarkId::new("StdMpsc", "sync_channel");

    let state = LatencyState::new(BATCH_SIZE);
    let processed = state.processed();
    let write_index = state.write_index();

    let (sender, receiver) = mpsc::sync_channel::<(i64, u64)>(BUFFER_SIZE);
    let latencies = state.latencies();

    let receiver_handle = thread::spawn(move || {
        for (_id, send_time) in receiver.iter() {
            let process_time = get_timestamp_nanos();
            let latency = process_time - send_time;

            let index = write_index.fetch_add(1, Ordering::Relaxed) % BATCH_SIZE;
            latencies[index].store(latency as i64, Ordering::Relaxed);
            processed.fetch_add(1, Ordering::Release);
        }
    });

    group.throughput(Throughput::Elements(BATCH_SIZE as u64));
    group.bench_function(benchmark_id, |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let start_processed = state.processed.load(Ordering::Acquire);
                let target = start_processed + BATCH_SIZE;

                let iter_start = Instant::now();
                for i in 0..BATCH_SIZE {
                    let send_time = get_timestamp_nanos();
                    sender.send((i as i64, send_time)).unwrap();
                }
                wait_for_processed(&state.processed, target);
                total += iter_start.elapsed();
            }
            total
        })
    });

    // Collect a fresh latency distribution sample outside the timed section.
    state.write_index.store(0, Ordering::Relaxed);
    let start_processed = state.processed.load(Ordering::Acquire);
    let target = start_processed + BATCH_SIZE;
    for i in 0..BATCH_SIZE {
        let send_time = get_timestamp_nanos();
        sender.send((i as i64, send_time)).unwrap();
    }
    wait_for_processed(&state.processed, target);
    let collected_latencies = collect_latencies(&state, BATCH_SIZE);
    print_latency_stats("StdMpsc", &collected_latencies);

    drop(sender);
    receiver_handle.join().unwrap();
}

/// Benchmark crossbeam bounded channel latency (steady-state, no per-iter thread spawn).
fn benchmark_crossbeam_latency(group: &mut BenchmarkGroup<WallTime>) {
    let benchmark_id = BenchmarkId::new("Crossbeam", "bounded");

    let state = LatencyState::new(BATCH_SIZE);
    let processed = state.processed();
    let write_index = state.write_index();

    let (sender, receiver) = crossbeam::channel::bounded::<(i64, u64)>(BUFFER_SIZE);
    let latencies = state.latencies();

    let receiver_handle = thread::spawn(move || {
        while let Ok((_id, send_time)) = receiver.recv() {
            let process_time = get_timestamp_nanos();
            let latency = process_time - send_time;

            let index = write_index.fetch_add(1, Ordering::Relaxed) % BATCH_SIZE;
            latencies[index].store(latency as i64, Ordering::Relaxed);
            processed.fetch_add(1, Ordering::Release);
        }
    });

    group.throughput(Throughput::Elements(BATCH_SIZE as u64));
    group.bench_function(benchmark_id, |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let start_processed = state.processed.load(Ordering::Acquire);
                let target = start_processed + BATCH_SIZE;

                let iter_start = Instant::now();
                for i in 0..BATCH_SIZE {
                    let send_time = get_timestamp_nanos();
                    sender.send((i as i64, send_time)).unwrap();
                }
                wait_for_processed(&state.processed, target);
                total += iter_start.elapsed();
            }
            total
        })
    });

    // Collect a fresh latency distribution sample outside the timed section.
    state.write_index.store(0, Ordering::Relaxed);
    let start_processed = state.processed.load(Ordering::Acquire);
    let target = start_processed + BATCH_SIZE;
    for i in 0..BATCH_SIZE {
        let send_time = get_timestamp_nanos();
        sender.send((i as i64, send_time)).unwrap();
    }
    wait_for_processed(&state.processed, target);
    let collected_latencies = collect_latencies(&state, BATCH_SIZE);
    print_latency_stats("Crossbeam", &collected_latencies);

    drop(sender);
    receiver_handle.join().unwrap();
}

/// Main latency comparison benchmark function
pub fn latency_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("Latency");

    // Configure benchmark group for latency measurement
    group.measurement_time(Duration::from_secs(10));
    group.warm_up_time(Duration::from_secs(3));
    group.sample_size(20); // Smaller sample size for latency tests

    benchmark_disruptor_latency(&mut group);
    benchmark_std_mpsc_latency(&mut group);
    benchmark_crossbeam_latency(&mut group);

    group.finish();
}

criterion_group!(latency, latency_benchmark);
criterion_main!(latency);
