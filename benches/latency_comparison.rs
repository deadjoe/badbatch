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

use criterion::Throughput;
use criterion::{criterion_group, criterion_main, BenchmarkGroup, BenchmarkId, Criterion};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use badbatch::disruptor::{
    event_translator::ClosureEventTranslator, BusySpinWaitStrategy, DefaultEventFactory, Disruptor,
    EventHandler, ProducerType, Result as DisruptorResult,
};

// Benchmark configuration constants
const BUFFER_SIZE: usize = 1024;
const SAMPLE_COUNT: usize = 500; // Reduced from 1000 to improve speed

#[derive(Debug, Default, Clone, Copy)]
struct LatencyEvent {
    id: i64,
    send_time: u64,
    #[allow(dead_code)]
    process_time: u64,
}

/// Event handler that measures processing latency
struct LatencyHandler {
    latencies: Arc<Vec<AtomicI64>>,
    processed: Arc<AtomicI64>,
    write_index: Arc<AtomicI64>,
}

impl LatencyHandler {
    fn new(capacity: usize) -> Self {
        let latencies: Vec<AtomicI64> = (0..capacity).map(|_| AtomicI64::new(0)).collect();

        Self {
            latencies: Arc::new(latencies),
            processed: Arc::new(AtomicI64::new(0)),
            write_index: Arc::new(AtomicI64::new(0)),
        }
    }

    fn latencies(&self) -> Arc<Vec<AtomicI64>> {
        Arc::clone(&self.latencies)
    }

    fn processed_counter(&self) -> Arc<AtomicI64> {
        Arc::clone(&self.processed)
    }

    fn write_index(&self) -> Arc<AtomicI64> {
        Arc::clone(&self.write_index)
    }

    fn capacity(&self) -> i64 {
        self.latencies.len() as i64
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

        let capacity = self.capacity();
        if capacity == 0 {
            return Ok(());
        }

        let index = self.write_index.fetch_add(1, Ordering::Release) % capacity;
        if index >= 0 {
            self.latencies[index as usize].store(latency as i64, Ordering::Release);
        }

        self.processed.fetch_add(1, Ordering::Release);
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

/// Benchmark Disruptor latency with BusySpinWaitStrategy
fn benchmark_disruptor_latency(group: &mut BenchmarkGroup<criterion::measurement::WallTime>) {
    let factory = DefaultEventFactory::<LatencyEvent>::new();
    let handler = LatencyHandler::new(SAMPLE_COUNT);
    let latencies = handler.latencies();
    let processed = handler.processed_counter();
    let write_index = handler.write_index();

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

    group.throughput(Throughput::Elements(SAMPLE_COUNT as u64));
    group.bench_function(benchmark_id, |b| {
        b.iter_custom(|iters| {
            use std::time::Instant;
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                processed.store(0, Ordering::Release);
                write_index.store(0, Ordering::Release);
                for latency in latencies.iter() {
                    latency.store(0, Ordering::Release);
                }

                let iter_start = Instant::now();
                for sample in 0..SAMPLE_COUNT {
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

                while processed.load(Ordering::Acquire) < SAMPLE_COUNT as i64 {
                    std::hint::spin_loop();
                }

                total += iter_start.elapsed();
            }
            total
        });
    });

    // Print latency statistics
    let collected_latencies: Vec<i64> = latencies
        .iter()
        .map(|lat| lat.load(Ordering::Acquire))
        .filter(|&lat| lat > 0)
        .collect();

    let (mean, median, p95, p99, max) = calculate_latency_stats(&collected_latencies);

    println!("\nDisruptor Latency Statistics (nanoseconds):");
    println!("  Mean: {mean:.2}");
    println!("  Median: {median:.2}");
    println!("  95th percentile: {p95:.2}");
    println!("  99th percentile: {p99:.2}");
    println!("  Max: {max:.2}");

    disruptor.shutdown().unwrap();
}

/// Benchmark std::sync::mpsc channel latency
fn benchmark_mpsc_latency(group: &mut BenchmarkGroup<criterion::measurement::WallTime>) {
    let benchmark_id = BenchmarkId::new("MPSC", "Channel");

    group.bench_function(benchmark_id, |b| {
        b.iter(|| {
            // Create fresh channel and thread for single measurement
            let (sender, receiver) = mpsc::channel();
            let counter = Arc::new(AtomicI64::new(0));

            let counter_clone = counter.clone();
            let receiver_handle = thread::spawn(move || {
                if let Ok((_id, _send_time)) = receiver.recv() {
                    counter_clone.store(1, Ordering::Release);
                }
            });

            // Measure single event latency
            let _send_time = get_timestamp_nanos();
            if sender.send((std::hint::black_box(1), _send_time)).is_ok() {
                // Wait for the single event to be processed
                while counter.load(Ordering::Acquire) < 1 {
                    std::hint::spin_loop();
                }
            }

            // Close sender to signal receiver to stop
            drop(sender);
            receiver_handle.join().unwrap();

            std::hint::black_box(())
        })
    });

    // Note: Individual latency statistics are not printed for MPSC channel
    // because latencies are measured inside each iteration. The timing
    // information is captured by Criterion's measurement framework.
}

/// Benchmark crossbeam channel latency for comparison
fn benchmark_crossbeam_latency(group: &mut BenchmarkGroup<criterion::measurement::WallTime>) {
    let benchmark_id = BenchmarkId::new("Crossbeam", "Channel");

    group.bench_function(benchmark_id, |b| {
        b.iter(|| {
            // Create fresh channel and thread for single measurement
            let (sender, receiver) = crossbeam::channel::bounded(BUFFER_SIZE);
            let counter = Arc::new(AtomicI64::new(0));

            let counter_clone = counter.clone();
            let receiver_handle = thread::spawn(move || {
                if let Ok((_id, _send_time)) = receiver.recv() {
                    counter_clone.store(1, Ordering::Release);
                }
            });

            // Measure single event latency
            let _send_time = get_timestamp_nanos();
            if sender.send((std::hint::black_box(1), _send_time)).is_ok() {
                // Wait for the single event to be processed
                while counter.load(Ordering::Acquire) < 1 {
                    std::hint::spin_loop();
                }
            }

            // Close sender to signal receiver to stop
            drop(sender);
            receiver_handle.join().unwrap();

            std::hint::black_box(())
        })
    });

    // Note: Individual latency statistics are not printed for Crossbeam channel
    // because latencies are measured inside each iteration. The timing
    // information is captured by Criterion's measurement framework.
}

/// Main latency comparison benchmark function
pub fn latency_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("Latency");

    // Configure benchmark group for latency measurement
    group.measurement_time(Duration::from_secs(10));
    group.warm_up_time(Duration::from_secs(3));
    group.sample_size(20); // Smaller sample size for latency tests

    benchmark_disruptor_latency(&mut group);
    benchmark_mpsc_latency(&mut group);
    benchmark_crossbeam_latency(&mut group);

    group.finish();
}

criterion_group!(latency, latency_benchmark);
criterion_main!(latency);
