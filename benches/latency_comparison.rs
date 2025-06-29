//! Latency Comparison Benchmarks
//!
//! This benchmark suite compares the latency characteristics of the BadBatch
//! Disruptor against other concurrency primitives like channels.

use criterion::{
    black_box, criterion_group, criterion_main, BenchmarkGroup, BenchmarkId, Criterion,
};
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
    counter: Arc<AtomicI64>,
}

impl LatencyHandler {
    fn new(capacity: usize) -> Self {
        let latencies: Vec<AtomicI64> = (0..capacity).map(|_| AtomicI64::new(0)).collect();

        Self {
            latencies: Arc::new(latencies),
            counter: Arc::new(AtomicI64::new(0)),
        }
    }

    fn get_latencies(&self) -> Arc<Vec<AtomicI64>> {
        self.latencies.clone()
    }

    fn get_counter(&self) -> Arc<AtomicI64> {
        self.counter.clone()
    }

    #[allow(dead_code)]
    fn reset(&self) {
        self.counter.store(0, Ordering::Release);
        for latency in self.latencies.iter() {
            latency.store(0, Ordering::Release);
        }
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

        let index = self.counter.fetch_add(1, Ordering::Release) as usize;
        if index < self.latencies.len() {
            self.latencies[index].store(latency as i64, Ordering::Release);
        }

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
    let latencies = handler.get_latencies();
    let counter = handler.get_counter();

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

    group.bench_function(benchmark_id, |b| {
        b.iter(|| {
            counter.store(0, Ordering::Release);

            // Measure single event latency
            let send_time = get_timestamp_nanos();

            disruptor
                .publish_event(ClosureEventTranslator::new(
                    move |event: &mut LatencyEvent, _seq: i64| {
                        event.id = black_box(1);
                        event.send_time = send_time;
                    },
                ))
                .unwrap();

            // Wait for the single event to be processed
            while counter.load(Ordering::Acquire) < 1 {
                std::hint::spin_loop();
            }

            black_box(())
        })
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
            if sender.send((black_box(1), _send_time)).is_ok() {
                // Wait for the single event to be processed
                while counter.load(Ordering::Acquire) < 1 {
                    std::hint::spin_loop();
                }
            }

            // Close sender to signal receiver to stop
            drop(sender);
            receiver_handle.join().unwrap();

            black_box(())
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
            if sender.send((black_box(1), _send_time)).is_ok() {
                // Wait for the single event to be processed
                while counter.load(Ordering::Acquire) < 1 {
                    std::hint::spin_loop();
                }
            }

            // Close sender to signal receiver to stop
            drop(sender);
            receiver_handle.join().unwrap();

            black_box(())
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
