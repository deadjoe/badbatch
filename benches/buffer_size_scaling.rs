#![allow(
    missing_docs,
    clippy::all,
    clippy::pedantic,
    clippy::nursery,
    clippy::cargo
)]

//! Buffer Size Scaling Benchmarks
//!
//! This benchmark suite tests how performance scales with different buffer sizes
//! and identifies optimal buffer configurations for different workloads.

use criterion::measurement::WallTime;
use criterion::{
    criterion_group, criterion_main, BenchmarkGroup, BenchmarkId, Criterion, Throughput,
};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use badbatch::disruptor::{
    event_translator::ClosureEventTranslator, BusySpinWaitStrategy, DefaultEventFactory, Disruptor,
    EventHandler, ProducerType, Result as DisruptorResult, YieldingWaitStrategy,
};

// Buffer size configurations to test
#[allow(dead_code)]
const BUFFER_SIZES: [usize; 8] = [64, 128, 256, 512, 1024, 2048, 4096, 8192];
#[allow(dead_code)]
const WORKLOAD_SIZES: [u64; 3] = [1_000, 5_000, 10_000]; // Standardized to common sizes
const TIMEOUT_MS: u64 = 5000; // 5 second timeout to prevent hanging

#[derive(Debug, Default, Clone)]
struct ScalingEvent {
    id: i64,
    data: Vec<i64>, // Variable size payload
}

/// Event handler that processes different payload sizes
struct ScalingHandler {
    counter: Arc<AtomicI64>,
    processing_time_ns: u64, // Simulate different processing times
}

impl ScalingHandler {
    fn new(processing_time_ns: u64) -> Self {
        Self {
            counter: Arc::new(AtomicI64::new(0)),
            processing_time_ns,
        }
    }

    fn get_counter(&self) -> Arc<AtomicI64> {
        self.counter.clone()
    }
}

