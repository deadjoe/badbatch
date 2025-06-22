//! Multi Producer Single Consumer (MPSC) Benchmarks
//! 
//! This benchmark suite tests the performance of the BadBatch Disruptor 
//! in multi producer, single consumer scenarios with different configurations.

use criterion::{
    black_box, criterion_group, criterion_main, BenchmarkGroup, BenchmarkId, Criterion, Throughput,
};
use criterion::measurement::WallTime;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use badbatch::disruptor::{
    event_translator::ClosureEventTranslator, BlockingWaitStrategy, BusySpinWaitStrategy,
    DefaultEventFactory, Disruptor, EventHandler, ProducerType, Result as DisruptorResult,
    YieldingWaitStrategy,
};

// Benchmark configuration constants
const PRODUCER_COUNT: usize = 3;
const BUFFER_SIZE: usize = 2048; // Larger buffer for multi-producer
const BURST_SIZES: [u64; 3] = [10, 100, 500];
const PAUSE_MS: [u64; 3] = [0, 1, 5];

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

/// Producer thread manager for coordinated burst production
struct BurstProducer {
    start_barrier: Arc<AtomicBool>,
    stop_flag: Arc<AtomicBool>,
    join_handle: Option<JoinHandle<()>>,
}

impl BurstProducer {
    fn new<F>(mut produce_burst: F) -> Self
    where
        F: 'static + Send + FnMut(),
    {
        let start_barrier = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::new(AtomicBool::new(false));

        let start_barrier_clone = start_barrier.clone();
        let stop_flag_clone = stop_flag.clone();

        let join_handle = thread::spawn(move || {
            while !stop_flag_clone.load(Ordering::Acquire) {
                // Wait for start signal
                while !start_barrier_clone.compare_exchange(
                    true,
                    false,
                    Ordering::Acquire,
                    Ordering::Relaxed,
                ).is_ok() {
                    if stop_flag_clone.load(Ordering::Acquire) {
                        return;
                    }
                    std::hint::spin_loop();
                }
                
                // Produce burst
                produce_burst();
            }
        });

        Self {
            start_barrier,
            stop_flag,
            join_handle: Some(join_handle),
        }
    }

    fn start(&self) {
        self.start_barrier.store(true, Ordering::Release);
    }

    fn stop(&mut self) {
        self.stop_flag.store(true, Ordering::Release);
        if let Some(handle) = self.join_handle.take() {
            handle.join().expect("Producer thread should not panic");
        }
    }
}

/// Run a coordinated benchmark with multiple producers
fn run_mpsc_benchmark<F>(
    group: &mut BenchmarkGroup<WallTime>,
    benchmark_id: BenchmarkId,
    burst_size: Arc<AtomicI64>,
    event_counter: Arc<AtomicI64>,
    params: (u64, u64),
    burst_producers: &[BurstProducer],
    setup_fn: F,
) where
    F: Fn(),
{
    group.bench_with_input(benchmark_id, &params, move |b, (size, pause_ms)| {
        setup_fn();
        
        b.iter_custom(|iters| {
            burst_size.store(*size as i64, Ordering::Release);
            let expected_total = *size * burst_producers.len() as u64;
            
            if *pause_ms > 0 {
                thread::sleep(Duration::from_millis(*pause_ms));
            }
            
            let start = Instant::now();
            for _ in 0..iters {
                event_counter.store(0, Ordering::Release);
                
                // Start all producers simultaneously
                for producer in burst_producers {
                    producer.start();
                }
                
                // Wait for all events to be processed
                while event_counter.load(Ordering::Acquire) < expected_total as i64 {
                    std::hint::spin_loop();
                }
            }
            start.elapsed()
        })
    });
}

