// MPSC debugging integration test
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use badbatch::disruptor::{
    event_translator::ClosureEventTranslator, BusySpinWaitStrategy, DefaultEventFactory, Disruptor,
    EventHandler, ProducerType, Result as DisruptorResult,
};

#[derive(Debug, Default, Clone, Copy)]
struct DebugEvent {
    value: i64,
    producer_id: usize,
}

struct DebugHandler {
    count: Arc<AtomicI64>,
}

impl DebugHandler {
    fn new() -> Self {
        Self {
            count: Arc::new(AtomicI64::new(0)),
        }
    }

    fn get_count(&self) -> Arc<AtomicI64> {
        self.count.clone()
    }
}

impl EventHandler<DebugEvent> for DebugHandler {
    fn on_event(
        &mut self,
        event: &mut DebugEvent,
        sequence: i64,
        _end_of_batch: bool,
    ) -> DisruptorResult<()> {
        println!("DEBUG: Handler processed event value={}, producer_id={}, sequence={}", 
                event.value, event.producer_id, sequence);
        self.count.fetch_add(1, Ordering::Release);
        Ok(())
    }
}

#[test]
fn test_mpsc_simple_case() {
    println!("=== MPSC Simple Debug Test ===");

    // Use small buffer to avoid bitmap complexity
    let factory = DefaultEventFactory::<DebugEvent>::new();
    let handler = DebugHandler::new();
    let counter = handler.get_count();

    let mut disruptor = Disruptor::new(
        factory,
        32, // Small buffer
        ProducerType::Multi,
        Box::new(BusySpinWaitStrategy::new()),
    )
    .unwrap()
    .handle_events_with(handler)
    .build();

    println!("DEBUG: Starting disruptor");
    disruptor.start().unwrap();

    // Test 1: Single event
    println!("DEBUG: Publishing single event");
    let result = disruptor.publish_event(ClosureEventTranslator::new(
        |event: &mut DebugEvent, _seq: i64| {
            event.value = 42;
            event.producer_id = 0;
        },
    ));
    
    assert!(result.is_ok(), "Single event publish should succeed");
    
    // Wait for processing
    thread::sleep(Duration::from_millis(100));
    let count = counter.load(Ordering::Acquire);
    println!("DEBUG: Events processed: {}", count);
    
    assert_eq!(count, 1, "Single event should be processed");

    // Test 2: Multiple sequential events from one producer
    println!("DEBUG: Publishing 3 sequential events");
    for i in 1..=3 {
        let result = disruptor.publish_event(ClosureEventTranslator::new(
            move |event: &mut DebugEvent, _seq: i64| {
                event.value = i;
                event.producer_id = 0;
            },
        ));
        assert!(result.is_ok(), "Event {} should publish successfully", i);
    }
    
    thread::sleep(Duration::from_millis(100));
    let count = counter.load(Ordering::Acquire);
    println!("DEBUG: Events processed after sequential: {}", count);
    assert_eq!(count, 4, "Should have processed 4 events total");

    println!("DEBUG: Basic single-producer tests PASSED");
    disruptor.shutdown().unwrap();
}

#[test]
fn test_mpsc_concurrent_producers() {
    println!("=== MPSC Concurrent Producers Test ===");

    let factory = DefaultEventFactory::<DebugEvent>::new();
    let handler = DebugHandler::new();
    let counter = handler.get_count();

    let mut disruptor = Disruptor::new(
        factory,
        32, // Small buffer
        ProducerType::Multi,
        Box::new(BusySpinWaitStrategy::new()),
    )
    .unwrap()
    .handle_events_with(handler)
    .build();

    disruptor.start().unwrap();
    let disruptor_arc = Arc::new(disruptor);

    // Test concurrent producers
    println!("DEBUG: Starting 2 concurrent producers");
    let mut handles = Vec::new();
    
    for producer_id in 0..2 {
        let disruptor_clone = disruptor_arc.clone();
        let handle = thread::spawn(move || {
            println!("DEBUG: Producer {} starting", producer_id);
            for i in 1..=2 {
                let result = disruptor_clone.publish_event(ClosureEventTranslator::new(
                    move |event: &mut DebugEvent, _seq: i64| {
                        event.value = (producer_id * 10 + i) as i64;
                        event.producer_id = producer_id;
                    },
                ));
                match result {
                    Ok(_) => println!("DEBUG: Producer {} published event {}", producer_id, i),
                    Err(e) => {
                        println!("ERROR: Producer {} failed to publish event {}: {:?}", producer_id, i, e);
                        return false;
                    }
                }
                // Small delay between events
                thread::sleep(Duration::from_millis(10));
            }
            println!("DEBUG: Producer {} finished", producer_id);
            true
        });
        handles.push(handle);
    }

    // Wait for producers to complete
    let mut all_success = true;
    for (i, handle) in handles.into_iter().enumerate() {
        match handle.join() {
            Ok(success) => {
                if !success {
                    println!("ERROR: Producer {} failed", i);
                    all_success = false;
                }
            }
            Err(_) => {
                println!("ERROR: Producer {} thread panicked", i);
                all_success = false;
            }
        }
    }

    assert!(all_success, "All producers should complete successfully");

    // Wait for event processing
    thread::sleep(Duration::from_millis(200));
    let final_count = counter.load(Ordering::Acquire);
    println!("DEBUG: Final event count: {}", final_count);

    assert_eq!(final_count, 4, "Should have processed 4 events from 2 producers");

    // Cleanup
    if let Ok(mut disruptor) = Arc::try_unwrap(disruptor_arc) {
        disruptor.shutdown().unwrap();
    }
}