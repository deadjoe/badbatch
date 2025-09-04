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
    last_id: Arc<AtomicI64>,
    processing_time_ns: u64, // Simulate different processing times
}

impl ScalingHandler {
    fn new(processing_time_ns: u64) -> Self {
        Self {
            counter: Arc::new(AtomicI64::new(0)),
            last_id: Arc::new(AtomicI64::new(0)),
            processing_time_ns,
        }
    }

    fn get_counter(&self) -> Arc<AtomicI64> {
        self.counter.clone()
    }

    fn get_last_id(&self) -> Arc<AtomicI64> {
        self.last_id.clone()
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

        self.last_id.store(event.id, Ordering::Release);
        self.counter.fetch_add(1, Ordering::Release);
        Ok(())
    }
}

/// Safe wait with timeout to prevent hanging
fn wait_for_completion(last_id: &Arc<AtomicI64>, expected: i64, timeout_ms: u64) -> bool {
    let start = Instant::now();
    let timeout = Duration::from_millis(timeout_ms);

    while last_id.load(Ordering::Acquire) < expected {
        if start.elapsed() > timeout {
            eprintln!(
                "WARNING: Benchmark timed out waiting for {expected} events, got {}",
                last_id.load(Ordering::Acquire)
            );
            return false;
        }
        std::hint::spin_loop();
    }
    true
}

/// Safe wait with timeout and yielding
fn wait_for_completion_yielding(last_id: &Arc<AtomicI64>, expected: i64, timeout_ms: u64) -> bool {
    let start = Instant::now();
    let timeout = Duration::from_millis(timeout_ms);

    while last_id.load(Ordering::Acquire) < expected {
        if start.elapsed() > timeout {
            eprintln!(
                "WARNING: Benchmark timed out waiting for {expected} events, got {}",
                last_id.load(Ordering::Acquire)
            );
            return false;
        }
        std::thread::yield_now();
    }
    true
}

/// Safe wait with timeout and sleeping
fn wait_for_completion_sleeping(last_id: &Arc<AtomicI64>, expected: i64, timeout_ms: u64) -> bool {
    let start = Instant::now();
    let timeout = Duration::from_millis(timeout_ms);

    while last_id.load(Ordering::Acquire) < expected {
        if start.elapsed() > timeout {
            eprintln!(
                "WARNING: Benchmark timed out waiting for {expected} events, got {}",
                last_id.load(Ordering::Acquire)
            );
            return false;
        }
        std::thread::sleep(Duration::from_millis(1));
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

    while counter.load(Ordering::Acquire) <= initial_count {
        if start.elapsed() > timeout {
            eprintln!(
                "WARNING: Counter timeout, expected > {}, got {}",
                initial_count,
                counter.load(Ordering::Acquire)
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
        b.iter(|| {
            // Create fresh Disruptor instance for each benchmark run
            let factory = DefaultEventFactory::<ScalingEvent>::new();
            let handler = ScalingHandler::new(0); // No artificial processing delay
            let _counter = handler.get_counter();
            let last_id = handler.get_last_id();

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

            for i in 1..=workload_size {
                disruptor
                    .publish_event(ClosureEventTranslator::new(
                        move |event: &mut ScalingEvent, _seq: i64| {
                            event.id = std::hint::black_box(i as i64);
                            event.data = vec![i as i64; 4]; // Small payload
                        },
                    ))
                    .unwrap();
            }

            // Wait for all events to be processed with timeout
            if !wait_for_completion(&last_id, workload_size as i64, TIMEOUT_MS) {
                panic!("Fast processing benchmark failed: events not processed within timeout");
            }

            disruptor.shutdown().unwrap();
        })
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
        b.iter(|| {
            // Create fresh Disruptor instance for each benchmark run
            let factory = DefaultEventFactory::<ScalingEvent>::new();
            let handler = ScalingHandler::new(1_000); // 1 microsecond processing delay
            let _counter = handler.get_counter();
            let last_id = handler.get_last_id();

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

            for i in 1..=workload_size {
                disruptor
                    .publish_event(ClosureEventTranslator::new(
                        move |event: &mut ScalingEvent, _seq: i64| {
                            event.id = std::hint::black_box(i as i64);
                            event.data = vec![i as i64; 16]; // Medium payload
                        },
                    ))
                    .unwrap();
            }

            // Wait for all events to be processed with timeout and yielding
            if !wait_for_completion_yielding(&last_id, workload_size as i64, TIMEOUT_MS) {
                panic!("Medium processing benchmark failed: events not processed within timeout");
            }

            disruptor.shutdown().unwrap();
        })
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
        b.iter(|| {
            // Create fresh Disruptor instance for each benchmark run
            let factory = DefaultEventFactory::<ScalingEvent>::new();
            let handler = ScalingHandler::new(10_000); // 10 microseconds processing delay
            let _counter = handler.get_counter();
            let last_id = handler.get_last_id();

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

            for i in 1..=workload_size {
                disruptor
                    .publish_event(ClosureEventTranslator::new(
                        move |event: &mut ScalingEvent, _seq: i64| {
                            event.id = std::hint::black_box(i as i64);
                            event.data = vec![i as i64; 64]; // Large payload
                        },
                    ))
                    .unwrap();
            }

            // Wait for all events to be processed with timeout and sleeping
            if !wait_for_completion_sleeping(&last_id, workload_size as i64, TIMEOUT_MS) {
                panic!("Slow processing benchmark failed: events not processed within timeout");
            }

            disruptor.shutdown().unwrap();
        })
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
                let success = disruptor.try_publish_event(ClosureEventTranslator::new(
                    move |event: &mut ScalingEvent, _seq: i64| {
                        event.id = std::hint::black_box(1);
                        event.data = vec![1; payload_size];
                    },
                ));

                if success {
                    // Wait for event to be processed with timeout
                    let initial_count = counter.load(Ordering::Acquire);
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
        b.iter(|| {
            // Create fresh Disruptor instance for each benchmark run
            let factory = DefaultEventFactory::<ScalingEvent>::new();
            let handler = ScalingHandler::new(5_000); // 5 microseconds delay to create backpressure
            let _counter = handler.get_counter();
            let last_id = handler.get_last_id();

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

            // Publish a burst of events
            for i in 1..=burst_size {
                // Use try_publish to test buffer utilization
                loop {
                    let success = disruptor.try_publish_event(ClosureEventTranslator::new(
                        move |event: &mut ScalingEvent, _seq: i64| {
                            event.id = std::hint::black_box(i as i64);
                            event.data = vec![i as i64; 8];
                        },
                    ));

                    if success {
                        break;
                    }
                    // Buffer is full, brief wait
                    std::hint::spin_loop();
                }
            }

            // Wait for all events to be processed with timeout
            if !wait_for_completion(&last_id, burst_size as i64, TIMEOUT_MS) {
                panic!("Buffer utilization benchmark failed: events not processed within timeout");
            }

            disruptor.shutdown().unwrap();
        })
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
