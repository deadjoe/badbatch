//! Buffer Size Scaling Benchmarks
//!
//! This benchmark suite tests how performance scales with different buffer sizes
//! and identifies optimal buffer configurations for different workloads.

use criterion::measurement::WallTime;
use criterion::{
    black_box, criterion_group, criterion_main, BenchmarkGroup, BenchmarkId, Criterion, Throughput,
};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use badbatch::disruptor::{
    event_translator::ClosureEventTranslator, BusySpinWaitStrategy, DefaultEventFactory, Disruptor,
    EventHandler, ProducerType, Result as DisruptorResult, YieldingWaitStrategy,
};

// Buffer size configurations to test
const BUFFER_SIZES: [usize; 8] = [64, 128, 256, 512, 1024, 2048, 4096, 8192];
const WORKLOAD_SIZES: [u64; 3] = [1_000, 5_000, 20_000];

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
        black_box(sum);

        self.last_id.store(event.id, Ordering::Release);
        self.counter.fetch_add(1, Ordering::Release);
        Ok(())
    }
}

/// Test scaling with fast processing (minimal per-event work)
fn benchmark_fast_processing_scaling(
    group: &mut BenchmarkGroup<WallTime>,
    buffer_size: usize,
    workload_size: u64,
) {
    let factory = DefaultEventFactory::<ScalingEvent>::new();
    let handler = ScalingHandler::new(0); // No artificial processing delay
    let counter = handler.get_counter();
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

    let param = format!("fast_buf{}_work{}", buffer_size, workload_size);
    let benchmark_id = BenchmarkId::new("FastProcessing", param);

    group.throughput(Throughput::Elements(workload_size));
    group.bench_function(benchmark_id, |b| {
        b.iter_custom(|iters| {
            let start = Instant::now();
            for _ in 0..iters {
                counter.store(0, Ordering::Release);
                last_id.store(0, Ordering::Release);

                for i in 1..=workload_size {
                    disruptor
                        .publish_event(ClosureEventTranslator::new(
                            move |event: &mut ScalingEvent, _seq: i64| {
                                event.id = black_box(i as i64);
                                event.data = vec![i as i64; 4]; // Small payload
                            },
                        ))
                        .unwrap();
                }

                // Wait for all events to be processed
                while last_id.load(Ordering::Acquire) < workload_size as i64 {
                    std::hint::spin_loop();
                }
            }
            start.elapsed()
        })
    });

    disruptor.shutdown().unwrap();
}

/// Test scaling with medium processing (moderate per-event work)
fn benchmark_medium_processing_scaling(
    group: &mut BenchmarkGroup<WallTime>,
    buffer_size: usize,
    workload_size: u64,
) {
    let factory = DefaultEventFactory::<ScalingEvent>::new();
    let handler = ScalingHandler::new(1_000); // 1 microsecond processing delay
    let counter = handler.get_counter();
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

    let param = format!("medium_buf{}_work{}", buffer_size, workload_size);
    let benchmark_id = BenchmarkId::new("MediumProcessing", param);

    group.throughput(Throughput::Elements(workload_size));
    group.bench_function(benchmark_id, |b| {
        b.iter_custom(|iters| {
            let start = Instant::now();
            for _ in 0..iters {
                counter.store(0, Ordering::Release);
                last_id.store(0, Ordering::Release);

                for i in 1..=workload_size {
                    disruptor
                        .publish_event(ClosureEventTranslator::new(
                            move |event: &mut ScalingEvent, _seq: i64| {
                                event.id = black_box(i as i64);
                                event.data = vec![i as i64; 16]; // Medium payload
                            },
                        ))
                        .unwrap();
                }

                // Wait for all events to be processed
                while last_id.load(Ordering::Acquire) < workload_size as i64 {
                    std::thread::yield_now();
                }
            }
            start.elapsed()
        })
    });

    disruptor.shutdown().unwrap();
}

