#![allow(
    missing_docs,
    clippy::all,
    clippy::pedantic,
    clippy::nursery,
    clippy::cargo
)]

//! Fixed Multi Producer Single Consumer (MPSC) Benchmarks
//!
//! This benchmark suite tests the performance of the BadBatch Disruptor
//! in multi producer, single consumer scenarios with proper synchronization.

use criterion::measurement::WallTime;
use criterion::{
    criterion_group, criterion_main, BenchmarkGroup, BenchmarkId, Criterion, Throughput,
};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use badbatch::disruptor::{
    build_multi_producer, event_translator::ClosureEventTranslator, BusySpinWaitStrategy,
    DefaultEventFactory, Disruptor, EventHandler, Producer, ProducerType,
    Result as DisruptorResult,
};

// Benchmark configuration constants
const PRODUCER_COUNT: usize = 3;
const BUFFER_SIZE: usize = 1024; // Standard buffer size per README.md
const BURST_SIZES: [u64; 3] = [10, 100, 500];
const PAUSE_MS: [u64; 2] = [0, 1]; // Reduced pause times
const TIMEOUT_MS: u64 = 10000; // 10 second timeout

#[derive(Debug, Default, Clone, Copy)]
struct MPSCEvent {
    producer_id: usize,
    value: i64,
    sequence: i64,
}

/// Event handler that counts processed events from all producers
struct MPSCCountingSink {
    event_count: Arc<AtomicI64>,
}

impl MPSCCountingSink {
    fn new() -> Self {
        Self {
            event_count: Arc::new(AtomicI64::new(0)),
        }
    }

    fn get_event_count(&self) -> Arc<AtomicI64> {
        Arc::clone(&self.event_count)
    }
}

