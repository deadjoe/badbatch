//! Throughput Comparison Benchmark
//!
//! This benchmark provides a comprehensive throughput comparison between BadBatch Disruptor
//! and crossbeam channels, inspired by the counters benchmark from disruptor-rs.
//!
//! Credit for the original setup goes to:
//! https://medium.com/@trunghuynh/rust-101-rust-crossbeam-vs-java-disruptor-a-wow-feeling-27eaffcda9cb

use criterion::{criterion_group, criterion_main, Criterion};
use crossbeam::channel::*;
use std::hint::black_box;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

// BadBatch Disruptor imports
use badbatch::disruptor::{
    build_multi_producer, build_single_producer, Producer, BusySpinWaitStrategy,
};

// Benchmark configuration
const BUF_SIZE: usize = 32_768;
const MAX_PRODUCER_EVENTS: usize = 10_000_000;
const BATCH_SIZE: usize = 2_000;

/// Event structure for benchmarking
#[derive(Debug, Default, Clone)]
struct Event {
    val: i32,
}

/// Crossbeam SPSC throughput test
fn crossbeam_spsc() {
    let (s, r) = bounded(BUF_SIZE);

    // Producer
    let producer_handle = thread::spawn(move || {
        for _ in 0..MAX_PRODUCER_EVENTS {
            s.send(1).unwrap();
        }
    });

    let sink = Arc::new(AtomicI32::new(0)); // Read and print value from main thread
    let sink_clone = Arc::clone(&sink);

    // Consumer
    let consumer_handle: JoinHandle<()> = thread::spawn(move || {
        for msg in r {
            sink_clone.fetch_add(msg, Ordering::Release);
        }
    });

    producer_handle.join().unwrap();
    consumer_handle.join().unwrap();

    // Verify all events were processed
    assert_eq!(sink.load(Ordering::Acquire), MAX_PRODUCER_EVENTS as i32);
}

/// Crossbeam MPSC throughput test
fn crossbeam_mpsc() {
    let (s, r) = bounded(BUF_SIZE);
    let s2 = s.clone();

    // Producer 1
    let producer1_handle = thread::spawn(move || {
        for _ in 0..MAX_PRODUCER_EVENTS {
            s.send(black_box(1)).unwrap();
        }
    });

    // Producer 2
    let producer2_handle = thread::spawn(move || {
        for _ in 0..MAX_PRODUCER_EVENTS {
            s2.send(black_box(1)).unwrap();
        }
    });

    let sink = Arc::new(AtomicI32::new(0)); // Read and print value from main thread
    let sink_clone = Arc::clone(&sink);

    // Consumer
    let consumer_handle: JoinHandle<()> = thread::spawn(move || {
        for msg in r {
            sink_clone.fetch_add(msg, Ordering::Release);
        }
    });

    producer1_handle.join().unwrap();
    producer2_handle.join().unwrap();
    consumer_handle.join().unwrap();

    // Verify all events were processed
    assert_eq!(sink.load(Ordering::Acquire), (MAX_PRODUCER_EVENTS * 2) as i32);
}

/// BadBatch Disruptor SPSC throughput test using modern API
fn badbatch_spsc_modern() {
    let factory = || Event { val: 0 };

    let sink = Arc::new(AtomicI32::new(0)); // Read and print value from main thread

    // Consumer
    let processor = {
        let sink = Arc::clone(&sink);
        move |event: &mut Event, _sequence: i64, _end_of_batch: bool| {
            sink.fetch_add(event.val, Ordering::Release);
        }
    };

    let mut producer = build_single_producer(BUF_SIZE, factory, BusySpinWaitStrategy)
        .handle_events_with(processor)
        .build();

    // Publish into the Disruptor using batch publishing for efficiency
    thread::scope(|s| {
        s.spawn(move || {
            for _ in 0..MAX_PRODUCER_EVENTS / BATCH_SIZE {
                producer.batch_publish(BATCH_SIZE, |batch| {
                    for event in batch {
                        event.val = black_box(1);
                    }
                });
            }
        });
    });

    // Verify all events were processed
    assert_eq!(sink.load(Ordering::Acquire), MAX_PRODUCER_EVENTS as i32);
}