impl EventHandler<ScalingEvent> for ScalingHandler {
    fn on_event(
        &mut self,
        event: &mut ScalingEvent,
        _sequence: i64,
        _end_of_batch: bool,
    ) -> DisruptorResult<()> {
        // Simulate processing time with a controlled busy loop
        if self.processing_time_ns > 0 {
            let start = std::time::Instant::now();
            while start.elapsed().as_nanos() < self.processing_time_ns as u128 {
                std::hint::spin_loop();
            }
        }

        // Process the event data
        let sum: i64 = event.data.iter().sum();
        std::hint::black_box(sum);

        self.counter.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

/// Safe wait with timeout to prevent hanging
fn wait_for_completion(counter: &Arc<AtomicI64>, target: i64, timeout_ms: u64) -> bool {
    let start = Instant::now();
    let timeout = Duration::from_millis(timeout_ms);

    while counter.load(Ordering::Relaxed) < target {
        if start.elapsed() > timeout {
            eprintln!(
                "WARNING: Benchmark timed out waiting for {} events, got {}",
                target,
                counter.load(Ordering::Relaxed)
            );
            return false;
        }
        std::hint::spin_loop();
    }
    true
}

/// Safe wait with timeout and yielding
fn wait_for_completion_yielding(counter: &Arc<AtomicI64>, target: i64, timeout_ms: u64) -> bool {
    let start = Instant::now();
    let timeout = Duration::from_millis(timeout_ms);

    while counter.load(Ordering::Relaxed) < target {
        if start.elapsed() > timeout {
            eprintln!(
                "WARNING: Benchmark timed out waiting for {} events, got {}",
                target,
                counter.load(Ordering::Relaxed)
            );
            return false;
        }
        std::thread::yield_now();
    }
    true
}

/// Safe wait with timeout using cooperative yielding to avoid millisecond polling artifacts.
fn wait_for_completion_cooperative(counter: &Arc<AtomicI64>, target: i64, timeout_ms: u64) -> bool {
    let start = Instant::now();
    let timeout = Duration::from_millis(timeout_ms);
    let mut spin_budget = 0_u32;

    while counter.load(Ordering::Relaxed) < target {
        if start.elapsed() > timeout {
            eprintln!(
                "WARNING: Benchmark timed out waiting for {} events, got {}",
                target,
                counter.load(Ordering::Relaxed)
            );
            return false;
        }

        if spin_budget < 1024 {
            spin_budget += 1;
            std::hint::spin_loop();
        } else {
            spin_budget = 0;
            std::thread::yield_now();
        }
    }
    true
}

/// Safe wait for counter with timeout
fn wait_for_counter_increase(
    counter: &Arc<AtomicI64>,
    initial_count: i64,
    timeout_ms: u64,
) -> bool {
    let start = Instant::now();
    let timeout = Duration::from_millis(timeout_ms);

    while counter.load(Ordering::Relaxed) <= initial_count {
        if start.elapsed() > timeout {
            eprintln!(
                "WARNING: Counter timeout, expected > {}, got {}",
                initial_count,
                counter.load(Ordering::Relaxed)
            );
            return false;
        }
        std::hint::spin_loop();
    }
    true
}

/// Test scaling with fast processing (minimal per-event work)
fn benchmark_fast_processing_scaling(
    group: &mut BenchmarkGroup<WallTime>,
    buffer_size: usize,
    workload_size: u64,
) {
    let param = format!("fast_buf{buffer_size}_work{workload_size}");
    let benchmark_id = BenchmarkId::new("FastProcessing", param);

    group.throughput(Throughput::Elements(workload_size));
    group.bench_function(benchmark_id, |b| {
        let factory = DefaultEventFactory::<ScalingEvent>::new();
        let handler = ScalingHandler::new(0); // No artificial processing delay
        let counter = handler.get_counter();

        let mut disruptor = Disruptor::new(
            factory,
            buffer_size,
            ProducerType::Single,
            Box::new(BusySpinWaitStrategy::new()),
        )
        .unwrap()
        .handle_events_with(handler)
        .build();

        disruptor.start().unwrap();

        b.iter_custom(|iters| {
            let start = Instant::now();
            for _ in 0..iters {
                let start_count = counter.load(Ordering::Relaxed);
                let target = start_count + workload_size as i64;

                for i in 1..=workload_size {
                    disruptor
                        .publish_event(ClosureEventTranslator::new(
                            move |event: &mut ScalingEvent, _seq: i64| {
                                event.id = std::hint::black_box(i as i64);
                                event.data.clear();
                                event.data.resize(4, i as i64); // Small payload (steady-state, no re-alloc)
                            },
                        ))
                        .unwrap();
                }

                // Wait for all events to be processed with timeout
                if !wait_for_completion(&counter, target, TIMEOUT_MS) {
                    panic!("Fast processing benchmark failed: events not processed within timeout");
                }
            }
            start.elapsed()
        });

        disruptor.shutdown().unwrap();
    });
}

/// Test scaling with medium processing (moderate per-event work)
fn benchmark_medium_processing_scaling(
    group: &mut BenchmarkGroup<WallTime>,
    buffer_size: usize,
    workload_size: u64,
) {
    let param = format!("medium_buf{buffer_size}_work{workload_size}");
    let benchmark_id = BenchmarkId::new("MediumProcessing", param);

    group.throughput(Throughput::Elements(workload_size));
    group.bench_function(benchmark_id, |b| {
        let factory = DefaultEventFactory::<ScalingEvent>::new();
        let handler = ScalingHandler::new(1_000); // 1 microsecond processing delay
        let counter = handler.get_counter();

        let mut disruptor = Disruptor::new(
            factory,
            buffer_size,
            ProducerType::Single,
            Box::new(YieldingWaitStrategy::new()),
        )
        .unwrap()
        .handle_events_with(handler)
        .build();

        disruptor.start().unwrap();

        b.iter_custom(|iters| {
            let start = Instant::now();
            for _ in 0..iters {
                let start_count = counter.load(Ordering::Relaxed);
                let target = start_count + workload_size as i64;

                for i in 1..=workload_size {
                    disruptor
                        .publish_event(ClosureEventTranslator::new(
                            move |event: &mut ScalingEvent, _seq: i64| {
                                event.id = std::hint::black_box(i as i64);
                                event.data.clear();
                                event.data.resize(16, i as i64); // Medium payload (steady-state)
                            },
                        ))
                        .unwrap();
                }

                // Wait for all events to be processed with timeout and yielding
                if !wait_for_completion_yielding(&counter, target, TIMEOUT_MS) {
                    panic!(
                        "Medium processing benchmark failed: events not processed within timeout"
                    );
                }
            }
            start.elapsed()
        });

        disruptor.shutdown().unwrap();
    });
}

/// Test scaling with slow processing (significant per-event work)
fn benchmark_slow_processing_scaling(
    group: &mut BenchmarkGroup<WallTime>,
    buffer_size: usize,
    workload_size: u64,
) {
    let param = format!("slow_buf{buffer_size}_work{workload_size}");
    let benchmark_id = BenchmarkId::new("SlowProcessing", param);

    group.throughput(Throughput::Elements(workload_size));
    group.bench_function(benchmark_id, |b| {
        let factory = DefaultEventFactory::<ScalingEvent>::new();
        let handler = ScalingHandler::new(10_000); // 10 microseconds processing delay
        let counter = handler.get_counter();

        let mut disruptor = Disruptor::new(
            factory,
            buffer_size,
            ProducerType::Single,
            Box::new(YieldingWaitStrategy::new()),
        )
        .unwrap()
        .handle_events_with(handler)
        .build();

        disruptor.start().unwrap();

        b.iter_custom(|iters| {
            let start = Instant::now();
            for _ in 0..iters {
                let start_count = counter.load(Ordering::Relaxed);
                let target = start_count + workload_size as i64;

                for i in 1..=workload_size {
                    disruptor
                        .publish_event(ClosureEventTranslator::new(
                            move |event: &mut ScalingEvent, _seq: i64| {
                                event.id = std::hint::black_box(i as i64);
                                event.data.clear();
                                event.data.resize(64, i as i64); // Large payload (steady-state)
                            },
                        ))
                        .unwrap();
                }

                // Use a cooperative waiter so slow-processing scenarios are not distorted
                // by 1ms polling granularity in the harness itself.
                if !wait_for_completion_cooperative(&counter, target, TIMEOUT_MS) {
                    panic!("Slow processing benchmark failed: events not processed within timeout");
                }
            }
            start.elapsed()
        });

        disruptor.shutdown().unwrap();
    });
}

/// Test memory usage scaling with different buffer sizes
fn benchmark_memory_scaling(group: &mut BenchmarkGroup<WallTime>, buffer_size: usize) {
    let benchmark_id = BenchmarkId::new("MemoryUsage", buffer_size);

    group.bench_function(benchmark_id, |b| {
        b.iter(|| {
            // Create fresh Disruptor instance for each benchmark run
            let factory = DefaultEventFactory::<ScalingEvent>::new();
            let handler = ScalingHandler::new(0);
            let counter = handler.get_counter();

            let mut disruptor = Disruptor::new(
                factory,
                buffer_size,
                ProducerType::Single,
                Box::new(BusySpinWaitStrategy::new()),
            )
            .unwrap()
            .handle_events_with(handler)
            .build();

            disruptor.start().unwrap();

            // Test memory allocation patterns by creating events with different payload sizes
            for payload_size in [1, 10, 100] {
                // Capture the counter *before* publishing to avoid a race where the event is
                // processed before we read `initial_count`.
                let initial_count = counter.load(Ordering::Relaxed);

                let success = disruptor.try_publish_event(ClosureEventTranslator::new(
                    move |event: &mut ScalingEvent, _seq: i64| {
                        event.id = std::hint::black_box(1);
                        event.data = vec![1; payload_size];
                    },
                ));

                if success {
                    // Wait for event to be processed with timeout
                    if !wait_for_counter_increase(&counter, initial_count, TIMEOUT_MS) {
                        eprintln!(
                            "WARNING: Memory scaling timeout for payload_size {payload_size}"
                        );
                        break; // Continue with next payload size
                    }
                }
            }

            disruptor.shutdown().unwrap();
        })
    });
}

/// Test buffer utilization patterns
fn benchmark_buffer_utilization(
    group: &mut BenchmarkGroup<WallTime>,
    buffer_size: usize,
    burst_size: u64,
) {
    let param = format!("util_buf{buffer_size}_burst{burst_size}");
    let benchmark_id = BenchmarkId::new("BufferUtil", param);

    group.throughput(Throughput::Elements(burst_size));
    group.bench_function(benchmark_id, |b| {
        let factory = DefaultEventFactory::<ScalingEvent>::new();
        let handler = ScalingHandler::new(5_000); // 5 microseconds delay to create backpressure
        let counter = handler.get_counter();

        let mut disruptor = Disruptor::new(
            factory,
            buffer_size,
            ProducerType::Single,
            Box::new(BusySpinWaitStrategy::new()),
        )
        .unwrap()
        .handle_events_with(handler)
        .build();

        disruptor.start().unwrap();

        b.iter_custom(|iters| {
            let start = Instant::now();
            for _ in 0..iters {
                let start_count = counter.load(Ordering::Relaxed);
                let target = start_count + burst_size as i64;

                // Publish a burst of events
                for i in 1..=burst_size {
                    // Use try_publish to test buffer utilization
                    while !disruptor.try_publish_event(ClosureEventTranslator::new(
                        move |event: &mut ScalingEvent, _seq: i64| {
                            event.id = std::hint::black_box(i as i64);
                            event.data.clear();
                            event.data.resize(8, i as i64);
                        },
                    )) {
                        // Buffer is full, brief wait
                        std::hint::spin_loop();
                    }
                }

                // Wait for all events to be processed with timeout
                if !wait_for_completion(&counter, target, TIMEOUT_MS) {
                    panic!(
                        "Buffer utilization benchmark failed: events not processed within timeout"
                    );
                }
            }
            start.elapsed()
        });

        disruptor.shutdown().unwrap();
    });
}

/// Main buffer size scaling benchmark function
pub fn buffer_scaling_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("BufferScaling");

    // Configure benchmark group with reduced times to prevent timeout
    group.measurement_time(Duration::from_secs(10)); // Reduced from 20s
    group.warm_up_time(Duration::from_secs(3)); // Reduced from 5s

    // Drastically reduced test matrix to prevent 900s timeout
    // Focus on key buffer sizes and smaller workloads
    let key_buffer_sizes = [64, 128, 256]; // Test only 3 representative sizes
    let key_workload_sizes = [1_000]; // Test only smallest workload

    // Test fast processing scaling for key configurations only
    for &buffer_size in key_buffer_sizes.iter() {
        for &workload_size in key_workload_sizes.iter() {
            benchmark_fast_processing_scaling(&mut group, buffer_size, workload_size);
        }
    }

    // Test one medium processing scaling configuration
    benchmark_medium_processing_scaling(&mut group, 256, 1_000);

    // Test one slow processing scaling configuration
    benchmark_slow_processing_scaling(&mut group, 512, 1_000);

    // Test memory usage for representative buffer sizes only
    for &buffer_size in [64, 256, 1024].iter() {
        benchmark_memory_scaling(&mut group, buffer_size);
    }

    // Test one buffer utilization pattern
    benchmark_buffer_utilization(&mut group, 256, 100);

    group.finish();
}

criterion_group!(buffer_scaling, buffer_scaling_benchmark);
criterion_main!(buffer_scaling);
