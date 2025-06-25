//! Fixed Single Producer Single Consumer (SPSC) Benchmarks
//!
//! This benchmark suite tests the performance of the BadBatch Disruptor
//! in single producer, single consumer scenarios with proper timeout handling.

use criterion::measurement::WallTime;
use criterion::{
    black_box, criterion_group, criterion_main, BenchmarkGroup, BenchmarkId, Criterion, Throughput,
};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use badbatch::disruptor::{
    event_translator::ClosureEventTranslator, BlockingWaitStrategy, BusySpinWaitStrategy,
    DefaultEventFactory, Disruptor, EventHandler, ProducerType, Result as DisruptorResult,
    SleepingWaitStrategy, YieldingWaitStrategy,
};

// Benchmark configuration constants
const BUFFER_SIZE: usize = 1024;
const BURST_SIZES: [u64; 4] = [1, 10, 100, 1000];
const PAUSE_MS: [u64; 3] = [0, 1, 10];
const TIMEOUT_MS: u64 = 5000; // 5 second timeout to prevent hanging

#[derive(Debug, Default, Clone, Copy)]
struct BenchmarkEvent {
    value: i64,
    sequence: i64,
}

/// Event handler that stores the last processed value in an atomic sink
struct CountingSink {
    sink: Arc<AtomicI64>,
    counter: Arc<AtomicI64>,
}

impl CountingSink {
    fn new() -> Self {
        Self {
            sink: Arc::new(AtomicI64::new(0)),
            counter: Arc::new(AtomicI64::new(0)),
        }
    }

    #[allow(dead_code)]
    fn get_sink(&self) -> Arc<AtomicI64> {
        self.sink.clone()
    }

    fn get_counter(&self) -> Arc<AtomicI64> {
        self.counter.clone()
    }

    #[allow(dead_code)]
    fn reset(&self) {
        self.sink.store(0, Ordering::Release);
        self.counter.store(0, Ordering::Release);
    }
}

impl EventHandler<BenchmarkEvent> for CountingSink {
    fn on_event(
        &mut self,
        event: &mut BenchmarkEvent,
        _sequence: i64,
        _end_of_batch: bool,
    ) -> DisruptorResult<()> {
        // Store the value and increment counter
        self.sink.store(event.value, Ordering::Release);
        self.counter.fetch_add(1, Ordering::Release);
        Ok(())
    }
}

/// Safe wait with timeout to prevent hanging
fn wait_for_completion(counter: &Arc<AtomicI64>, expected: i64, timeout_ms: u64) -> bool {
    let start = Instant::now();
    let timeout = Duration::from_millis(timeout_ms);

    while counter.load(Ordering::Acquire) < expected {
        if start.elapsed() > timeout {
            eprintln!(
                "WARNING: Benchmark timed out waiting for {} events, got {}",
                expected,
                counter.load(Ordering::Acquire)
            );
            return false;
        }
        std::hint::spin_loop();
    }
    true
}

/// Safe wait with timeout and yielding
fn wait_for_completion_yielding(counter: &Arc<AtomicI64>, expected: i64, timeout_ms: u64) -> bool {
    let start = Instant::now();
    let timeout = Duration::from_millis(timeout_ms);

    while counter.load(Ordering::Acquire) < expected {
        if start.elapsed() > timeout {
            eprintln!(
                "WARNING: Benchmark timed out waiting for {} events, got {}",
                expected,
                counter.load(Ordering::Acquire)
            );
            return false;
        }
        std::thread::yield_now();
    }
    true
}

