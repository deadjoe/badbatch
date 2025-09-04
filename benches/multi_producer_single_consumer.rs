//! Fixed Multi Producer Single Consumer (MPSC) Benchmarks
//!
//! This benchmark suite tests the performance of the BadBatch Disruptor
//! in multi producer, single consumer scenarios with proper synchronization.

use criterion::measurement::WallTime;
use criterion::{
    criterion_group, criterion_main, BenchmarkGroup, BenchmarkId, Criterion, Throughput,
};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Barrier};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use badbatch::disruptor::{
    event_translator::ClosureEventTranslator, BusySpinWaitStrategy, DefaultEventFactory, Disruptor,
    EventHandler, ProducerType, Result as DisruptorResult,
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
    last_values: Arc<Vec<AtomicI64>>, // Track last value from each producer
}

impl MPSCCountingSink {
    fn new(producer_count: usize) -> Self {
        let last_values = (0..producer_count)
            .map(|_| AtomicI64::new(0))
            .collect::<Vec<_>>();

        Self {
            event_count: Arc::new(AtomicI64::new(0)),
            last_values: Arc::new(last_values),
        }
    }

    fn get_event_count(&self) -> Arc<AtomicI64> {
        self.event_count.clone()
    }

    #[allow(dead_code)]
    fn reset(&self) {
        self.event_count.store(0, Ordering::Release);
        for last_value in self.last_values.iter() {
            last_value.store(0, Ordering::Release);
        }
    }
}

impl EventHandler<MPSCEvent> for MPSCCountingSink {
    fn on_event(
        &mut self,
        event: &mut MPSCEvent,
        _sequence: i64,
        _end_of_batch: bool,
    ) -> DisruptorResult<()> {
        // Update last value for this producer
        if event.producer_id < self.last_values.len() {
            self.last_values[event.producer_id].store(event.value, Ordering::Release);
        }

        // Increment total event count
        self.event_count.fetch_add(1, Ordering::Release);
        Ok(())
    }
}

/// Improved producer thread with proper synchronization
struct SyncProducer {
    join_handle: Option<JoinHandle<()>>,
    stop_flag: Arc<AtomicBool>,
}

