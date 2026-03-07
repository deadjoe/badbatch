#![allow(
    missing_docs,
    clippy::all,
    clippy::pedantic,
    clippy::nursery,
    clippy::cargo
)]

//! Throughput Comparison Benchmarks
//!
//! This benchmark suite compares the raw throughput performance of different
//! configurations and compares against other concurrency primitives.

use criterion::measurement::WallTime;
use criterion::{
    criterion_group, criterion_main, BenchmarkGroup, BenchmarkId, Criterion, Throughput,
};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

use badbatch::disruptor::{
    build_multi_producer, build_single_producer, event_translator::ClosureEventTranslator,
    BlockingWaitStrategy, BusySpinWaitStrategy, DefaultEventFactory, Disruptor, EventHandler,
    ProducerType, Result as DisruptorResult, YieldingWaitStrategy,
};

// Benchmark configuration constants
const SMALL_BUFFER: usize = 256;
const MEDIUM_BUFFER: usize = 1024;
const LARGE_BUFFER: usize = 4096;
const THROUGHPUT_EVENTS: u64 = 10_000; // Standardized event count for consistent measurement

#[derive(Debug, Default, Clone, Copy)]
struct ThroughputEvent {
    id: i64,
    data: [i64; 4], // Some payload to make events more realistic
}

/// High-performance event handler that just counts events
struct ThroughputHandler {
    counter: Arc<AtomicI64>,
}

fn batch_chunk_size(buffer_size: usize) -> usize {
    buffer_size.min(256)
}

fn counting_handler(
    counter: &Arc<AtomicI64>,
) -> impl FnMut(&mut ThroughputEvent, i64, bool) + Send + Sync + 'static {
    let counter = Arc::clone(counter);
    move |event: &mut ThroughputEvent, _sequence, _end_of_batch| {
        std::hint::black_box(event.data[0]);
        counter.fetch_add(1, Ordering::Relaxed);
    }
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
        std::hint::black_box(event.data[0]);
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
    let counter = Arc::new(AtomicI64::new(0));
    let mut disruptor = match (producer_type, wait_strategy) {
        (ProducerType::Single, "BusySpin") => build_single_producer(
            buffer_size,
            ThroughputEvent::default,
            BusySpinWaitStrategy::new(),
        )
        .handle_events_with(counting_handler(&counter))
        .build(),
        (ProducerType::Single, "Yielding") => build_single_producer(
            buffer_size,
            ThroughputEvent::default,
            YieldingWaitStrategy::new(),
        )
        .handle_events_with(counting_handler(&counter))
        .build(),
        (ProducerType::Single, "Blocking") => build_single_producer(
            buffer_size,
            ThroughputEvent::default,
            BlockingWaitStrategy::new(),
        )
        .handle_events_with(counting_handler(&counter))
        .build(),
        (ProducerType::Multi, "BusySpin") => build_multi_producer(
            buffer_size,
            ThroughputEvent::default,
            BusySpinWaitStrategy::new(),
        )
        .handle_events_with(counting_handler(&counter))
        .build(),
        (ProducerType::Multi, "Yielding") => build_multi_producer(
            buffer_size,
            ThroughputEvent::default,
            YieldingWaitStrategy::new(),
        )
        .handle_events_with(counting_handler(&counter))
        .build(),
        (ProducerType::Multi, "Blocking") => build_multi_producer(
            buffer_size,
            ThroughputEvent::default,
            BlockingWaitStrategy::new(),
        )
        .handle_events_with(counting_handler(&counter))
        .build(),
        _ => unreachable!("unsupported producer/wait-strategy combination"),
    };

    let producer_str = match producer_type {
        ProducerType::Single => "SP",
        // Note: this selects the multi-producer *sequencer*; this benchmark still publishes
        // from a single thread. True multi-threaded producer benchmarks live in
        // `benches/multi_producer_single_consumer.rs`.
        ProducerType::Multi => "MS",
    };

    let param = format!("Batch_{producer_str}_{wait_strategy}_buf{buffer_size}");
    let benchmark_id = BenchmarkId::new("Disruptor", param);
    let batch_size = batch_chunk_size(buffer_size);

    group.throughput(Throughput::Elements(THROUGHPUT_EVENTS));
    group.bench_function(benchmark_id, |b| {
        b.iter_custom(|iters| {
            let start = Instant::now();
            for _ in 0..iters {
                // Capture completion state before publishing so this iteration waits
                // for its own batch instead of racing with the previous one.
                let start_count = counter.load(Ordering::Relaxed);
                let target = start_count + THROUGHPUT_EVENTS as i64;

                let mut published = 0usize;
                while published < THROUGHPUT_EVENTS as usize {
                    let chunk_len = (THROUGHPUT_EVENTS as usize - published).min(batch_size);
                    let chunk_start = published;
                    disruptor.batch_publish(chunk_len, |iter| {
                        for (index, event) in iter.enumerate() {
                            let value = (chunk_start + index) as i64;
                            event.id = std::hint::black_box(value);
                            event.data = [value; 4];
                        }
                    });
                    published += chunk_len;
                }

                // Wait for all events to be processed
                while counter.load(Ordering::Relaxed) < target {
                    std::hint::spin_loop();
                }
            }
            start.elapsed()
        })
    });

    disruptor.shutdown();
}