/// Baseline measurement for MPSC overhead
fn baseline_mpsc_measurement(group: &mut BenchmarkGroup<WallTime>, burst_size: u64) {
    let event_counter = Arc::new(AtomicI64::new(0));
    let burst_size_atomic = Arc::new(AtomicI64::new(0));
    
    let mut burst_producers: Vec<BurstProducer> = (0..PRODUCER_COUNT)
        .map(|_| {
            let counter = event_counter.clone();
            let burst_size_ref = burst_size_atomic.clone();
            
            BurstProducer::new(move || {
                let size = burst_size_ref.load(Ordering::Acquire);
                for _ in 0..size {
                    counter.fetch_add(1, Ordering::Release);
                }
            })
        })
        .collect();

    let benchmark_id = BenchmarkId::new("baseline", burst_size);
    group.throughput(Throughput::Elements(burst_size * PRODUCER_COUNT as u64));
    
    run_mpsc_benchmark(
        group,
        benchmark_id,
        burst_size_atomic,
        event_counter,
        (burst_size, 0),
        &burst_producers,
        || {},
    );

    for producer in &mut burst_producers {
        producer.stop();
    }
}

/// Benchmark MPSC with BusySpinWaitStrategy
fn benchmark_mpsc_busy_spin(
    group: &mut BenchmarkGroup<WallTime>,
    burst_size: u64,
    pause_ms: u64,
) {
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
    
    let burst_size_atomic = Arc::new(AtomicI64::new(0));
    
    // Store disruptor in Arc for sharing between producer threads
    let disruptor_arc = Arc::new(disruptor);
    
    let mut burst_producers: Vec<BurstProducer> = (0..PRODUCER_COUNT)
        .map(|producer_id| {
            let burst_size_ref = burst_size_atomic.clone();
            let disruptor_ref = disruptor_arc.clone();
            
            BurstProducer::new(move || {
                let size = burst_size_ref.load(Ordering::Acquire);
                for i in 1..=size {
                    // Try to publish, retry if buffer is full
                    loop {
                        let success = disruptor_ref.try_publish_event(ClosureEventTranslator::new(
                            move |event: &mut MPSCEvent, seq: i64| {
                                event.producer_id = producer_id;
                                event.value = black_box(i);
                                event.sequence = seq;
                            },
                        ));
                        
                        if success {
                            break;
                        }
                        // Brief pause before retry
                        std::hint::spin_loop();
                    }
                }
            })
        })
        .collect();

    let param = format!("burst:{}_pause:{}ms", burst_size, pause_ms);
    let benchmark_id = BenchmarkId::new("BusySpin", param);
    group.throughput(Throughput::Elements(burst_size * PRODUCER_COUNT as u64));
    
    run_mpsc_benchmark(
        group,
        benchmark_id,
        burst_size_atomic,
        event_counter.clone(),
        (burst_size, pause_ms),
        &burst_producers,
        || {},
    );

    for producer in &mut burst_producers {
        producer.stop();
    }
    
    // Extract disruptor from Arc to shutdown
    if let Ok(mut disruptor) = Arc::try_unwrap(disruptor_arc) {
        disruptor.shutdown().unwrap();
    }
}

/// Benchmark MPSC with YieldingWaitStrategy
fn benchmark_mpsc_yielding(
    group: &mut BenchmarkGroup<WallTime>,
    burst_size: u64,
    pause_ms: u64,
) {
    let factory = DefaultEventFactory::<MPSCEvent>::new();
    let handler = MPSCCountingSink::new(PRODUCER_COUNT);
    let event_counter = handler.get_event_count();

    let mut disruptor = Disruptor::new(
        factory,
        BUFFER_SIZE,
        ProducerType::Multi,
        Box::new(YieldingWaitStrategy::new()),
    )
    .unwrap()
    .handle_events_with(handler)
    .build();

    disruptor.start().unwrap();
    
    let burst_size_atomic = Arc::new(AtomicI64::new(0));
    
    // Store disruptor in Arc for sharing between producer threads  
    let disruptor_arc = Arc::new(disruptor);
    
    let mut burst_producers: Vec<BurstProducer> = (0..PRODUCER_COUNT)
        .map(|producer_id| {
            let burst_size_ref = burst_size_atomic.clone();
            let disruptor_ref = disruptor_arc.clone();
            
            BurstProducer::new(move || {
                let size = burst_size_ref.load(Ordering::Acquire);
                for i in 1..=size {
                    loop {
                        let success = disruptor_ref.try_publish_event(ClosureEventTranslator::new(
                            move |event: &mut MPSCEvent, seq: i64| {
                                event.producer_id = producer_id;
                                event.value = black_box(i);
                                event.sequence = seq;
                            },
                        ));
                        
                        if success {
                            break;
                        }
                        thread::yield_now();
                    }
                }
            })
        })
        .collect();

    let param = format!("burst:{}_pause:{}ms", burst_size, pause_ms);
    let benchmark_id = BenchmarkId::new("Yielding", param);
    group.throughput(Throughput::Elements(burst_size * PRODUCER_COUNT as u64));
    
    run_mpsc_benchmark(
        group,
        benchmark_id,
        burst_size_atomic,
        event_counter.clone(),
        (burst_size, pause_ms),
        &burst_producers,
        || {},
    );

    for producer in &mut burst_producers {
        producer.stop();
    }
    
    // Extract disruptor from Arc to shutdown
    if let Ok(mut disruptor) = Arc::try_unwrap(disruptor_arc) {
        disruptor.shutdown().unwrap();
    }
}

