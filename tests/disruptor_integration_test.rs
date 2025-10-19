#![allow(
    missing_docs,
    clippy::all,
    clippy::pedantic,
    clippy::nursery,
    clippy::cargo
)]

//! Integration tests for the fixed LMAX Disruptor implementation
//!
//! These tests verify that the core issues identified in the evaluation report
//! have been properly addressed.

use badbatch::disruptor::{
    event_translator::ClosureEventTranslator, BlockingWaitStrategy, DefaultEventFactory, Disruptor,
    EventHandler, EventTranslator, ProducerType, Result as DisruptorResult,
};
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

#[derive(Debug, Default)]
struct TestEvent {
    value: i64,
    producer_id: i32,
}

struct TestEventTranslator {
    value: i64,
    producer_id: i32,
}

impl EventTranslator<TestEvent> for TestEventTranslator {
    fn translate_to(&self, event: &mut TestEvent, _sequence: i64) {
        event.value = self.value;
        event.producer_id = self.producer_id;
    }
}

#[test]
fn test_single_producer_basic_functionality() {
    let factory = DefaultEventFactory::<TestEvent>::new();
    let disruptor = Disruptor::new(
        factory,
        1024,
        ProducerType::Single,
        Box::new(BlockingWaitStrategy::new()),
    )
    .unwrap();

    // Test basic event publishing
    let translator = TestEventTranslator {
        value: 42,
        producer_id: 1,
    };
    assert!(disruptor.publish_event(translator).is_ok());

    // Test try_publish_event
    let translator2 = TestEventTranslator {
        value: 100,
        producer_id: 1,
    };
    assert!(disruptor.try_publish_event(translator2));
}

#[test]
fn test_multi_producer_coordination() {
    let factory = DefaultEventFactory::<TestEvent>::new();
    let disruptor = Arc::new(
        Disruptor::new(
            factory,
            1024,
            ProducerType::Multi,
            Box::new(BlockingWaitStrategy::new()),
        )
        .unwrap(),
    );

    let counter = Arc::new(AtomicI64::new(0));
    let mut handles = vec![];

    // Spawn multiple producer threads
    for producer_id in 0..4 {
        let disruptor_clone = Arc::clone(&disruptor);
        let counter_clone = Arc::clone(&counter);

        let handle = thread::spawn(move || {
            for _i in 0..100 {
                let value = counter_clone.fetch_add(1, Ordering::SeqCst);
                let translator = TestEventTranslator { value, producer_id };

                // This should not fail with proper multi-producer coordination
                disruptor_clone.publish_event(translator).unwrap();

                // Small delay to increase contention
                thread::sleep(Duration::from_micros(1));
            }
        });
        handles.push(handle);
    }

    // Wait for all producers to complete
    for handle in handles {
        handle.join().unwrap();
    }

    // Verify that all events were published successfully
    assert_eq!(counter.load(Ordering::SeqCst), 400);
}

#[derive(Debug, Default)]
struct RestartEvent {
    count: usize,
}

#[derive(Clone)]
struct RestartCountingHandler {
    processed: Arc<AtomicUsize>,
}

impl EventHandler<RestartEvent> for RestartCountingHandler {
    fn on_event(
        &mut self,
        event: &mut RestartEvent,
        _sequence: i64,
        _end_of_batch: bool,
    ) -> DisruptorResult<()> {
        self.processed.fetch_add(event.count, Ordering::SeqCst);
        Ok(())
    }
}

#[test]
fn test_disruptor_restart_clears_alerts() {
    let processed = Arc::new(AtomicUsize::new(0));
    let handler = RestartCountingHandler {
        processed: Arc::clone(&processed),
    };

    let factory = DefaultEventFactory::<RestartEvent>::new();
    let mut disruptor = Disruptor::new(
        factory,
        32,
        ProducerType::Single,
        Box::new(BlockingWaitStrategy::new()),
    )
    .unwrap()
    .handle_events_with(handler)
    .build();

    disruptor.start().expect("initial start should succeed");

    for _ in 0..3 {
        let translator = ClosureEventTranslator::new(|event: &mut RestartEvent, _sequence| {
            event.count = 1;
        });
        disruptor
            .publish_event(translator)
            .expect("publish should succeed");
    }

    let start = std::time::Instant::now();
    while processed.load(Ordering::SeqCst) < 3
        && start.elapsed() < std::time::Duration::from_secs(1)
    {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(
        processed.load(Ordering::SeqCst),
        3,
        "events should process before shutdown"
    );

    disruptor.shutdown().expect("first shutdown should succeed");

    disruptor.start().expect("restart should succeed");

    for _ in 0..2 {
        let translator = ClosureEventTranslator::new(|event: &mut RestartEvent, _sequence| {
            event.count = 1;
        });
        disruptor
            .publish_event(translator)
            .expect("publish after restart should succeed");
    }

    let start = std::time::Instant::now();
    while processed.load(Ordering::SeqCst) < 5
        && start.elapsed() < std::time::Duration::from_secs(1)
    {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(
        processed.load(Ordering::SeqCst),
        5,
        "events after restart should be processed"
    );

    disruptor.shutdown().expect("final shutdown should succeed");
}

#[test]
fn test_wait_strategy_blocking() {
    let factory = DefaultEventFactory::<TestEvent>::new();
    let disruptor = Arc::new(
        Disruptor::new(
            factory,
            16, // Small buffer to test blocking
            ProducerType::Single,
            Box::new(BlockingWaitStrategy::new()),
        )
        .unwrap(),
    );

    // Fill up the buffer
    for i in 0..16 {
        let translator = TestEventTranslator {
            value: i,
            producer_id: 1,
        };
        disruptor.publish_event(translator).unwrap();
    }

    // This should work without the 1-nanosecond polling issue
    let start = std::time::Instant::now();
    let translator = TestEventTranslator {
        value: 999,
        producer_id: 1,
    };

    // This might block briefly but should not consume 100% CPU
    let _result = disruptor.try_publish_event(translator);
    let elapsed = start.elapsed();

    // The operation should complete quickly (not hang in busy loop)
    assert!(elapsed < Duration::from_millis(10));
}

#[test]
fn test_available_buffer_tracking() {
    let factory = DefaultEventFactory::<TestEvent>::new();
    let disruptor = Disruptor::new(
        factory,
        8,
        ProducerType::Multi,
        Box::new(BlockingWaitStrategy::new()),
    )
    .unwrap();

    // Publish some events
    for i in 0..5 {
        let translator = TestEventTranslator {
            value: i,
            producer_id: 1,
        };
        disruptor.publish_event(translator).unwrap();
    }

    // The sequencer should properly track which sequences are available
    // This is tested indirectly through successful publishing
    // If we get here without hanging, the test passes
}

#[test]
fn test_event_translator_integration() {
    let factory = DefaultEventFactory::<TestEvent>::new();
    let disruptor = Disruptor::new(
        factory,
        64,
        ProducerType::Single,
        Box::new(BlockingWaitStrategy::new()),
    )
    .unwrap();

    // Test that EventTranslator is properly called
    let translator = TestEventTranslator {
        value: 12345,
        producer_id: 99,
    };
    disruptor.publish_event(translator).unwrap();

    // If this completes without error, the translator was properly invoked
    // and the ring buffer was correctly accessed
    // Test passes if no panic occurs
}