/// Safe wait with timeout and sleeping
fn wait_for_completion_sleeping(counter: &Arc<AtomicI64>, expected: i64, timeout_ms: u64) -> bool {
    let start = Instant::now();
    let timeout = Duration::from_millis(timeout_ms);

    while counter.load(Ordering::Acquire) < expected {
        if start.elapsed() > timeout {
            eprintln!(
                "WARNING: Benchmark timed out waiting for {} events, got {}",
                expected,
                counter.load(Ordering::Acquire)
            );
            return false;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    true
}

/// Baseline measurement to determine overhead
fn baseline_measurement(group: &mut BenchmarkGroup<WallTime>, burst_size: u64) {
    let sink = Arc::new(AtomicI64::new(0));
    let benchmark_id = BenchmarkId::new("baseline", burst_size);

    group.throughput(Throughput::Elements(burst_size));
    group.bench_function(benchmark_id, |b| {
        b.iter_custom(|iters| {
            let start = Instant::now();
            for _ in 0..iters {
                for i in 1..=burst_size {
                    sink.store(black_box(i as i64), Ordering::Release);
                }
                // Simple verification
                assert_eq!(sink.load(Ordering::Acquire), burst_size as i64);
            }
            start.elapsed()
        })
    });
}

/// Benchmark with BusySpinWaitStrategy
fn benchmark_busy_spin(group: &mut BenchmarkGroup<WallTime>, burst_size: u64, pause_ms: u64) {
    let factory = DefaultEventFactory::<BenchmarkEvent>::new();
    let handler = CountingSink::new();
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

    let param = format!("burst:{}_pause:{}ms", burst_size, pause_ms);
    let benchmark_id = BenchmarkId::new("BusySpin", param);

    group.throughput(Throughput::Elements(burst_size));
    group.bench_function(benchmark_id, |b| {
        b.iter_custom(|iters| {
            if pause_ms > 0 {
                std::thread::sleep(Duration::from_millis(pause_ms));
            }

            let start = Instant::now();
            for _ in 0..iters {
                counter.store(0, Ordering::Release);

                for i in 1..=burst_size {
                    disruptor
                        .publish_event(ClosureEventTranslator::new(
                            move |event: &mut BenchmarkEvent, seq: i64| {
                                event.value = black_box(i as i64);
                                event.sequence = seq;
                            },
                        ))
                        .unwrap();
                }

                // Wait for all events to be processed with timeout
                if !wait_for_completion(&counter, burst_size as i64, TIMEOUT_MS) {
                    panic!("Benchmark failed: events not processed within timeout");
                }
            }
            start.elapsed()
        })
    });

    disruptor.shutdown().unwrap();
}

/// Benchmark with YieldingWaitStrategy
fn benchmark_yielding(group: &mut BenchmarkGroup<WallTime>, burst_size: u64, pause_ms: u64) {
    let factory = DefaultEventFactory::<BenchmarkEvent>::new();
    let handler = CountingSink::new();
    let counter = handler.get_counter();

    let mut disruptor = Disruptor::new(
        factory,
        BUFFER_SIZE,
        ProducerType::Single,
        Box::new(YieldingWaitStrategy::new()),
    )
    .unwrap()
    .handle_events_with(handler)
    .build();

    disruptor.start().unwrap();

    let param = format!("burst:{}_pause:{}ms", burst_size, pause_ms);
    let benchmark_id = BenchmarkId::new("Yielding", param);

    group.throughput(Throughput::Elements(burst_size));
    group.bench_function(benchmark_id, |b| {
        b.iter_custom(|iters| {
            if pause_ms > 0 {
                std::thread::sleep(Duration::from_millis(pause_ms));
            }

            let start = Instant::now();
            for _ in 0..iters {
                counter.store(0, Ordering::Release);

                for i in 1..=burst_size {
                    disruptor
                        .publish_event(ClosureEventTranslator::new(
                            move |event: &mut BenchmarkEvent, seq: i64| {
                                event.value = black_box(i as i64);
                                event.sequence = seq;
                            },
                        ))
                        .unwrap();
                }

                // Wait for all events to be processed with timeout and yielding
                if !wait_for_completion_yielding(&counter, burst_size as i64, TIMEOUT_MS) {
                    panic!("Benchmark failed: events not processed within timeout");
                }
            }
            start.elapsed()
        })
    });

    disruptor.shutdown().unwrap();
}

/// Benchmark with BlockingWaitStrategy
fn benchmark_blocking(group: &mut BenchmarkGroup<WallTime>, burst_size: u64, pause_ms: u64) {
    let factory = DefaultEventFactory::<BenchmarkEvent>::new();
    let handler = CountingSink::new();
    let counter = handler.get_counter();

    let mut disruptor = Disruptor::new(
        factory,
        BUFFER_SIZE,
        ProducerType::Single,
        Box::new(BlockingWaitStrategy::new()),
    )
    .unwrap()
    .handle_events_with(handler)
    .build();

    disruptor.start().unwrap();

    let param = format!("burst:{}_pause:{}ms", burst_size, pause_ms);
    let benchmark_id = BenchmarkId::new("Blocking", param);

    group.throughput(Throughput::Elements(burst_size));
    group.bench_function(benchmark_id, |b| {
        b.iter_custom(|iters| {
            if pause_ms > 0 {
                std::thread::sleep(Duration::from_millis(pause_ms));
            }

            let start = Instant::now();
            for _ in 0..iters {
                counter.store(0, Ordering::Release);

                for i in 1..=burst_size {
                    disruptor
                        .publish_event(ClosureEventTranslator::new(
                            move |event: &mut BenchmarkEvent, seq: i64| {
                                event.value = black_box(i as i64);
                                event.sequence = seq;
                            },
                        ))
                        .unwrap();
                }

                // Wait for all events to be processed with timeout and sleeping
                if !wait_for_completion_sleeping(&counter, burst_size as i64, TIMEOUT_MS) {
                    panic!("Benchmark failed: events not processed within timeout");
                }
            }
            start.elapsed()
        })
    });

    disruptor.shutdown().unwrap();
}

/// Benchmark with SleepingWaitStrategy
fn benchmark_sleeping(group: &mut BenchmarkGroup<WallTime>, burst_size: u64, pause_ms: u64) {
    let factory = DefaultEventFactory::<BenchmarkEvent>::new();
    let handler = CountingSink::new();
    let counter = handler.get_counter();

    let mut disruptor = Disruptor::new(
        factory,
        BUFFER_SIZE,
        ProducerType::Single,
        Box::new(SleepingWaitStrategy::new()),
    )
    .unwrap()
    .handle_events_with(handler)
    .build();

    disruptor.start().unwrap();

    let param = format!("burst:{}_pause:{}ms", burst_size, pause_ms);
    let benchmark_id = BenchmarkId::new("Sleeping", param);

    group.throughput(Throughput::Elements(burst_size));
    group.bench_function(benchmark_id, |b| {
        b.iter_custom(|iters| {
            if pause_ms > 0 {
                std::thread::sleep(Duration::from_millis(pause_ms));
            }

            let start = Instant::now();
            for _ in 0..iters {
                counter.store(0, Ordering::Release);

                for i in 1..=burst_size {
                    disruptor
                        .publish_event(ClosureEventTranslator::new(
                            move |event: &mut BenchmarkEvent, seq: i64| {
                                event.value = black_box(i as i64);
                                event.sequence = seq;
                            },
                        ))
                        .unwrap();
                }

                // Wait for all events to be processed with timeout and sleeping
                if !wait_for_completion_sleeping(&counter, burst_size as i64, TIMEOUT_MS) {
                    panic!("Benchmark failed: events not processed within timeout");
                }
            }
            start.elapsed()
        })
    });

    disruptor.shutdown().unwrap();
}

/// Main SPSC benchmark function
pub fn fixed_spsc_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("Fixed_SPSC");

    // Configure benchmark group with reasonable timeouts
    group.measurement_time(Duration::from_secs(10));
    group.warm_up_time(Duration::from_secs(3));
    group.sample_size(20); // More samples for better statistics

    for &burst_size in BURST_SIZES.iter() {
        // Baseline measurement
        baseline_measurement(&mut group, burst_size);

        for &pause_ms in PAUSE_MS.iter() {
            // Skip very large combinations to keep benchmarks manageable
            if burst_size > 100 && pause_ms > 1 {
                continue;
            }

            benchmark_busy_spin(&mut group, burst_size, pause_ms);
            benchmark_yielding(&mut group, burst_size, pause_ms);
            benchmark_blocking(&mut group, burst_size, pause_ms);

            // Only test sleeping strategy for smaller burst sizes
            if burst_size <= 100 {
                benchmark_sleeping(&mut group, burst_size, pause_ms);
            }
        }
    }

    group.finish();
}

criterion_group!(fixed_spsc, fixed_spsc_benchmark);
criterion_main!(fixed_spsc);
