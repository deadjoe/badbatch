#![allow(
    missing_docs,
    clippy::all,
    clippy::pedantic,
    clippy::nursery,
    clippy::cargo
)]

//! Integration tests for the fixed event processing functionality
//!
//! These tests verify that the critical fixes for event processing work correctly:
//! 1. Real event processing in disruptor.rs
//! 2. Proper exception handling in builder.rs  
//! 3. Sequence barrier dependency resolution
//! 4. Complete try_run_once implementation

use badbatch::disruptor::event_translator::ClosureEventTranslator;
use badbatch::disruptor::{
    BlockingWaitStrategy, DefaultEventFactory, Disruptor, EventHandler, EventProcessor,
    ProducerType, Result, Sequencer,
};
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Default, Clone)]
struct TestEvent {
    value: i64,
    processed_by: String,
}

/// Test event handler that counts processed events
struct CountingEventHandler {
    name: String,
    count: Arc<AtomicUsize>,
    last_sequence: Arc<AtomicI64>,
    processed_sequences: Arc<Mutex<Vec<i64>>>,
    should_fail_on: Option<i64>, // Sequence number to fail on for exception testing
}

#[allow(dead_code)]
impl CountingEventHandler {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            count: Arc::new(AtomicUsize::new(0)),
            last_sequence: Arc::new(AtomicI64::new(-1)),
            processed_sequences: Arc::new(Mutex::new(Vec::new())),
            should_fail_on: None,
        }
    }

    fn new_with_failure(name: &str, fail_on_sequence: i64) -> Self {
        Self {
            name: name.to_string(),
            count: Arc::new(AtomicUsize::new(0)),
            last_sequence: Arc::new(AtomicI64::new(-1)),
            processed_sequences: Arc::new(Mutex::new(Vec::new())),
            should_fail_on: Some(fail_on_sequence),
        }
    }

    fn get_count(&self) -> usize {
        self.count.load(Ordering::Acquire)
    }

    #[allow(dead_code)]
    fn get_last_sequence(&self) -> i64 {
        self.last_sequence.load(Ordering::Acquire)
    }
}

fn wait_until<F>(timeout: Duration, mut condition: F, description: &str)
where
    F: FnMut() -> bool,
{
    let start = Instant::now();
    while !condition() {
        assert!(
            start.elapsed() < timeout,
            "timed out waiting for {description} after {timeout:?}"
        );
        std::thread::yield_now();
    }
}

impl EventHandler<TestEvent> for CountingEventHandler {
    fn on_event(
        &mut self,
        event: &mut TestEvent,
        sequence: i64,
        _end_of_batch: bool,
    ) -> Result<()> {
        // Check if we should simulate a failure
        if let Some(fail_seq) = self.should_fail_on {
            if sequence == fail_seq {
                return Err(badbatch::disruptor::DisruptorError::InvalidSequence(
                    sequence,
                ));
            }
        }

        // Process the event
        event.processed_by = self.name.clone();
        self.count.fetch_add(1, Ordering::Release);
        self.last_sequence.store(sequence, Ordering::Release);
        self.processed_sequences.lock().unwrap().push(sequence);

        Ok(())
    }

    fn on_start(&mut self) -> Result<()> {
        badbatch::test_log!("Handler '{name}' started", name = self.name);
        Ok(())
    }

    fn on_shutdown(&mut self) -> Result<()> {
        badbatch::test_log!("Handler '{name}' shutting down", name = self.name);
        Ok(())
    }
}

#[test]
fn test_real_event_processing_single_consumer() {
    let factory = DefaultEventFactory::<TestEvent>::new();
    let handler = CountingEventHandler::new("consumer1");
    let count_ref = handler.count.clone();
    let sequence_ref = handler.last_sequence.clone();
    let processed_sequences = handler.processed_sequences.clone();

    let mut disruptor = Disruptor::new(
        factory,
        64, // Small buffer for testing
        ProducerType::Single,
        Box::new(BlockingWaitStrategy::new()),
    )
    .unwrap()
    .handle_events_with(handler)
    .build();

    // Start the disruptor
    disruptor.start().unwrap();

    // Publish some events
    for i in 0..10 {
        disruptor
            .publish_event(ClosureEventTranslator::new(
                move |event: &mut TestEvent, _seq: i64| {
                    event.value = i;
                },
            ))
            .unwrap();
    }

    wait_until(
        Duration::from_secs(1),
        || count_ref.load(Ordering::Acquire) == 10,
        "single consumer to process all events",
    );

    // Check that events were processed
    let processed_count = count_ref.load(Ordering::Acquire);
    let last_sequence = sequence_ref.load(Ordering::Acquire);
    let seen_sequences = processed_sequences.lock().unwrap().clone();

    badbatch::test_log!("Processed {processed_count} events, last sequence: {last_sequence}");

    assert_eq!(
        processed_count, 10,
        "single consumer should process all events"
    );
    assert_eq!(last_sequence, 9, "last processed sequence should be 9");
    assert_eq!(seen_sequences, (0..10).collect::<Vec<_>>());

    // Shutdown
    disruptor.shutdown().unwrap();
}