/// Benchmark std::sync::mpsc channel throughput
fn benchmark_mpsc_throughput(group: &mut BenchmarkGroup<WallTime>, buffer_size: usize) {
    let param = format!("buf{buffer_size}");
    let benchmark_id = BenchmarkId::new("MPSC", param);

    group.throughput(Throughput::Elements(THROUGHPUT_EVENTS));
    group.bench_function(benchmark_id, |b| {
        let (sender, receiver) = mpsc::sync_channel::<ThroughputEvent>(buffer_size);
        let processed_total = Arc::new(AtomicU64::new(0));

        let processed_total_clone = Arc::clone(&processed_total);
        let receiver_handle = thread::spawn(move || {
            while let Ok(event) = receiver.recv() {
                std::hint::black_box(event.data[0]);
                processed_total_clone.fetch_add(1, Ordering::Relaxed);
            }
        });

        b.iter_custom(|iters| {
            let start = Instant::now();
            for _ in 0..iters {
                let start_processed = processed_total.load(Ordering::Relaxed);
                let target = start_processed + THROUGHPUT_EVENTS;

                for i in 0..THROUGHPUT_EVENTS {
                    let event = ThroughputEvent {
                        id: std::hint::black_box(i as i64),
                        data: [i as i64; 4],
                    };
                    sender.send(event).unwrap();
                }

                while processed_total.load(Ordering::Relaxed) < target {
                    std::hint::spin_loop();
                }
            }
            start.elapsed()
        });

        drop(sender);
        receiver_handle.join().unwrap();
    });
}

/// Benchmark crossbeam channel throughput
fn benchmark_crossbeam_throughput(group: &mut BenchmarkGroup<WallTime>, buffer_size: usize) {
    let param = format!("buf{buffer_size}");
    let benchmark_id = BenchmarkId::new("Crossbeam", param);

    group.throughput(Throughput::Elements(THROUGHPUT_EVENTS));
    group.bench_function(benchmark_id, |b| {
        let (sender, receiver) = crossbeam::channel::bounded::<ThroughputEvent>(buffer_size);
        let processed_total = Arc::new(AtomicU64::new(0));

        let processed_total_clone = Arc::clone(&processed_total);
        let receiver_handle = thread::spawn(move || {
            while let Ok(event) = receiver.recv() {
                std::hint::black_box(event.data[0]);
                processed_total_clone.fetch_add(1, Ordering::Relaxed);
            }
        });

        b.iter_custom(|iters| {
            let start = Instant::now();
            for _ in 0..iters {
                let start_processed = processed_total.load(Ordering::Relaxed);
                let target = start_processed + THROUGHPUT_EVENTS;

                for i in 0..THROUGHPUT_EVENTS {
                    let event = ThroughputEvent {
                        id: std::hint::black_box(i as i64),
                        data: [i as i64; 4],
                    };
                    sender.send(event).unwrap();
                }

                while processed_total.load(Ordering::Relaxed) < target {
                    std::hint::spin_loop();
                }
            }
            start.elapsed()
        });

        drop(sender);
        receiver_handle.join().unwrap();
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
                let start_count = counter.load(Ordering::Relaxed);
                let target = start_count + THROUGHPUT_EVENTS as i64;

                let mut published = 0;
                while published < THROUGHPUT_EVENTS {
                    let success = disruptor.try_publish_event(ClosureEventTranslator::new(
                        move |event: &mut ThroughputEvent, _seq: i64| {
                            event.id = std::hint::black_box(published as i64);
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
                while counter.load(Ordering::Relaxed) < target {
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
    group.measurement_time(Duration::from_secs(10));
    group.warm_up_time(Duration::from_secs(3));

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
