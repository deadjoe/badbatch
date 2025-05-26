use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};
use std::time::Duration;

use badbatch::disruptor::{Disruptor, ProducerType, BlockingWaitStrategy, DefaultEventFactory, NoOpEventHandler};

/// Test event for benchmarking
#[derive(Debug, Clone, Default)]
struct BenchEvent {
    value: i64,
}

/// Benchmark Disruptor creation
fn disruptor_creation_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("disruptor_creation");
    
    for buffer_size in [1024, 4096, 16384].iter() {
        group.bench_with_input(
            BenchmarkId::new("creation", buffer_size),
            buffer_size,
            |b, &size| {
                b.iter(|| {
                    let factory = DefaultEventFactory::<BenchEvent>::new();
                    let disruptor = Disruptor::new(
                        factory,
                        size,
                        ProducerType::Single,
                        Box::new(BlockingWaitStrategy::new()),
                    ).expect("Failed to create disruptor");
                    black_box(disruptor);
                });
            },
        );
    }
    
    group.finish();
}

/// Benchmark Disruptor with event handler setup
fn disruptor_with_handler_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("disruptor_with_handler");
    
    for buffer_size in [1024, 4096].iter() {
        group.bench_with_input(
            BenchmarkId::new("setup", buffer_size),
            buffer_size,
            |b, &size| {
                b.iter(|| {
                    let factory = DefaultEventFactory::<BenchEvent>::new();
                    let disruptor = Disruptor::new(
                        factory,
                        size,
                        ProducerType::Single,
                        Box::new(BlockingWaitStrategy::new()),
                    )
                    .expect("Failed to create disruptor")
                    .handle_events_with(NoOpEventHandler::<BenchEvent>::new())
                    .build();
                    
                    black_box(disruptor);
                });
            },
        );
    }
    
    group.finish();
}

/// Benchmark different producer types
fn producer_type_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("producer_types");
    
    let buffer_size = 4096;
    
    group.bench_function("single_producer", |b| {
        b.iter(|| {
            let factory = DefaultEventFactory::<BenchEvent>::new();
            let disruptor = Disruptor::new(
                factory,
                buffer_size,
                ProducerType::Single,
                Box::new(BlockingWaitStrategy::new()),
            ).expect("Failed to create disruptor");
            black_box(disruptor);
        });
    });
    
    group.bench_function("multi_producer", |b| {
        b.iter(|| {
            let factory = DefaultEventFactory::<BenchEvent>::new();
            let disruptor = Disruptor::new(
                factory,
                buffer_size,
                ProducerType::Multi,
                Box::new(BlockingWaitStrategy::new()),
            ).expect("Failed to create disruptor");
            black_box(disruptor);
        });
    });
    
    group.finish();
}

/// Benchmark memory allocation patterns
fn memory_allocation_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("memory_allocation");
    group.measurement_time(Duration::from_secs(5));
    
    for buffer_size in [512, 1024, 2048, 4096].iter() {
        group.bench_with_input(
            BenchmarkId::new("allocation", buffer_size),
            buffer_size,
            |b, &size| {
                b.iter(|| {
                    let factory = DefaultEventFactory::<BenchEvent>::new();
                    let disruptor = Disruptor::new(
                        factory,
                        size,
                        ProducerType::Single,
                        Box::new(BlockingWaitStrategy::new()),
                    ).expect("Failed to create disruptor");
                    
                    // Access the ring buffer to ensure it's allocated
                    let ring_buffer = disruptor.get_ring_buffer();
                    black_box(ring_buffer.buffer_size());
                    black_box(disruptor);
                });
            },
        );
    }
    
    group.finish();
}

criterion_group!(
    benches,
    disruptor_creation_benchmark,
    disruptor_with_handler_benchmark,
    producer_type_benchmark,
    memory_allocation_benchmark
);
criterion_main!(benches);
