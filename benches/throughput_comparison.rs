//! Throughput Comparison Benchmarks
//!
//! This benchmark suite compares the raw throughput performance of different
//! configurations and compares against other concurrency primitives.

use criterion::measurement::WallTime;
use criterion::{
    black_box, criterion_group, criterion_main, BenchmarkGroup, BenchmarkId, Criterion, Throughput,
};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

use badbatch::disruptor::{
    event_translator::ClosureEventTranslator, BlockingWaitStrategy, BusySpinWaitStrategy,
    DefaultEventFactory, Disruptor, EventHandler, ProducerType, Result as DisruptorResult,
    YieldingWaitStrategy,
};

// Benchmark configuration constants
const SMALL_BUFFER: usize = 256;
const MEDIUM_BUFFER: usize = 1024;
const LARGE_BUFFER: usize = 4096;
const THROUGHPUT_EVENTS: u64 = 5_000; // Reduced from 10_000 to improve speed

#[derive(Debug, Default, Clone, Copy)]
struct ThroughputEvent {
    id: i64,
    data: [i64; 4], // Some payload to make events more realistic
}

/// High-performance event handler that just counts events
struct ThroughputHandler {
    counter: Arc<AtomicI64>,
}

impl ThroughputHandler {
    fn new() -> Self {
        Self {
            counter: Arc::new(AtomicI64::new(0)),
        }
    }

    fn get_counter(&self) -> Arc<AtomicI64> {
        self.counter.clone()
    }
}

