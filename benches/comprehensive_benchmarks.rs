//! Safe Comprehensive Benchmark Suite
//!
//! This is a robust benchmark suite that runs comprehensive performance
//! evaluation with proper timeout handling and error recovery.

use criterion::{criterion_group, criterion_main, Criterion};
use std::time::Duration;

// Import the fixed benchmark modules
mod fixed_benchmarks {
    use criterion::{black_box, BenchmarkId, Criterion, Throughput};
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use badbatch::disruptor::{
        event_translator::ClosureEventTranslator, BusySpinWaitStrategy, DefaultEventFactory,
        Disruptor, EventHandler, ProducerType, Result as DisruptorResult,
    };

    const TIMEOUT_MS: u64 = 3000; // 3 second timeout for quick tests

    #[derive(Debug, Default, Clone, Copy)]
    struct SafeEvent {
        value: i64,
    }

    struct SafeHandler {
        counter: Arc<AtomicI64>,
    }

    impl SafeHandler {
        fn new() -> Self {
            Self {
                counter: Arc::new(AtomicI64::new(0)),
            }
        }

        fn get_counter(&self) -> Arc<AtomicI64> {
            self.counter.clone()
        }
    }

    impl EventHandler<SafeEvent> for SafeHandler {
        fn on_event(
            &mut self,
            _event: &mut SafeEvent,
            _sequence: i64,
            _end_of_batch: bool,
        ) -> DisruptorResult<()> {
            self.counter.fetch_add(1, Ordering::Release);
            Ok(())
        }
    }

    /// Safe wait with timeout
    fn safe_wait(counter: &Arc<AtomicI64>, expected: i64, timeout_ms: u64) -> bool {
        let start = Instant::now();
        let timeout = Duration::from_millis(timeout_ms);

        while counter.load(Ordering::Acquire) < expected {
            if start.elapsed() > timeout {
                return false;
            }
            std::hint::spin_loop();
        }
        true
    }

    /// Basic SPSC performance test
    pub fn safe_spsc_test(c: &mut Criterion) {
        let mut group = c.benchmark_group("Safe_SPSC");
        group.measurement_time(Duration::from_secs(5));
        group.warm_up_time(Duration::from_secs(2));

        let factory = DefaultEventFactory::<SafeEvent>::new();
        let handler = SafeHandler::new();
        let counter = handler.get_counter();

        let mut disruptor = match Disruptor::new(
            factory,
            1024,
            ProducerType::Single,
            Box::new(BusySpinWaitStrategy::new()),
        ) {
            Ok(d) => d.handle_events_with(handler).build(),
            Err(e) => {
                eprintln!("Failed to create disruptor: {e:?}");
                return;
            }
        };

        if disruptor.start().is_err() {
            eprintln!("Failed to start disruptor");
            return;
        }

        for burst_size in [100, 1000].iter() {
            let benchmark_id = BenchmarkId::new("burst", burst_size);
            group.throughput(Throughput::Elements(*burst_size));

            group.bench_function(benchmark_id, |b| {
                b.iter_custom(|iters| {
                    let start = Instant::now();
                    for _ in 0..iters {
                        counter.store(0, Ordering::Release);

                        for i in 0..*burst_size {
                            if disruptor
                                .publish_event(ClosureEventTranslator::new(
                                    move |event: &mut SafeEvent, _seq: i64| {
                                        event.value = black_box(i as i64);
                                    },
                                ))
                                .is_err()
                            {
                                eprintln!("Failed to publish event {i}");
                                break;
                            }
                        }

                        if !safe_wait(&counter, *burst_size as i64, TIMEOUT_MS) {
                            eprintln!("Timeout waiting for {burst_size} events");
                            break;
                        }
                    }
                    start.elapsed()
                })
            });
        }

        let _ = disruptor.shutdown();
        group.finish();
    }