/// Test scaling with slow processing (significant per-event work)
fn benchmark_slow_processing_scaling(
    group: &mut BenchmarkGroup<WallTime>,
    buffer_size: usize,
    workload_size: u64,
) {
    let factory = DefaultEventFactory::<ScalingEvent>::new();
    let handler = ScalingHandler::new(10_000); // 10 microseconds processing delay
    let counter = handler.get_counter();
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

    let param = format!("slow_buf{}_work{}", buffer_size, workload_size);
    let benchmark_id = BenchmarkId::new("SlowProcessing", param);

    group.throughput(Throughput::Elements(workload_size));
    group.bench_function(benchmark_id, |b| {
        b.iter_custom(|iters| {
            let start = Instant::now();
            for _ in 0..iters {
                counter.store(0, Ordering::Release);
                last_id.store(0, Ordering::Release);

                for i in 1..=workload_size {
                    disruptor
                        .publish_event(ClosureEventTranslator::new(
                            move |event: &mut ScalingEvent, _seq: i64| {
                                event.id = black_box(i as i64);
                                event.data = vec![i as i64; 64]; // Large payload
                            },
                        ))
                        .unwrap();
                }

                // Wait for all events to be processed
                while last_id.load(Ordering::Acquire) < workload_size as i64 {
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
            start.elapsed()
        })
    });

    disruptor.shutdown().unwrap();
}

/// Test memory usage scaling with different buffer sizes
fn benchmark_memory_scaling(group: &mut BenchmarkGroup<WallTime>, buffer_size: usize) {
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

    let benchmark_id = BenchmarkId::new("MemoryUsage", buffer_size);

    group.bench_function(benchmark_id, |b| {
        b.iter(|| {
            // Test memory allocation patterns by creating events with different payload sizes
            for payload_size in [1, 10, 100] {
                let success = disruptor.try_publish_event(ClosureEventTranslator::new(
                    move |event: &mut ScalingEvent, _seq: i64| {
                        event.id = black_box(1);
                        event.data = vec![1; payload_size];
                    },
                ));

                if success {
                    // Wait for event to be processed
                    let initial_count = counter.load(Ordering::Acquire);
                    while counter.load(Ordering::Acquire) <= initial_count {
                        std::hint::spin_loop();
                    }
                }
            }
        })
    });

    disruptor.shutdown().unwrap();
}

/// Test buffer utilization patterns
fn benchmark_buffer_utilization(
    group: &mut BenchmarkGroup<WallTime>,
    buffer_size: usize,
    burst_size: u64,
) {
    let factory = DefaultEventFactory::<ScalingEvent>::new();
    let handler = ScalingHandler::new(5_000); // 5 microseconds delay to create backpressure
    let counter = handler.get_counter();
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

    let param = format!("util_buf{}_burst{}", buffer_size, burst_size);
    let benchmark_id = BenchmarkId::new("BufferUtil", param);

    group.throughput(Throughput::Elements(burst_size));
    group.bench_function(benchmark_id, |b| {
        b.iter_custom(|iters| {
            let start = Instant::now();
            for _ in 0..iters {
                counter.store(0, Ordering::Release);
                last_id.store(0, Ordering::Release);

                // Publish a burst of events
                for i in 1..=burst_size {
                    // Use try_publish to test buffer utilization
                    loop {
                        let success = disruptor.try_publish_event(ClosureEventTranslator::new(
                            move |event: &mut ScalingEvent, _seq: i64| {
                                event.id = black_box(i as i64);
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

                // Wait for all events to be processed
                while last_id.load(Ordering::Acquire) < burst_size as i64 {
                    std::hint::spin_loop();
                }
            }
            start.elapsed()
        })
    });

    disruptor.shutdown().unwrap();
}

/// Main buffer size scaling benchmark function
pub fn buffer_scaling_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("BufferScaling");

    // Configure benchmark group
    group.measurement_time(Duration::from_secs(20));
    group.warm_up_time(Duration::from_secs(5));

    // Test fast processing scaling across all buffer sizes and workloads
    for &buffer_size in BUFFER_SIZES.iter() {
        for &workload_size in WORKLOAD_SIZES.iter() {
            // Skip large combinations to keep benchmark time reasonable
            if buffer_size > 2048 && workload_size > 5_000 {
                continue;
            }

            benchmark_fast_processing_scaling(&mut group, buffer_size, workload_size);
        }
    }

    // Test medium processing scaling for selected configurations
    for &buffer_size in [256, 1024, 4096].iter() {
        for &workload_size in [1_000, 5_000].iter() {
            benchmark_medium_processing_scaling(&mut group, buffer_size, workload_size);
        }
    }

    // Test slow processing scaling for selected configurations
    for &buffer_size in [512, 2048].iter() {
        benchmark_slow_processing_scaling(&mut group, buffer_size, 1_000);
    }

    // Test memory usage for all buffer sizes
    for &buffer_size in BUFFER_SIZES.iter() {
        benchmark_memory_scaling(&mut group, buffer_size);
    }

    // Test buffer utilization patterns
    for &buffer_size in [256, 1024, 4096].iter() {
        for &burst_size in [100, 500].iter() {
            // Only test combinations where burst is smaller than buffer
            if burst_size < buffer_size as u64 {
                benchmark_buffer_utilization(&mut group, buffer_size, burst_size);
            }
        }
    }

    group.finish();
}

criterion_group!(buffer_scaling, buffer_scaling_benchmark);
criterion_main!(buffer_scaling);
