#![allow(
    missing_docs,
    clippy::all,
    clippy::pedantic,
    clippy::nursery,
    clippy::cargo
)]

//! Fixed Single Producer Single Consumer (SPSC) Benchmarks
//!
//! This benchmark suite tests the performance of the BadBatch Disruptor
//! in single producer, single consumer scenarios with proper timeout handling.

use criterion::measurement::WallTime;
use criterion::{
    criterion_group, criterion_main, BenchmarkGroup, BenchmarkId, Criterion, Throughput,
};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use badbatch::disruptor::{
    build_single_producer, event_translator::ClosureEventTranslator, BlockingWaitStrategy,
    BusySpinWaitStrategy, DefaultEventFactory, Disruptor, EventHandler, ProducerType,
    Result as DisruptorResult, SleepingWaitStrategy, YieldingWaitStrategy,
};

// Benchmark configuration constants
const BUFFER_SIZE: usize = 1024;
const TIMEOUT_MS: u64 = 5000; // 5 second timeout to prevent hanging

#[derive(Debug, Default, Clone, Copy)]
struct BenchmarkEvent {
    value: i64,
    sequence: i64,
}

/// Event handler that stores the last processed value in an atomic sink
struct CountingSink {
    counter: Arc<AtomicI64>,
}

impl CountingSink {
    fn new() -> Self {
        Self {
            counter: Arc::new(AtomicI64::new(0)),
        }
    }

    fn get_counter(&self) -> Arc<AtomicI64> {
        self.counter.clone()
    }
}