impl EventHandler<MPSCEvent> for MPSCCountingSink {
    fn on_event(
        &mut self,
        event: &mut MPSCEvent,
        _sequence: i64,
        _end_of_batch: bool,
    ) -> DisruptorResult<()> {
        std::hint::black_box(event.value);
        // Increment total event count
        self.event_count.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

fn pause(millis: u64) {
    if millis > 0 {
        thread::sleep(Duration::from_millis(millis));
    }
}

fn wait_until_counter_reaches(counter: &AtomicI64, target: i64, timeout: Duration) {
    let start = Instant::now();
    while counter.load(Ordering::Relaxed) < target {
        if start.elapsed() > timeout {
            let current = counter.load(Ordering::Relaxed);
            panic!("MPSC benchmark timed out: expected >= {target}, got {current}");
        }
        std::hint::spin_loop();
    }
}

/// Persistent producer thread that publishes one burst per generation.
struct BurstProducer {
    join_handle: Option<JoinHandle<()>>,
    stop_flag: Arc<AtomicBool>,
}

impl BurstProducer {
    fn new(
        generation: Arc<AtomicUsize>,
        stop_flag: Arc<AtomicBool>,
        mut produce_one_burst: impl FnMut() + Send + 'static,
    ) -> Self {
        let stop_flag_for_thread = Arc::clone(&stop_flag);
        let join_handle = thread::spawn(move || {
            // Important: start from a fixed generation so producers cannot "miss" the first
            // generation if the main thread increments before this thread is scheduled.
            let mut last_seen_generation = 0usize;
            while !stop_flag_for_thread.load(Ordering::Acquire) {
                let current_generation = generation.load(Ordering::Acquire);
                if current_generation != last_seen_generation {
                    last_seen_generation = current_generation;
                    produce_one_burst();
                } else {
                    std::hint::spin_loop();
                }
            }
        });

        Self {
            join_handle: Some(join_handle),
            stop_flag,
        }
    }

    fn stop(&mut self) {
        self.stop_flag.store(true, Ordering::Release);
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.join(); // Don't panic on producer thread errors
        }
    }
}

impl Drop for BurstProducer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Baseline measurement for MPSC overhead
fn baseline_mpsc_measurement(group: &mut BenchmarkGroup<WallTime>, burst_size: u64) {
    let benchmark_id = BenchmarkId::new("baseline", burst_size);
    let expected_total = (burst_size * PRODUCER_COUNT as u64) as i64;

    group.throughput(Throughput::Elements(expected_total as u64));
    group.bench_function(benchmark_id, |b| {
        let generation = Arc::new(AtomicUsize::new(0));
        let stop_flag = Arc::new(AtomicBool::new(false));
        let event_counter = Arc::new(AtomicI64::new(0));

        let mut producers: Vec<BurstProducer> = (0..PRODUCER_COUNT)
            .map(|_| {
                let generation = Arc::clone(&generation);
                let stop_flag = Arc::clone(&stop_flag);
                let event_counter = Arc::clone(&event_counter);

                BurstProducer::new(generation, stop_flag, move || {
                    for _ in 0..burst_size {
                        event_counter.fetch_add(1, Ordering::Relaxed);
                        std::hint::black_box(());
                    }
                })
            })
            .collect();

        b.iter_custom(|iters| {
            let start = Instant::now();
            for _ in 0..iters {
                let start_count = event_counter.load(Ordering::Relaxed);
                let target = start_count + expected_total;
                generation.fetch_add(1, Ordering::Release);
                wait_until_counter_reaches(
                    &event_counter,
                    target,
                    Duration::from_millis(TIMEOUT_MS),
                );
            }
            start.elapsed()
        });

        // Clean up producers
        producers.iter_mut().for_each(BurstProducer::stop);
    });
}

/// Benchmark MPSC with BusySpinWaitStrategy  
/// Uses persistent producer threads to avoid measuring thread setup/teardown.
fn benchmark_mpsc_busy_spin(group: &mut BenchmarkGroup<WallTime>, burst_size: u64, pause_ms: u64) {
    let param = format!("burst:{burst_size}_pause:{pause_ms}ms");
    let benchmark_id = BenchmarkId::new("MPSC_SingleEvent_BusySpin", param);
    let expected_total = (burst_size * PRODUCER_COUNT as u64) as i64;

    group.throughput(Throughput::Elements(expected_total as u64));
    group.bench_function(benchmark_id, |b| {
        let factory = DefaultEventFactory::<MPSCEvent>::new();
        let handler = MPSCCountingSink::new();
        let event_counter = handler.get_event_count();

        let mut disruptor = Disruptor::new(
            factory,
            BUFFER_SIZE,
            ProducerType::Multi,
            Box::new(BusySpinWaitStrategy::new()),
        )
        .unwrap()
        .handle_events_with(handler)
        .build();

        disruptor.start().unwrap();
        let disruptor_arc = Arc::new(disruptor);

        let generation = Arc::new(AtomicUsize::new(0));
        let stop_flag = Arc::new(AtomicBool::new(false));

        let mut producers: Vec<BurstProducer> = (0..PRODUCER_COUNT)
            .map(|producer_id| {
                let disruptor = Arc::clone(&disruptor_arc);
                let generation = Arc::clone(&generation);
                let stop_flag = Arc::clone(&stop_flag);
                let stop_flag_for_burst = Arc::clone(&stop_flag);

                BurstProducer::new(generation, stop_flag, move || {
                    for i in 1..=burst_size {
                        while !disruptor.try_publish_event(ClosureEventTranslator::new(
                            move |event: &mut MPSCEvent, seq: i64| {
                                event.producer_id = producer_id;
                                event.value = std::hint::black_box(i as i64);
                                event.sequence = seq;
                            },
                        )) {
                            if stop_flag_for_burst.load(Ordering::Relaxed) {
                                return;
                            }
                            std::hint::spin_loop();
                        }
                    }
                })
            })
            .collect();

        b.iter_custom(|iters| {
            pause(pause_ms);
            let start = Instant::now();

            for _ in 0..iters {
                let start_count = event_counter.load(Ordering::Relaxed);
                let target = start_count + expected_total;
                generation.fetch_add(1, Ordering::Release);

                wait_until_counter_reaches(
                    &event_counter,
                    target,
                    Duration::from_millis(TIMEOUT_MS),
                );
            }

            start.elapsed()
        });

        // Clean up producers and disruptor
        producers.iter_mut().for_each(BurstProducer::stop);

        // Properly shutdown the disruptor
        if let Ok(mut disruptor) = Arc::try_unwrap(disruptor_arc) {
            disruptor.shutdown().unwrap();
        }
    });
}

/// Reference-aligned MPSC benchmark that uses one batch claim per producer burst.
///
/// This matches the publication model used by LMAX range-claim throughput tests and the
/// disruptor-rs Criterion benches more closely than the per-event translator API.
fn benchmark_mpsc_batch_busy_spin(
    group: &mut BenchmarkGroup<WallTime>,
    burst_size: u64,
    pause_ms: u64,
) {
    let param = format!("batch_burst:{burst_size}_pause:{pause_ms}ms");
    let benchmark_id = BenchmarkId::new("MPSC_Batch_BusySpin", param);
    let expected_total = (burst_size * PRODUCER_COUNT as u64) as i64;

    group.throughput(Throughput::Elements(expected_total as u64));
    group.bench_function(benchmark_id, |b| {
        let event_counter = Arc::new(AtomicI64::new(0));
        let mut disruptor =
            build_multi_producer(BUFFER_SIZE, MPSCEvent::default, BusySpinWaitStrategy::new())
                .handle_events_with({
                    let event_counter = Arc::clone(&event_counter);
                    move |event: &mut MPSCEvent, _sequence, _end_of_batch| {
                        std::hint::black_box(event.value);
                        event_counter.fetch_add(1, Ordering::Relaxed);
                    }
                })
                .build();

        let generation = Arc::new(AtomicUsize::new(0));
        let stop_flag = Arc::new(AtomicBool::new(false));

        let mut producers: Vec<BurstProducer> = (0..PRODUCER_COUNT)
            .map(|producer_id| {
                let generation = Arc::clone(&generation);
                let stop_flag = Arc::clone(&stop_flag);
                let mut producer = disruptor.create_producer();

                BurstProducer::new(generation, stop_flag, move || {
                    producer.batch_publish(burst_size as usize, |iter| {
                        for (index, event) in iter.enumerate() {
                            let value = (index + 1) as i64;
                            event.producer_id = producer_id;
                            event.value = std::hint::black_box(value);
                        }
                    });
                })
            })
            .collect();

        b.iter_custom(|iters| {
            pause(pause_ms);
            let start = Instant::now();

            for _ in 0..iters {
                let start_count = event_counter.load(Ordering::Relaxed);
                let target = start_count + expected_total;
                generation.fetch_add(1, Ordering::Release);

                wait_until_counter_reaches(
                    &event_counter,
                    target,
                    Duration::from_millis(TIMEOUT_MS),
                );
            }

            start.elapsed()
        });

        producers.iter_mut().for_each(BurstProducer::stop);
        disruptor.shutdown();
    });
}

/// Main MPSC benchmark function
pub fn fixed_mpsc_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("Fixed_MPSC");

    // Configure benchmark group with proper timing for comprehensive testing
    group.measurement_time(Duration::from_secs(15));
    group.warm_up_time(Duration::from_secs(5));
    group.sample_size(10); // Fewer samples for complex multi-threaded benchmarks

    for &burst_size in BURST_SIZES.iter() {
        // Baseline measurement
        baseline_mpsc_measurement(&mut group, burst_size);

        for &pause_ms in PAUSE_MS.iter() {
            // Only test reasonable combinations to avoid very long benchmarks
            if burst_size <= 100 || pause_ms == 0 {
                benchmark_mpsc_busy_spin(&mut group, burst_size, pause_ms);
                benchmark_mpsc_batch_busy_spin(&mut group, burst_size, pause_ms);
            }
        }
    }

    group.finish();
}

criterion_group!(fixed_mpsc, fixed_mpsc_benchmark);
criterion_main!(fixed_mpsc);