/// Benchmark MPSC with BlockingWaitStrategy
fn benchmark_mpsc_blocking(
    group: &mut BenchmarkGroup<WallTime>,
    burst_size: u64,
    pause_ms: u64,
) {
    let factory = DefaultEventFactory::<MPSCEvent>::new();
    let handler = MPSCCountingSink::new(PRODUCER_COUNT);
    let event_counter = handler.get_event_count();

    let mut disruptor = Disruptor::new(
        factory,
        BUFFER_SIZE,
        ProducerType::Multi,
        Box::new(BlockingWaitStrategy::new()),
    )
    .unwrap()
    .handle_events_with(handler)
    .build();

    disruptor.start().unwrap();
    
    let burst_size_atomic = Arc::new(AtomicI64::new(0));
    
    // Store disruptor in Arc for sharing between producer threads  
    let disruptor_arc = Arc::new(disruptor);
    
    let mut burst_producers: Vec<BurstProducer> = (0..PRODUCER_COUNT)
        .map(|producer_id| {
            let burst_size_ref = burst_size_atomic.clone();
            let disruptor_ref = disruptor_arc.clone();
            
            BurstProducer::new(move || {
                let size = burst_size_ref.load(Ordering::Acquire);
                for i in 1..=size {
                    loop {
                        let success = disruptor_ref.try_publish_event(ClosureEventTranslator::new(
                            move |event: &mut MPSCEvent, seq: i64| {
                                event.producer_id = producer_id;
                                event.value = black_box(i);
                                event.sequence = seq;
                            },
                        ));
                        
                        if success {
                            break;
                        }
                        thread::sleep(Duration::from_millis(1));
                    }
                }
            })
        })
        .collect();

    let param = format!("burst:{}_pause:{}ms", burst_size, pause_ms);
    let benchmark_id = BenchmarkId::new("Blocking", param);
    group.throughput(Throughput::Elements(burst_size * PRODUCER_COUNT as u64));
    
    run_mpsc_benchmark(
        group,
        benchmark_id,
        burst_size_atomic,
        event_counter.clone(),
        (burst_size, pause_ms),
        &burst_producers,
        || {},
    );

    for producer in &mut burst_producers {
        producer.stop();
    }
    
    // Extract disruptor from Arc to shutdown
    if let Ok(mut disruptor) = Arc::try_unwrap(disruptor_arc) {
        disruptor.shutdown().unwrap();
    }
}

/// Main MPSC benchmark function
pub fn mpsc_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("MPSC");
    
    // Configure benchmark group
    group.measurement_time(Duration::from_secs(15));
    group.warm_up_time(Duration::from_secs(5));
    
    for &burst_size in BURST_SIZES.iter() {
        // Baseline measurement
        baseline_mpsc_measurement(&mut group, burst_size);
        
        for &pause_ms in PAUSE_MS.iter() {
            // Skip heavy combinations
            if burst_size > 100 && pause_ms > 1 {
                continue;
            }
            
            benchmark_mpsc_busy_spin(&mut group, burst_size, pause_ms);
            benchmark_mpsc_yielding(&mut group, burst_size, pause_ms);
            
            // Only test blocking for smaller burst sizes
            if burst_size <= 100 {
                benchmark_mpsc_blocking(&mut group, burst_size, pause_ms);
            }
        }
    }
    
    group.finish();
}

criterion_group!(mpsc, mpsc_benchmark);
criterion_main!(mpsc);