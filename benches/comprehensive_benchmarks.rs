//! Comprehensive Benchmark Suite
//! 
//! This is the main benchmark entry point that runs a comprehensive
//! performance evaluation of the BadBatch Disruptor implementation.

use criterion::{criterion_group, criterion_main, Criterion};

/// Run a focused performance test suite for CI/quick validation
pub fn quick_benchmark_suite(c: &mut Criterion) {
    // Quick SPSC test
    let mut group = c.benchmark_group("Quick_SPSC");
    group.measurement_time(std::time::Duration::from_secs(5));
    
    use badbatch::disruptor::{
        event_translator::ClosureEventTranslator, BusySpinWaitStrategy, DefaultEventFactory,
        Disruptor, EventHandler, ProducerType, Result as DisruptorResult,
    };
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::sync::Arc;
    use std::time::Instant;
    
    #[derive(Debug, Default, Clone, Copy)]
    struct QuickEvent {
        value: i64,
    }
    
    struct QuickHandler {
        counter: Arc<AtomicI64>,
    }
    
    impl QuickHandler {
        fn new() -> Self {
            Self {
                counter: Arc::new(AtomicI64::new(0)),
            }
        }
    }
    
    impl EventHandler<QuickEvent> for QuickHandler {
        fn on_event(
            &mut self,
            _event: &mut QuickEvent,
            _sequence: i64,
            _end_of_batch: bool,
        ) -> DisruptorResult<()> {
            self.counter.fetch_add(1, Ordering::Release);
            Ok(())
        }
    }
    
    let factory = DefaultEventFactory::<QuickEvent>::new();
    let handler = QuickHandler::new();
    let counter = handler.counter.clone();
    
    let mut disruptor = Disruptor::new(
        factory,
        1024,
        ProducerType::Single,
        Box::new(BusySpinWaitStrategy::new()),
    )
    .unwrap()
    .handle_events_with(handler)
    .build();
    
    disruptor.start().unwrap();
    
    group.bench_function("basic_publish", |b| {
        b.iter_custom(|iters| {
            let start = Instant::now();
            for _ in 0..iters {
                counter.store(0, Ordering::Release);
                
                for i in 0..100 {
                    disruptor
                        .publish_event(ClosureEventTranslator::new(
                            move |event: &mut QuickEvent, _seq: i64| {
                                event.value = i;
                            },
                        ))
                        .unwrap();
                }
                
                while counter.load(Ordering::Acquire) < 100 {
                    std::hint::spin_loop();
                }
            }
            start.elapsed()
        })
    });
    
    disruptor.shutdown().unwrap();
    group.finish();
}

// Define the comprehensive benchmark groups
// (Individual benchmarks are run separately)

criterion_group!(
    name = quick;
    config = Criterion::default()
        .measurement_time(std::time::Duration::from_secs(5))
        .warm_up_time(std::time::Duration::from_secs(2))
        .sample_size(10);
    targets = quick_benchmark_suite
);

// Main entry point - use 'quick' for CI
criterion_main!(quick);