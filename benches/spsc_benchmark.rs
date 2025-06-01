//! Single Producer Single Consumer (SPSC) Benchmark
//!
//! This benchmark compares BadBatch Disruptor performance against crossbeam channels
//! in single producer, single consumer scenarios with various burst sizes and pause intervals.

use criterion::measurement::WallTime;
use criterion::{
    black_box, criterion_group, criterion_main, BenchmarkGroup, BenchmarkId, Criterion, Throughput,
};
use crossbeam::channel::TrySendError::Full;
use crossbeam::channel::{
    bounded,
    TryRecvError::{Disconnected, Empty},
};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

// BadBatch Disruptor imports
use badbatch::disruptor::BusySpinWaitStrategy;
use badbatch::disruptor::build_single_producer;

// Benchmark configuration
const DATA_STRUCTURE_SIZE: usize = 128;
const BURST_SIZES: [u64; 3] = [1, 10, 100];
const PAUSES_MS: [u64; 3] = [0, 1, 10];

/// Event structure for benchmarking
#[derive(Debug, Default, Clone)]
struct Event {
    data: i64,
}

/// Pause execution for the specified number of milliseconds
fn pause(millis: u64) {
    if millis > 0 {
        thread::sleep(Duration::from_millis(millis));
    }
}

/// Main SPSC benchmark function
pub fn spsc_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("spsc");

    for burst_size in BURST_SIZES.into_iter() {
        group.throughput(Throughput::Elements(burst_size));

        // Base: Benchmark overhead of measurement logic
        base_overhead(&mut group, burst_size as i64);

        for pause_ms in PAUSES_MS.into_iter() {
            let inputs = (burst_size as i64, pause_ms);
            let param = format!("burst: {}, pause: {} ms", burst_size, pause_ms);

            crossbeam_spsc(&mut group, inputs, &param);
            badbatch_spsc_modern(&mut group, inputs, &param);
            badbatch_spsc_traditional(&mut group, inputs, &param);
        }
    }
    group.finish();
}

/// Synthetic benchmark to measure the overhead of the measurement itself
fn base_overhead(group: &mut BenchmarkGroup<WallTime>, burst_size: i64) {
    let sink = Arc::new(AtomicI64::new(0));
    let benchmark_id = BenchmarkId::new("base_overhead", burst_size);

    group.bench_with_input(benchmark_id, &burst_size, move |b, size| {
        b.iter_custom(|iters| {
            let start = Instant::now();
            for _ in 0..iters {
                for data in 1..=*size {
                    sink.store(black_box(data), Ordering::Release);
                }
                // Wait for the last data element to "be received"
                let last_data = black_box(*size);
                while sink.load(Ordering::Acquire) != last_data {}
            }
            start.elapsed()
        })
    });
}

/// Crossbeam channel SPSC benchmark
fn crossbeam_spsc(group: &mut BenchmarkGroup<WallTime>, inputs: (i64, u64), param: &str) {
    // Use an AtomicI64 to "extract" the value from the receiving thread
    let sink = Arc::new(AtomicI64::new(0));
    let (s, r) = bounded::<Event>(DATA_STRUCTURE_SIZE);

    let receiver = {
        let sink = Arc::clone(&sink);
        thread::spawn(move || loop {
            match r.try_recv() {
                Ok(event) => sink.store(event.data, Ordering::Release),
                Err(Empty) => continue,
                Err(Disconnected) => break,
            }
        })
    };

    let benchmark_id = BenchmarkId::new("crossbeam_channel", param);
    group.bench_with_input(benchmark_id, &inputs, move |b, (size, pause_ms)| {
        b.iter_custom(|iters| {
            pause(*pause_ms);
            let start = Instant::now();
            for _ in 0..iters {
                for data in 1..=*size {
                    let mut event = Event {
                        data: black_box(data),
                    };
                    while let Err(Full(e)) = s.try_send(event) {
                        event = e;
                    }
                }
                // Wait for the last data element to be received in the receiver thread
                let last_data = black_box(*size);
                while sink.load(Ordering::Acquire) != last_data {}
            }
            start.elapsed()
        })
    });

    receiver.join().expect("Receiver thread should not panic");
}