    /// Throughput comparison test
    pub fn safe_throughput_test(c: &mut Criterion) {
        let mut group = c.benchmark_group("Safe_Throughput");
        group.measurement_time(Duration::from_secs(3));

        // Test different buffer sizes
        for buffer_size in [256, 1024].iter() {
            let benchmark_id = BenchmarkId::new("buffer", buffer_size);

            group.bench_function(benchmark_id, |b| {
                let factory = DefaultEventFactory::<SafeEvent>::new();
                let handler = SafeHandler::new();
                let counter = handler.get_counter();

                let mut disruptor = match Disruptor::new(
                    factory,
                    *buffer_size,
                    ProducerType::Single,
                    Box::new(BusySpinWaitStrategy::new()),
                ) {
                    Ok(d) => d.handle_events_with(handler).build(),
                    Err(_) => return, // Skip this test if creation fails
                };

                if disruptor.start().is_err() {
                    return; // Skip this test if start fails
                }

                b.iter_custom(|iters| {
                    let events_per_iter = 100;
                    let start = Instant::now();

                    for _ in 0..iters {
                        counter.store(0, Ordering::Release);

                        for i in 0..events_per_iter {
                            if disruptor
                                .publish_event(ClosureEventTranslator::new(
                                    move |event: &mut SafeEvent, _seq: i64| {
                                        event.value = black_box(i);
                                    },
                                ))
                                .is_err()
                            {
                                break;
                            }
                        }

                        if !safe_wait(&counter, events_per_iter, TIMEOUT_MS) {
                            break;
                        }
                    }

                    let _ = disruptor.shutdown();
                    start.elapsed()
                })
            });
        }

        group.finish();
    }

    /// Basic latency test
    pub fn safe_latency_test(c: &mut Criterion) {
        let mut group = c.benchmark_group("Safe_Latency");
        group.measurement_time(Duration::from_secs(3));

        let factory = DefaultEventFactory::<SafeEvent>::new();
        let handler = SafeHandler::new();
        let counter = handler.get_counter();

        let mut disruptor = match Disruptor::new(
            factory,
            512,
            ProducerType::Single,
            Box::new(BusySpinWaitStrategy::new()),
        ) {
            Ok(d) => d.handle_events_with(handler).build(),
            Err(_) => return,
        };

        if disruptor.start().is_err() {
            return;
        }

        group.bench_function("single_event", |b| {
            b.iter_custom(|iters| {
                let start = Instant::now();
                for i in 0..iters {
                    counter.store(0, Ordering::Release);

                    if disruptor
                        .publish_event(ClosureEventTranslator::new(
                            move |event: &mut SafeEvent, _seq: i64| {
                                event.value = black_box(i as i64);
                            },
                        ))
                        .is_err()
                    {
                        break;
                    }

                    if !safe_wait(&counter, 1, TIMEOUT_MS) {
                        break;
                    }
                }
                start.elapsed()
            })
        });

        let _ = disruptor.shutdown();
        group.finish();
    }
}

/// Run a focused performance test suite for safe validation
pub fn safe_comprehensive_suite(c: &mut Criterion) {
    // Run individual test modules
    fixed_benchmarks::safe_spsc_test(c);
    fixed_benchmarks::safe_throughput_test(c);
    fixed_benchmarks::safe_latency_test(c);
}

// Channel comparison for baseline
pub fn channel_comparison(c: &mut Criterion) {
    use std::sync::mpsc;
    use std::thread;

    let mut group = c.benchmark_group("Channel_Baseline");
    group.measurement_time(Duration::from_secs(3));

    for burst_size in [100, 1000].iter() {
        let benchmark_id = criterion::BenchmarkId::new("std_mpsc", burst_size);

        group.bench_function(benchmark_id, |b| {
            b.iter_custom(|iters| {
                let start = std::time::Instant::now();
                for _ in 0..iters {
                    let (tx, rx) = mpsc::channel();

                    let producer = thread::spawn(move || {
                        for i in 0..*burst_size {
                            if tx.send(i).is_err() {
                                break;
                            }
                        }
                    });

                    let consumer = thread::spawn(move || {
                        let mut count = 0;
                        while count < *burst_size {
                            if rx.recv().is_ok() {
                                count += 1;
                            } else {
                                break;
                            }
                        }
                    });

                    let _ = producer.join();
                    let _ = consumer.join();
                }
                start.elapsed()
            })
        });
    }

    group.finish();
}

criterion_group!(
    name = safe;
    config = Criterion::default()
        .measurement_time(Duration::from_secs(5))
        .warm_up_time(Duration::from_secs(2))
        .sample_size(15); // Reasonable sample size
    targets = safe_comprehensive_suite, channel_comparison
);

criterion_main!(safe);