impl SyncProducer {
    fn new(
        producer_id: usize,
        burst_size: u64,
        disruptor: Arc<Disruptor<MPSCEvent>>,
        start_barrier: Arc<Barrier>,
        stop_flag: Arc<AtomicBool>,
    ) -> Self {
        let stop_flag_clone = stop_flag.clone();

        let join_handle = thread::spawn(move || {
            // Wait for all producers to be ready
            start_barrier.wait();

            // Produce burst of events
            for i in 1..=burst_size {
                if stop_flag_clone.load(Ordering::Acquire) {
                    return;
                }

                let result = disruptor.publish_event(ClosureEventTranslator::new(
                    move |event: &mut MPSCEvent, seq: i64| {
                        event.producer_id = producer_id;
                        event.value = std::hint::black_box(i as i64);
                        event.sequence = seq;
                    },
                ));

                if result.is_err() {
                    eprintln!("Producer {producer_id} failed to publish event {i}");
                    return;
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

impl Drop for SyncProducer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Safe wait with timeout for MPSC scenarios
fn wait_for_mpsc_completion(counter: &Arc<AtomicI64>, expected: i64, timeout_ms: u64) -> bool {
    let start = Instant::now();
    let timeout = Duration::from_millis(timeout_ms);
    let mut last_count = 0;
    let mut stall_count = 0;

    while counter.load(Ordering::Acquire) < expected {
        if start.elapsed() > timeout {
            let current_count = counter.load(Ordering::Acquire);
            eprintln!(
                "WARNING: MPSC benchmark timed out waiting for {expected} events, got {current_count}"
            );
            return false;
        }

        // Check for progress to detect stalls
        let current_count = counter.load(Ordering::Acquire);
        if current_count == last_count {
            stall_count += 1;
            if stall_count > 1000 {
                eprintln!("WARNING: MPSC benchmark appears stalled at {current_count} events (expected: {expected})");
                if stall_count == 1001 {
                    eprintln!("DEBUG: This appears to be a sequence availability issue in MultiProducerSequencer");
                    eprintln!("DEBUG: Multiple producers may have claimed sequences but EventProcessor can't see them as published");
                }
                std::thread::sleep(Duration::from_millis(1));
                stall_count = 0;
            }
        } else {
            stall_count = 0;
        }
        last_count = current_count;

        std::hint::spin_loop();
    }
    true
}

/// Baseline measurement for MPSC overhead
fn baseline_mpsc_measurement(group: &mut BenchmarkGroup<WallTime>, burst_size: u64) {
    let event_counter = Arc::new(AtomicI64::new(0));
    let benchmark_id = BenchmarkId::new("baseline", burst_size);

    group.throughput(Throughput::Elements(burst_size * PRODUCER_COUNT as u64));
    group.bench_function(benchmark_id, |b| {
        b.iter_custom(|iters| {
            let start = Instant::now();
            for _ in 0..iters {
                event_counter.store(0, Ordering::Release);

                // Simulate multi-producer work
                let handles: Vec<_> = (0..PRODUCER_COUNT)
                    .map(|_| {
                        let counter = event_counter.clone();
                        thread::spawn(move || {
                            for i in 1..=burst_size {
                                counter.fetch_add(1, Ordering::Release);
                                std::hint::black_box(i);
                            }
                        })
                    })
                    .collect();

                // Wait for all threads to complete
                for handle in handles {
                    handle.join().unwrap();
                }

                // Verify completion
                assert_eq!(
                    event_counter.load(Ordering::Acquire),
                    (burst_size * PRODUCER_COUNT as u64) as i64
                );
            }
            start.elapsed()
        })
    });
}

/// Benchmark MPSC with BusySpinWaitStrategy  
/// FIXED: Use single iteration approach to avoid thread/resource accumulation
fn benchmark_mpsc_busy_spin(group: &mut BenchmarkGroup<WallTime>, burst_size: u64, pause_ms: u64) {
    let param = format!("burst:{burst_size}_pause:{pause_ms}ms");
    let benchmark_id = BenchmarkId::new("MPSC_BusySpin", param);
    let expected_total = burst_size * PRODUCER_COUNT as u64;

    group.throughput(Throughput::Elements(expected_total));
    group.bench_function(benchmark_id, |b| {
        // Use iter() instead of iter_custom() to avoid the iteration loop
        // This eliminates the thread accumulation problem
        b.iter(|| {
            if pause_ms > 0 {
                thread::sleep(Duration::from_millis(pause_ms));
            }

            // Create a fresh disruptor for this single iteration
            let factory = DefaultEventFactory::<MPSCEvent>::new();
            let handler = MPSCCountingSink::new(PRODUCER_COUNT);
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

            // Create synchronization barrier for all producers
            let start_barrier = Arc::new(Barrier::new(PRODUCER_COUNT));
            let stop_flag = Arc::new(AtomicBool::new(false));

            // Create producer threads
            let mut producers: Vec<SyncProducer> = (0..PRODUCER_COUNT)
                .map(|producer_id| {
                    SyncProducer::new(
                        producer_id,
                        burst_size,
                        disruptor_arc.clone(),
                        start_barrier.clone(),
                        stop_flag.clone(),
                    )
                })
                .collect();

            // Wait for all events to be processed
            if !wait_for_mpsc_completion(&event_counter, expected_total as i64, TIMEOUT_MS) {
                let final_count = event_counter.load(Ordering::Acquire);

                // Clean up producers before panicking
                for producer in &mut producers {
                    producer.stop();
                }
                panic!("MPSC benchmark failed: events {final_count} of {expected_total} processed within timeout");
            }

            // Clean up producers and disruptor
            for producer in &mut producers {
                producer.stop();
            }

            // Properly shutdown the disruptor
            if let Ok(mut disruptor) = Arc::try_unwrap(disruptor_arc) {
                disruptor.shutdown().unwrap();
            }

            // Return the expected count for validation
            expected_total
        });
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
            }
        }
    }

    group.finish();
}

criterion_group!(fixed_mpsc, fixed_mpsc_benchmark);
criterion_main!(fixed_mpsc);