#[test]
fn test_real_event_processing_multiple_consumers() {
    let factory = DefaultEventFactory::<TestEvent>::new();
    let handler1 = CountingEventHandler::new("consumer1");
    let handler2 = CountingEventHandler::new("consumer2");

    let count1_ref = handler1.count.clone();
    let count2_ref = handler2.count.clone();
    let sequences1 = handler1.processed_sequences.clone();
    let sequences2 = handler2.processed_sequences.clone();

    let mut disruptor = Disruptor::new(
        factory,
        64,
        ProducerType::Single,
        Box::new(BlockingWaitStrategy::new()),
    )
    .unwrap()
    .handle_events_with(handler1)
    .then(handler2) // Sequential processing
    .build();

    // Start the disruptor
    disruptor.start().unwrap();

    // Publish events
    for i in 0..5 {
        disruptor
            .publish_event(ClosureEventTranslator::new(
                move |event: &mut TestEvent, _seq: i64| {
                    event.value = i;
                },
            ))
            .unwrap();
    }

    wait_until(
        Duration::from_secs(1),
        || count2_ref.load(Ordering::Acquire) == 5,
        "sequential second-stage consumer to process all events",
    );

    // Check results
    let count1 = count1_ref.load(Ordering::Acquire);
    let count2 = count2_ref.load(Ordering::Acquire);
    let seen_sequences1 = sequences1.lock().unwrap().clone();
    let seen_sequences2 = sequences2.lock().unwrap().clone();

    badbatch::test_log!("Consumer1 processed: {count1}, Consumer2 processed: {count2}");

    assert_eq!(count1, 5, "first consumer should process every event");
    assert_eq!(count2, 5, "second consumer should process every event");
    assert_eq!(seen_sequences1, (0..5).collect::<Vec<_>>());
    assert_eq!(seen_sequences2, (0..5).collect::<Vec<_>>());

    disruptor.shutdown().unwrap();
}

#[test]
fn test_exception_handling_continues_processing() {
    let factory = DefaultEventFactory::<TestEvent>::new();

    // Create a handler that will fail on sequence 3
    let handler = CountingEventHandler::new_with_failure("failing_consumer", 3);
    let count_ref = handler.count.clone();
    let sequence_ref = handler.last_sequence.clone();
    let processed_sequences = handler.processed_sequences.clone();

    let mut disruptor = Disruptor::new(
        factory,
        32,
        ProducerType::Single,
        Box::new(BlockingWaitStrategy::new()),
    )
    .unwrap()
    .handle_events_with(handler)
    .build();

    disruptor.start().unwrap();

    // Publish events including the one that will fail
    for i in 0..8 {
        disruptor
            .publish_event(ClosureEventTranslator::new(
                move |event: &mut TestEvent, _seq: i64| {
                    event.value = i;
                },
            ))
            .unwrap();
    }

    wait_until(
        Duration::from_secs(1),
        || count_ref.load(Ordering::Acquire) == 7,
        "processor to continue after a handler error",
    );

    let processed_count = count_ref.load(Ordering::Acquire);
    let last_sequence = sequence_ref.load(Ordering::Acquire);
    let seen_sequences = processed_sequences.lock().unwrap().clone();

    badbatch::test_log!(
        "Processed {processed_count} events (should be 7, since sequence 3 failed)"
    );

    assert_eq!(
        processed_count, 7,
        "all non-failing events should be processed"
    );
    assert_eq!(
        last_sequence, 7,
        "processing should continue past the failed event"
    );
    assert_eq!(seen_sequences, vec![0, 1, 2, 4, 5, 6, 7]);

    disruptor.shutdown().unwrap();
}