impl EventHandler<BenchmarkEvent> for CountingSink {
    fn on_event(
        &mut self,
        event: &mut BenchmarkEvent,
        _sequence: i64,
        _end_of_batch: bool,
    ) -> DisruptorResult<()> {
        // Minimal processing - just increment counter
        std::hint::black_box(event.value);
        self.counter.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

/// Safe wait with timeout to prevent hanging
fn wait_for_completion(counter: &Arc<AtomicI64>, expected: i64, timeout_ms: u64) -> bool {
    let start = Instant::now();
    let timeout = Duration::from_millis(timeout_ms);

    while counter.load(Ordering::Relaxed) < expected {
        if start.elapsed() > timeout {
            eprintln!(
                "WARNING: Benchmark timed out waiting for {expected} events, got {}",
                counter.load(Ordering::Relaxed)
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

    while counter.load(Ordering::Relaxed) < expected {
        if start.elapsed() > timeout {
            eprintln!(
                "WARNING: Benchmark timed out waiting for {expected} events, got {}",
                counter.load(Ordering::Relaxed)
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

    while counter.load(Ordering::Relaxed) < expected {
        if start.elapsed() > timeout {
            eprintln!(
                "WARNING: Benchmark timed out waiting for {expected} events, got {}",
                counter.load(Ordering::Relaxed)
            );
            return false;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    true
}

/// Baseline measurement to determine overhead
fn baseline_measurement(group: &mut BenchmarkGroup<WallTime>, burst_size: u64) {
    let counter = Arc::new(AtomicI64::new(0));
    let benchmark_id = BenchmarkId::new("baseline", burst_size);

    group.throughput(Throughput::Elements(burst_size));
    group.bench_function(benchmark_id, |b| {
        b.iter_custom(|iters| {
            let start = Instant::now();
            for _ in 0..iters {
                counter.store(0, Ordering::Relaxed);
                for i in 1..=burst_size {
                    counter.store(std::hint::black_box(i as i64), Ordering::Relaxed);
                }
                // Simple verification
                assert_eq!(counter.load(Ordering::Relaxed), burst_size as i64);
            }
            start.elapsed()
        })
    });
}

/// Benchmark with BusySpinWaitStrategy
fn benchmark_busy_spin(group: &mut BenchmarkGroup<WallTime>, burst_size: u64, pause_ms: u64) {
    let param = format!("burst:{burst_size}_pause:{pause_ms}ms");
    let benchmark_id = BenchmarkId::new("BusySpin", param);

    // Create Disruptor OUTSIDE the benchmark iteration - this is the key fix
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

    group.throughput(Throughput::Elements(burst_size));
    group.bench_function(benchmark_id, |b| {
        b.iter_custom(|iters| {
            if pause_ms > 0 {
                std::thread::sleep(Duration::from_millis(pause_ms));
            }

            let start = Instant::now();
            for _ in 0..iters {
                counter.store(0, Ordering::Relaxed); // Reset counter for each iteration

                for i in 1..=burst_size {
                    disruptor
                        .publish_event(ClosureEventTranslator::new(
                            move |event: &mut BenchmarkEvent, seq: i64| {
                                event.value = std::hint::black_box(i as i64);
                                event.sequence = seq;
                            },
                        ))
                        .unwrap();
                }

                // Wait for all events to be processed with timeout
                if !wait_for_completion(&counter, burst_size as i64, TIMEOUT_MS) {
                    panic!("BusySpin benchmark failed: events not processed within timeout");
                }
            }
            start.elapsed()
        })
    });

    if let Err(e) = disruptor.shutdown() {
        eprintln!("WARNING: BusySpin shutdown failed: {e:?}");
    }
}

/// Benchmark with YieldingWaitStrategy
fn benchmark_yielding(group: &mut BenchmarkGroup<WallTime>, burst_size: u64, pause_ms: u64) {
    let param = format!("burst:{burst_size}_pause:{pause_ms}ms");
    let benchmark_id = BenchmarkId::new("Yielding", param);

    // Create Disruptor OUTSIDE the benchmark iteration
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

    group.throughput(Throughput::Elements(burst_size));
    group.bench_function(benchmark_id, |b| {
        b.iter_custom(|iters| {
            if pause_ms > 0 {
                std::thread::sleep(Duration::from_millis(pause_ms));
            }

            let start = Instant::now();
            for _ in 0..iters {
                counter.store(0, Ordering::Relaxed); // Reset counter for each iteration

                for i in 1..=burst_size {
                    disruptor
                        .publish_event(ClosureEventTranslator::new(
                            move |event: &mut BenchmarkEvent, seq: i64| {
                                event.value = std::hint::black_box(i as i64);
                                event.sequence = seq;
                            },
                        ))
                        .unwrap();
                }

                // Wait for all events to be processed with timeout and yielding
                if !wait_for_completion_yielding(&counter, burst_size as i64, TIMEOUT_MS) {
                    panic!("Yielding benchmark failed: events not processed within timeout");
                }
            }
            start.elapsed()
        })
    });

    if let Err(e) = disruptor.shutdown() {
        eprintln!("WARNING: Yielding shutdown failed: {e:?}");
    }
}

/// Benchmark with BlockingWaitStrategy
fn benchmark_blocking(group: &mut BenchmarkGroup<WallTime>, burst_size: u64, pause_ms: u64) {
    let param = format!("burst:{burst_size}_pause:{pause_ms}ms");
    let benchmark_id = BenchmarkId::new("Blocking", param);

    // Create Disruptor OUTSIDE the benchmark iteration
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

    group.throughput(Throughput::Elements(burst_size));
    group.bench_function(benchmark_id, |b| {
        b.iter_custom(|iters| {
            if pause_ms > 0 {
                std::thread::sleep(Duration::from_millis(pause_ms));
            }

            let start = Instant::now();
            for _ in 0..iters {
                counter.store(0, Ordering::Relaxed); // Reset counter for each iteration

                for i in 1..=burst_size {
                    disruptor
                        .publish_event(ClosureEventTranslator::new(
                            move |event: &mut BenchmarkEvent, seq: i64| {
                                event.value = std::hint::black_box(i as i64);
                                event.sequence = seq;
                            },
                        ))
                        .unwrap();
                }

                // Wait for all events to be processed with timeout and sleeping
                if !wait_for_completion_sleeping(&counter, burst_size as i64, TIMEOUT_MS) {
                    panic!("Blocking benchmark failed: events not processed within timeout");
                }
            }
            start.elapsed()
        })
    });

    if let Err(e) = disruptor.shutdown() {
        eprintln!("WARNING: Blocking shutdown failed: {e:?}");
    }
}

/// Benchmark with SleepingWaitStrategy
fn benchmark_sleeping(group: &mut BenchmarkGroup<WallTime>, burst_size: u64, pause_ms: u64) {
    let param = format!("burst:{burst_size}_pause:{pause_ms}ms");
    let benchmark_id = BenchmarkId::new("Sleeping", param);

    // Create Disruptor OUTSIDE the benchmark iteration
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

    group.throughput(Throughput::Elements(burst_size));
    group.bench_function(benchmark_id, |b| {
        b.iter_custom(|iters| {
            if pause_ms > 0 {
                std::thread::sleep(Duration::from_millis(pause_ms));
            }

            let start = Instant::now();
            for _ in 0..iters {
                counter.store(0, Ordering::Relaxed); // Reset counter for each iteration

                for i in 1..=burst_size {
                    disruptor
                        .publish_event(ClosureEventTranslator::new(
                            move |event: &mut BenchmarkEvent, seq: i64| {
                                event.value = std::hint::black_box(i as i64);
                                event.sequence = seq;
                            },
                        ))
                        .unwrap();
                }

                // Wait for all events to be processed with timeout and sleeping
                if !wait_for_completion_sleeping(&counter, burst_size as i64, TIMEOUT_MS) {
                    panic!("Sleeping benchmark failed: events not processed within timeout");
                }
            }
            start.elapsed()
        })
    });

    if let Err(e) = disruptor.shutdown() {
        eprintln!("WARNING: Sleeping shutdown failed: {e:?}");
    }
}

/// Reference-aligned benchmark that claims and publishes the whole burst in one batch.
///
/// This is closer to the LMAX raw throughput tests and the disruptor-rs benches, which
/// primarily exercise range-claim publication rather than per-event translator calls.
fn benchmark_batch_busy_spin(group: &mut BenchmarkGroup<WallTime>, burst_size: u64, pause_ms: u64) {
    let param = format!("batch_burst:{burst_size}_pause:{pause_ms}ms");
    let benchmark_id = BenchmarkId::new("BatchBusySpin", param);

    let counter = Arc::new(AtomicI64::new(0));
    let mut disruptor = build_single_producer(
        BUFFER_SIZE,
        BenchmarkEvent::default,
        BusySpinWaitStrategy::new(),
    )
    .handle_events_with({
        let counter = Arc::clone(&counter);
        move |event: &mut BenchmarkEvent, _sequence, _end_of_batch| {
            std::hint::black_box(event.value);
            counter.fetch_add(1, Ordering::Relaxed);
        }
    })
    .build();

    group.throughput(Throughput::Elements(burst_size));
    group.bench_function(benchmark_id, |b| {
        b.iter_custom(|iters| {
            if pause_ms > 0 {
                std::thread::sleep(Duration::from_millis(pause_ms));
            }

            let start = Instant::now();
            for _ in 0..iters {
                counter.store(0, Ordering::Relaxed);

                disruptor.batch_publish(burst_size as usize, |iter| {
                    for (index, event) in iter.enumerate() {
                        let value = (index + 1) as i64;
                        event.value = std::hint::black_box(value);
                    }
                });

                if !wait_for_completion(&counter, burst_size as i64, TIMEOUT_MS) {
                    panic!("BatchBusySpin benchmark failed: events not processed within timeout");
                }
            }
            start.elapsed()
        })
    });

    disruptor.shutdown();
}

fn benchmark_batch_yielding(group: &mut BenchmarkGroup<WallTime>, burst_size: u64, pause_ms: u64) {
    let param = format!("batch_burst:{burst_size}_pause:{pause_ms}ms");
    let benchmark_id = BenchmarkId::new("BatchYielding", param);

    let counter = Arc::new(AtomicI64::new(0));
    let mut disruptor = build_single_producer(
        BUFFER_SIZE,
        BenchmarkEvent::default,
        YieldingWaitStrategy::new(),
    )
    .handle_events_with({
        let counter = Arc::clone(&counter);
        move |event: &mut BenchmarkEvent, _sequence, _end_of_batch| {
            std::hint::black_box(event.value);
            counter.fetch_add(1, Ordering::Relaxed);
        }
    })
    .build();

    group.throughput(Throughput::Elements(burst_size));
    group.bench_function(benchmark_id, |b| {
        b.iter_custom(|iters| {
            if pause_ms > 0 {
                std::thread::sleep(Duration::from_millis(pause_ms));
            }

            let start = Instant::now();
            for _ in 0..iters {
                counter.store(0, Ordering::Relaxed);

                disruptor.batch_publish(burst_size as usize, |iter| {
                    for (index, event) in iter.enumerate() {
                        let value = (index + 1) as i64;
                        event.value = std::hint::black_box(value);
                    }
                });

                if !wait_for_completion_yielding(&counter, burst_size as i64, TIMEOUT_MS) {
                    panic!("BatchYielding benchmark failed: events not processed within timeout");
                }
            }
            start.elapsed()
        })
    });

    disruptor.shutdown();
}

/// Main SPSC benchmark function
pub fn fixed_spsc_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("Fixed_SPSC");

    // Configure benchmark group with shorter timeouts to prevent hanging
    group.measurement_time(Duration::from_secs(10));
    group.warm_up_time(Duration::from_secs(3));
    group.sample_size(20);

    // Reduced test matrix to prevent timeouts while maintaining coverage
    let test_burst_sizes = [100, 1000]; // Standardized burst sizes for consistency
    let test_pause_ms = [0]; // Only test without pause to minimize combinations

    for &burst_size in test_burst_sizes.iter() {
        // Baseline measurement
        baseline_measurement(&mut group, burst_size);

        for &pause_ms in test_pause_ms.iter() {
            benchmark_busy_spin(&mut group, burst_size, pause_ms);
            benchmark_yielding(&mut group, burst_size, pause_ms);
            benchmark_blocking(&mut group, burst_size, pause_ms);
            benchmark_sleeping(&mut group, burst_size, pause_ms);
            benchmark_batch_busy_spin(&mut group, burst_size, pause_ms);
            benchmark_batch_yielding(&mut group, burst_size, pause_ms);
        }
    }

    group.finish();
}

criterion_group!(fixed_spsc, fixed_spsc_benchmark);
criterion_main!(fixed_spsc);