impl EventHandler<ThroughputEvent> for ThroughputHandler {
    fn on_event(
        &mut self,
        event: &mut ThroughputEvent,
        _sequence: i64,
        _end_of_batch: bool,
    ) -> DisruptorResult<()> {
        // Minimal processing - just ensure event is not optimized away
        black_box(event.data[0]);
        self.counter.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

/// Benchmark Disruptor throughput with different buffer sizes and wait strategies
fn benchmark_disruptor_throughput(
    group: &mut BenchmarkGroup<WallTime>,
    buffer_size: usize,
    wait_strategy: &str,
    producer_type: ProducerType,
) {
    let factory = DefaultEventFactory::<ThroughputEvent>::new();
    let handler = ThroughputHandler::new();
    let counter = handler.get_counter();

    let wait_strategy_impl: Box<dyn badbatch::disruptor::WaitStrategy> = match wait_strategy {
        "BusySpin" => Box::new(BusySpinWaitStrategy::new()),
        "Yielding" => Box::new(YieldingWaitStrategy::new()),
        "Blocking" => Box::new(BlockingWaitStrategy::new()),
        _ => Box::new(BusySpinWaitStrategy::new()),
    };

    let mut disruptor = Disruptor::new(factory, buffer_size, producer_type, wait_strategy_impl)
        .unwrap()
        .handle_events_with(handler)
        .build();

    disruptor.start().unwrap();

    let producer_str = match producer_type {
        ProducerType::Single => "SP",
        ProducerType::Multi => "MP",
    };

    let param = format!("{producer_str}_{wait_strategy}_buf{buffer_size}");
    let benchmark_id = BenchmarkId::new("Disruptor", param);

    group.throughput(Throughput::Elements(THROUGHPUT_EVENTS));
    group.bench_function(benchmark_id, |b| {
        b.iter_custom(|iters| {
            let start = Instant::now();
            for _ in 0..iters {
                counter.store(0, Ordering::Release);

                for i in 0..THROUGHPUT_EVENTS {
                    // Use blocking publish for maximum throughput measurement
                    disruptor
                        .publish_event(ClosureEventTranslator::new(
                            move |event: &mut ThroughputEvent, _seq: i64| {
                                event.id = black_box(i as i64);
                                event.data = [i as i64; 4];
                            },
                        ))
                        .unwrap();
                }

                // Wait for all events to be processed
                while counter.load(Ordering::Acquire) < THROUGHPUT_EVENTS as i64 {
                    std::hint::spin_loop();
                }
            }
            start.elapsed()
        })
    });

    disruptor.shutdown().unwrap();
}

/// Benchmark std::sync::mpsc channel throughput
fn benchmark_mpsc_throughput(group: &mut BenchmarkGroup<WallTime>, buffer_size: usize) {
    let param = format!("buf{buffer_size}");
    let benchmark_id = BenchmarkId::new("MPSC", param);

    group.throughput(Throughput::Elements(THROUGHPUT_EVENTS));
    group.bench_function(benchmark_id, |b| {
        b.iter_custom(|iters| {
            let start = Instant::now();
            for _ in 0..iters {
                // Create fresh channel and thread for each iteration
                let (sender, receiver) = mpsc::sync_channel(buffer_size);
                let counter = Arc::new(AtomicI64::new(0));

                let counter_clone = counter.clone();
                let receiver_handle = thread::spawn(move || {
                    let mut processed = 0;
                    while processed < THROUGHPUT_EVENTS {
                        if let Ok(event) = receiver.recv() {
                            // Minimal processing to match Disruptor handler
                            let _event: ThroughputEvent = event;
                            black_box(_event.data[0]);

                            processed += 1;
                            counter_clone.store(processed as i64, Ordering::Relaxed);
                        } else {
                            // Channel closed, exit
                            break;
                        }
                    }
                });

                for i in 0..THROUGHPUT_EVENTS {
                    let event = ThroughputEvent {
                        id: black_box(i as i64),
                        data: [i as i64; 4],
                    };
                    if sender.send(event).is_err() {
                        // Channel closed, stop sending
                        break;
                    }
                }

                // Wait for all events to be processed
                while counter.load(Ordering::Acquire) < THROUGHPUT_EVENTS as i64 {
                    std::hint::spin_loop();
                }

                // Close sender to signal receiver to stop
                drop(sender);
                receiver_handle.join().unwrap();
            }
            start.elapsed()
        })
    });
}

/// Benchmark crossbeam channel throughput
fn benchmark_crossbeam_throughput(group: &mut BenchmarkGroup<WallTime>, buffer_size: usize) {
    let param = format!("buf{buffer_size}");
    let benchmark_id = BenchmarkId::new("Crossbeam", param);

    group.throughput(Throughput::Elements(THROUGHPUT_EVENTS));
    group.bench_function(benchmark_id, |b| {
        b.iter_custom(|iters| {
            let start = Instant::now();
            for _ in 0..iters {
                // Create fresh channel and thread for each iteration
                let (sender, receiver) = crossbeam::channel::bounded(buffer_size);
                let counter = Arc::new(AtomicI64::new(0));

                let counter_clone = counter.clone();
                let receiver_handle = thread::spawn(move || {
                    let mut processed = 0;
                    while processed < THROUGHPUT_EVENTS {
                        if let Ok(event) = receiver.recv() {
                            // Minimal processing to match Disruptor handler
                            let _event: ThroughputEvent = event;
                            black_box(_event.data[0]);

                            processed += 1;
                            counter_clone.store(processed as i64, Ordering::Relaxed);
                        } else {
                            // Channel closed, exit
                            break;
                        }
                    }
                });

                for i in 0..THROUGHPUT_EVENTS {
                    let event = ThroughputEvent {
                        id: black_box(i as i64),
                        data: [i as i64; 4],
                    };
                    if sender.send(event).is_err() {
                        // Channel closed, stop sending
                        break;
                    }
                }

                // Wait for all events to be processed
                while counter.load(Ordering::Acquire) < THROUGHPUT_EVENTS as i64 {
                    std::hint::spin_loop();
                }

                // Close sender to signal receiver to stop
                drop(sender);
                receiver_handle.join().unwrap();
            }
            start.elapsed()
        })
    });
}

/// Benchmark try_publish performance for non-blocking scenarios
fn benchmark_try_publish_throughput(group: &mut BenchmarkGroup<WallTime>, buffer_size: usize) {
    let factory = DefaultEventFactory::<ThroughputEvent>::new();
    let handler = ThroughputHandler::new();
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

    let param = format!("try_publish_buf{buffer_size}");
    let benchmark_id = BenchmarkId::new("Disruptor", param);

    group.throughput(Throughput::Elements(THROUGHPUT_EVENTS));
    group.bench_function(benchmark_id, |b| {
        b.iter_custom(|iters| {
            let start = Instant::now();
            for _ in 0..iters {
                counter.store(0, Ordering::Release);

                let mut published = 0;
                while published < THROUGHPUT_EVENTS {
                    let success = disruptor.try_publish_event(ClosureEventTranslator::new(
                        move |event: &mut ThroughputEvent, _seq: i64| {
                            event.id = black_box(published as i64);
                            event.data = [published as i64; 4];
                        },
                    ));

                    if success {
                        published += 1;
                    } else {
                        // Brief pause to allow consumer to catch up
                        std::hint::spin_loop();
                    }
                }

                // Wait for all events to be processed
                while counter.load(Ordering::Acquire) < THROUGHPUT_EVENTS as i64 {
                    std::hint::spin_loop();
                }
            }
            start.elapsed()
        })
    });

    disruptor.shutdown().unwrap();
}

/// Main throughput comparison benchmark function
pub fn throughput_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("Throughput");

    // Configure benchmark group for throughput measurement
    group.measurement_time(Duration::from_secs(15));
    group.warm_up_time(Duration::from_secs(5));

    // Test different buffer sizes
    let buffer_sizes = [SMALL_BUFFER, MEDIUM_BUFFER, LARGE_BUFFER];

    for &buffer_size in buffer_sizes.iter() {
        // Test Disruptor with different configurations
        benchmark_disruptor_throughput(&mut group, buffer_size, "BusySpin", ProducerType::Single);
        benchmark_disruptor_throughput(&mut group, buffer_size, "Yielding", ProducerType::Single);

        // Test multi-producer only for medium buffer size to reduce benchmark time
        if buffer_size == MEDIUM_BUFFER {
            benchmark_disruptor_throughput(
                &mut group,
                buffer_size,
                "BusySpin",
                ProducerType::Multi,
            );
            benchmark_disruptor_throughput(
                &mut group,
                buffer_size,
                "Yielding",
                ProducerType::Multi,
            );
        }

        // Test blocking strategy only for smaller buffers (it's typically slower)
        if buffer_size <= MEDIUM_BUFFER {
            benchmark_disruptor_throughput(
                &mut group,
                buffer_size,
                "Blocking",
                ProducerType::Single,
            );
        }

        // Test try_publish performance
        if buffer_size == MEDIUM_BUFFER {
            benchmark_try_publish_throughput(&mut group, buffer_size);
        }

        // Compare against standard channels
        benchmark_mpsc_throughput(&mut group, buffer_size);
        benchmark_crossbeam_throughput(&mut group, buffer_size);
    }

    group.finish();
}

criterion_group!(throughput, throughput_benchmark);
criterion_main!(throughput);
