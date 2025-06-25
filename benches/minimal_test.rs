//! Minimal test to check for hanging issues

use criterion::{criterion_group, criterion_main, Criterion};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use badbatch::disruptor::{
    event_translator::ClosureEventTranslator, BusySpinWaitStrategy,
    DefaultEventFactory, Disruptor, EventHandler, ProducerType, Result as DisruptorResult,
};

#[derive(Debug, Default, Clone, Copy)]
struct TestEvent {
    value: i64,
}

struct TestHandler {
    counter: Arc<AtomicI64>,
}

impl TestHandler {
    fn new() -> Self {
        Self {
            counter: Arc::new(AtomicI64::new(0)),
        }
    }
}

impl EventHandler<TestEvent> for TestHandler {
    fn on_event(
        &mut self,
        _event: &mut TestEvent,
        _sequence: i64,
        _end_of_batch: bool,
    ) -> DisruptorResult<()> {
        self.counter.fetch_add(1, Ordering::Release);
        Ok(())
    }
}

fn minimal_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("MinimalTest");
    group.measurement_time(Duration::from_secs(2));
    
    group.bench_function("basic", |b| {
        let factory = DefaultEventFactory::<TestEvent>::new();
        let handler = TestHandler::new();
        let counter = handler.counter.clone();

        let mut disruptor = Disruptor::new(
            factory,
            64, // Small buffer
            ProducerType::Single,
            Box::new(BusySpinWaitStrategy::new()),
        )
        .unwrap()
        .handle_events_with(handler)
        .build();

        disruptor.start().unwrap();

        b.iter(|| {
            counter.store(0, Ordering::Release);
            
            // Publish single event
            disruptor
                .publish_event(ClosureEventTranslator::new(
                    |event: &mut TestEvent, _seq: i64| {
                        event.value = 42;
                    },
                ))
                .unwrap();

            // Wait with timeout
            let start = Instant::now();
            while counter.load(Ordering::Acquire) < 1 {
                if start.elapsed() > Duration::from_millis(100) {
                    panic!("Timeout waiting for event");
                }
                std::hint::spin_loop();
            }
        });

        disruptor.shutdown().unwrap();
    });
    
    group.finish();
}

criterion_group!(minimal, minimal_benchmark);
criterion_main!(minimal);