/// BadBatch Disruptor MPSC throughput test using modern API
fn badbatch_mpsc_modern() {
    let factory = || Event { val: 0 };

    let sink = Arc::new(AtomicI32::new(0)); // Read and print value from main thread

    // Consumer
    let processor = {
        let sink = Arc::clone(&sink);
        move |event: &mut Event, _sequence: i64, _end_of_batch: bool| {
            sink.fetch_add(event.val, Ordering::Release);
        }
    };

    let producer1 = build_multi_producer(BUF_SIZE, factory, BusySpinWaitStrategy)
        .handle_events_with(processor)
        .build();

    let producer2 = producer1.clone();

    // Publish into the Disruptor using batch publishing for efficiency
    thread::scope(|s| {
        s.spawn(move || {
            for _ in 0..MAX_PRODUCER_EVENTS / BATCH_SIZE {
                producer1.batch_publish(BATCH_SIZE, |batch| {
                    for event in batch {
                        event.val = black_box(1);
                    }
                });
            }
        });

        s.spawn(move || {
            for _ in 0..MAX_PRODUCER_EVENTS / BATCH_SIZE {
                producer2.batch_publish(BATCH_SIZE, |batch| {
                    for event in batch {
                        event.val = 1;
                    }
                });
            }
        });
    });

    // Verify all events were processed
    assert_eq!(sink.load(Ordering::Acquire), (MAX_PRODUCER_EVENTS * 2) as i32);
}

/// BadBatch Disruptor SPSC throughput test using traditional LMAX API
fn badbatch_spsc_traditional() {
    use badbatch::disruptor::{
        Disruptor, ProducerType, BlockingWaitStrategy, DefaultEventFactory,
        EventHandler, EventTranslator, Result,
    };

    // Event handler implementation
    struct ThroughputEventHandler {
        sink: Arc<AtomicI32>,
    }

    impl EventHandler<Event> for ThroughputEventHandler {
        fn on_event(&mut self, event: &mut Event, _sequence: i64, _end_of_batch: bool) -> Result<()> {
            self.sink.fetch_add(event.val, Ordering::Release);
            Ok(())
        }
    }

    // Event translator implementation
    struct ThroughputEventTranslator {
        val: i32,
    }

    impl EventTranslator<Event> for ThroughputEventTranslator {
        fn translate_to(&self, event: &mut Event, _sequence: i64) {
            event.val = self.val;
        }
    }

    let sink = Arc::new(AtomicI32::new(0));
    let factory = DefaultEventFactory::<Event>::new();
    let handler = ThroughputEventHandler { sink: Arc::clone(&sink) };

    let mut disruptor = Disruptor::new(
        factory,
        BUF_SIZE,
        ProducerType::Single,
        Box::new(BlockingWaitStrategy::new()),
    ).unwrap()
    .handle_events_with(handler)
    .build();

    disruptor.start().unwrap();

    // Publish events using traditional API
    thread::scope(|s| {
        s.spawn(move || {
            for _ in 0..MAX_PRODUCER_EVENTS {
                let translator = ThroughputEventTranslator { val: black_box(1) };
                disruptor.publish_event(translator).unwrap();
            }
        });
    });

    // Verify all events were processed
    assert_eq!(sink.load(Ordering::Acquire), MAX_PRODUCER_EVENTS as i32);
}

/// Benchmark functions for Criterion
fn crossbeam_spsc_benchmark(c: &mut Criterion) {
    c.bench_function("crossbeam_spsc_throughput", |b| {
        b.iter(|| {
            crossbeam_spsc();
        });
    });
}

fn crossbeam_mpsc_benchmark(c: &mut Criterion) {
    c.bench_function("crossbeam_mpsc_throughput", |b| {
        b.iter(|| {
            crossbeam_mpsc();
        });
    });
}

fn badbatch_spsc_modern_benchmark(c: &mut Criterion) {
    c.bench_function("badbatch_spsc_modern_throughput", |b| {
        b.iter(|| {
            badbatch_spsc_modern();
        });
    });
}

fn badbatch_mpsc_modern_benchmark(c: &mut Criterion) {
    c.bench_function("badbatch_mpsc_modern_throughput", |b| {
        b.iter(|| {
            badbatch_mpsc_modern();
        });
    });
}

fn badbatch_spsc_traditional_benchmark(c: &mut Criterion) {
    c.bench_function("badbatch_spsc_traditional_throughput", |b| {
        b.iter(|| {
            badbatch_spsc_traditional();
        });
    });
}

criterion_group!(
    throughput_comparison,
    crossbeam_spsc_benchmark,
    badbatch_spsc_modern_benchmark,
    badbatch_spsc_traditional_benchmark,
    crossbeam_mpsc_benchmark,
    badbatch_mpsc_modern_benchmark
);
criterion_main!(throughput_comparison);