#[test]
fn test_try_run_once_functionality() {
    use badbatch::disruptor::event_processor::DataProvider;
    use badbatch::disruptor::{
        BatchEventProcessor, DefaultExceptionHandler, ProcessingSequenceBarrier,
        SingleProducerSequencer,
    };

    // Create test components
    let factory = DefaultEventFactory::<TestEvent>::new();
    let ring_buffer = Arc::new(badbatch::disruptor::RingBuffer::new(16, factory).unwrap());
    let wait_strategy = Arc::new(BlockingWaitStrategy::new());
    let sequencer = Arc::new(SingleProducerSequencer::new(16, wait_strategy.clone()));
    let barrier = Arc::new(ProcessingSequenceBarrier::new(
        sequencer.get_cursor(),
        wait_strategy,
        vec![],
        badbatch::disruptor::SequencerEnum::Single(sequencer.clone()),
    ));

    let handler = CountingEventHandler::new("test_handler");
    let count_ref = handler.count.clone();
    let sequence_ref = handler.last_sequence.clone();
    let processed_sequences = handler.processed_sequences.clone();

    let processor = BatchEventProcessor::new(
        ring_buffer.clone() as Arc<dyn DataProvider<TestEvent>>,
        barrier,
        Box::new(handler),
        Box::new(DefaultExceptionHandler::new()),
    );

    processor.on_start();

    // Publish an event first
    let seq = sequencer.next().unwrap();
    unsafe {
        let event = ring_buffer.get_mut(seq);
        event.value = 42;
    }
    sequencer.publish(seq);

    assert_eq!(
        processor.try_run_once().unwrap(),
        true,
        "try_run_once should process the published event"
    );
    assert_eq!(count_ref.load(Ordering::Acquire), 1);
    assert_eq!(sequence_ref.load(Ordering::Acquire), 0);
    assert_eq!(*processed_sequences.lock().unwrap(), vec![0]);

    processor.on_shutdown();
}

#[test]
fn test_sequence_barrier_dependency_resolution() {
    use badbatch::disruptor::{Sequence, SingleProducerSequencer};

    let wait_strategy = Arc::new(BlockingWaitStrategy::new());
    let sequencer = Arc::new(SingleProducerSequencer::new(64, wait_strategy.clone()));

    // Test that new_barrier_typed creates proper barriers with dependency tracking
    let dep_sequence = Arc::new(Sequence::new(5));
    let barrier = sequencer.new_barrier_typed(vec![dep_sequence.clone()]);

    // Verify the barrier was created successfully
    assert_eq!(barrier.get_cursor().get(), -1);

    let cursor = sequencer.get_cursor();
    cursor.set(15);

    assert!(
        matches!(
            barrier.wait_for_with_timeout(10, Duration::from_millis(20)),
            Err(badbatch::disruptor::DisruptorError::Timeout)
        ),
        "barrier should not bypass lagging dependencies"
    );

    dep_sequence.set(12);
    assert_eq!(
        barrier
            .wait_for_with_timeout(10, Duration::from_millis(20))
            .unwrap(),
        12,
        "barrier should expose the minimum available dependent sequence"
    );
}

#[test]
fn test_complete_event_flow() {
    let factory = DefaultEventFactory::<TestEvent>::new();
    let handler = CountingEventHandler::new("complete_flow");
    let count_ref = handler.count.clone();
    let sequence_ref = handler.last_sequence.clone();
    let processed_sequences = handler.processed_sequences.clone();

    let mut disruptor = Disruptor::with_defaults(factory, 32)
        .unwrap()
        .handle_events_with(handler)
        .build();

    disruptor.start().unwrap();

    // Test both publish_event and try_publish_event
    for i in 0..3 {
        disruptor
            .publish_event(ClosureEventTranslator::new(
                move |event: &mut TestEvent, _seq: i64| {
                    event.value = i;
                },
            ))
            .unwrap();
    }

    for i in 3..6 {
        let success = disruptor.try_publish_event(ClosureEventTranslator::new(
            move |event: &mut TestEvent, _seq: i64| {
                event.value = i;
            },
        ));
        assert!(success, "try_publish_event should succeed");
    }

    wait_until(
        Duration::from_secs(1),
        || count_ref.load(Ordering::Acquire) == 6,
        "complete event flow to process all events",
    );

    let final_count = count_ref.load(Ordering::Acquire);
    let final_sequence = sequence_ref.load(Ordering::Acquire);
    let seen_sequences = processed_sequences.lock().unwrap().clone();

    badbatch::test_log!("Final count: {final_count}, final sequence: {final_sequence}");

    assert_eq!(final_count, 6, "all published events should be processed");
    assert_eq!(
        final_sequence, 5,
        "last processed sequence should match final publish"
    );
    assert_eq!(seen_sequences, (0..6).collect::<Vec<_>>());

    disruptor.shutdown().unwrap();
}