/// BadBatch Disruptor SPSC benchmark using modern disruptor-rs inspired API
fn badbatch_spsc_modern(group: &mut BenchmarkGroup<WallTime>, inputs: (i64, u64), param: &str) {
    let factory = || Event { data: 0 };
    // Use an AtomicI64 to "extract" the value from the processing thread
    let sink = Arc::new(AtomicI64::new(0));

    let processor = {
        let sink = Arc::clone(&sink);
        move |event: &mut Event, _sequence: i64, _end_of_batch: bool| {
            sink.store(event.data, Ordering::Release);
            // 现代 API 处理器应该返回 () 而不是 Result
        }
    };

    let mut producer = build_single_producer(DATA_STRUCTURE_SIZE, factory, BusySpinWaitStrategy)
        .handle_events_with(processor)
        .build();

    let benchmark_id = BenchmarkId::new("badbatch_modern", param);
    group.bench_with_input(benchmark_id, &inputs, move |b, (size, pause_ms)| {
        b.iter_custom(|iters| {
            pause(*pause_ms);
            let start = Instant::now();
            for _ in 0..iters {
                // 对于小批量，使用单个发布
                if *size <= 10 {
                    for data in 1..=*size {
                        producer.publish(|event| {
                            event.data = black_box(data);
                        });
                    }
                } else {
                    // 对于大批量，使用批量发布以提高效率
                    producer.batch_publish(*size as usize, |batch| {
                        for (i, event) in batch.enumerate() {
                            event.data = black_box(i as i64 + 1);
                        }
                    });
                }
                // 等待最后一个数据元素被处理
                let last_data = black_box(*size);
                while sink.load(Ordering::Acquire) != last_data {}
            }
            start.elapsed()
        })
    });
}

/// BadBatch Disruptor SPSC benchmark using traditional LMAX API
fn badbatch_spsc_traditional(
    group: &mut BenchmarkGroup<WallTime>,
    inputs: (i64, u64),
    param: &str,
) {
    // Simple factory function (using closure like the reference implementation)
    let factory = || Event { data: 0 };
    
    // Create a shared counter for event processing validation
    let sink = Arc::new(AtomicI64::new(0));

    // Create the processor function (following the reference implementation pattern)
    let processor = {
        let sink = Arc::clone(&sink);
        move |event: &mut Event, _sequence: i64, _end_of_batch: bool| {
            // Store the data value to the sink (no debug logs)
            sink.store(event.data, Ordering::Release);
        }
    };
    
    // Create and configure Disruptor using the build_single_producer pattern
    let mut producer = build_single_producer(DATA_STRUCTURE_SIZE, factory, BusySpinWaitStrategy::new())
        .handle_events_with(processor)
        .build();

    // Send a test event to verify event processor is working (without excessive logging)
    sink.store(0, Ordering::Release); // Reset sink value before test
    
    // Publish test event using the batch_publish approach
    producer.batch_publish(1, |iter| {
        for e in iter {
            e.data = 999;
        }
    });
    
    // Wait to see if the event is processed
    let test_start = Instant::now();
    let mut test_processed = false;
    while test_start.elapsed() < Duration::from_millis(200) {
        if sink.load(Ordering::Acquire) == 999 {
            test_processed = true;
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    
    if !test_processed {
        println!("WARNING: Initial test event not processed! Disruptor may not be functioning correctly.");
    }
    
    let benchmark_id = BenchmarkId::new("badbatch_traditional", param);
    // Use mutable reference to producer
    let producer_ref = &mut producer;
    group.bench_with_input(benchmark_id, &inputs, move |b, (size, pause_ms)| {
        b.iter_custom(|iters| {
            pause(*pause_ms);
            // Reset sink before each iteration
            sink.store(0, Ordering::Release);
            let start = Instant::now();
            
            for _iter in 0..iters {
                
                // Use batch_publish for better performance (following reference implementation)
                producer_ref.batch_publish(*size as usize, |events| {
                    for (i, e) in events.enumerate() {
                        e.data = black_box(i as i64 + 1);
                    }
                });
                
                // Wait for events to be processed
                let start_wait = Instant::now();
                
                // Wait for the last data element to be processed (silently)
                loop {
                    let current = sink.load(Ordering::Acquire);
                    if current == *size as i64 {
                        break;
                    }

                    // Extended timeout to give more time for event processing
                    if start_wait.elapsed() > Duration::from_secs(10) {
                        // Only log timeouts as they indicate a problem
                        println!("Timeout waiting for events: {}/{}", current, *size);
                        break;
                    }

                    thread::sleep(Duration::from_millis(10));
                }
            }
            start.elapsed()
        })
    });

    // Shutdown Disruptor
    producer.shutdown();
}

criterion_group!(spsc, spsc_benchmark);
criterion_main!(spsc);
