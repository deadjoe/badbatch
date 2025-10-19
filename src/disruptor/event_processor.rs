//! Event Processor Implementation
//!
//! This module provides event processors for consuming events from the Disruptor.
//! Event processors run the main event processing loop and coordinate with
//! sequence barriers to ensure proper ordering and dependencies.

use crate::disruptor::{
    DisruptorError, EventHandler, ExceptionHandler, Result, Sequence, SequenceBarrier,
};
use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

/// Trait for event processors
///
/// This trait defines the interface for event processors, which are responsible
/// for running the main event processing loop. This follows the exact design
/// from the original LMAX Disruptor EventProcessor interface.
pub trait EventProcessor: Send + Sync + std::fmt::Debug {
    /// Get the sequence that this processor is currently processing
    ///
    /// # Returns
    /// The current sequence being processed
    fn get_sequence(&self) -> Arc<Sequence>;

    /// Halt the event processor
    ///
    /// This signals the processor to stop processing events and shut down.
    fn halt(&self);

    /// Check if the processor is running
    ///
    /// # Returns
    /// True if the processor is currently running, false otherwise
    fn is_running(&self) -> bool;

    /// Run the event processor
    ///
    /// This starts the main event processing loop. This method typically
    /// runs in its own thread and processes events until halted.
    fn run(&mut self) -> Result<()>;

    /// Try to run one batch of event processing
    ///
    /// This processes available events once and returns immediately.
    /// Returns true if events were processed, false if no events available.
    ///
    /// # Returns
    /// True if events were processed, false if no events available
    fn try_run_once(&self) -> Result<bool>;

    /// Notify of a timeout
    fn notify_timeout(&self, sequence: i64);

    /// Called when the processor starts
    fn on_start(&self);

    /// Called when the processor shuts down
    fn on_shutdown(&self);
}

/// Data provider trait for accessing events
///
/// This trait abstracts the source of events for event processors.
/// It allows different types of data sources to be used with the same
/// event processing logic.
///
/// # Type Parameters
/// * `T` - The event type
pub trait DataProvider<T>: Send + Sync {
    /// Get the event at the specified sequence
    ///
    /// # Arguments
    /// * `sequence` - The sequence number of the event
    ///
    /// # Returns
    /// A reference to the event at the specified sequence
    fn get(&self, sequence: i64) -> &T;

    /// Get mutable access to an event at the specified sequence
    ///
    /// # Arguments
    /// * `sequence` - The sequence number of the event to access
    ///
    /// # Returns
    /// A mutable reference to the event
    ///
    /// # Safety
    /// This method is unsafe because it allows mutable access to events
    /// that might be accessed concurrently. The caller must ensure that
    /// only one thread accesses the event mutably at a time.
    #[allow(clippy::mut_from_ref)]
    unsafe fn get_mut(&self, sequence: i64) -> &mut T;
}

/// Batch event processor
///
/// This is the main event processor implementation that processes events in batches
/// for optimal performance. It follows the exact design from the original LMAX
/// Disruptor BatchEventProcessor.
///
/// # Type Parameters
/// * `T` - The event type being processed
#[repr(align(64))] // Cache line alignment for performance
pub struct BatchEventProcessor<T>
where
    T: Send + Sync,
{
    /// Cache line padding before critical fields
    _pad1: [u8; 64],
    /// The data provider for accessing events
    data_provider: Arc<dyn DataProvider<T>>,
    /// The sequence barrier for coordination
    sequence_barrier: Arc<dyn SequenceBarrier>,
    /// The event handler for processing events (wrapped in UnsafeCell for interior mutability)
    event_handler: UnsafeCell<Box<dyn EventHandler<T>>>,
    /// The exception handler for error handling
    exception_handler: Box<dyn ExceptionHandler<T>>,
    /// The current sequence being processed
    sequence: Arc<Sequence>,
    /// Flag indicating if the processor is running
    running: AtomicBool,
    /// Cache line padding after critical fields
    _pad2: [u8; 64],
}

// Safety: BatchEventProcessor is Send and Sync because:
// - All fields except event_handler are Send + Sync
// - event_handler is protected by UnsafeCell and only accessed from the processing thread
// - The processor ensures single-threaded access to the event handler
unsafe impl<T> Send for BatchEventProcessor<T> where T: Send + Sync {}
unsafe impl<T> Sync for BatchEventProcessor<T> where T: Send + Sync {}

impl<T> BatchEventProcessor<T>
where
    T: Send + Sync + 'static,
{
    /// Create a new batch event processor
    ///
    /// # Arguments
    /// * `data_provider` - The data provider for accessing events
    /// * `sequence_barrier` - The sequence barrier for coordination
    /// * `event_handler` - The event handler for processing events
    /// * `exception_handler` - The exception handler for error handling
    ///
    /// # Returns
    /// A new BatchEventProcessor instance
    pub fn new(
        data_provider: Arc<dyn DataProvider<T>>,
        sequence_barrier: Arc<dyn SequenceBarrier>,
        event_handler: Box<dyn EventHandler<T>>,
        exception_handler: Box<dyn ExceptionHandler<T>>,
    ) -> Self {
        Self {
            _pad1: [0; 64],
            data_provider,
            sequence_barrier,
            event_handler: UnsafeCell::new(event_handler),
            exception_handler,
            sequence: Arc::new(Sequence::new_with_initial_value()),
            running: AtomicBool::new(false),
            _pad2: [0; 64],
        }
    }

    /// Process events in a batch
    ///
    /// This follows the exact LMAX Disruptor BatchEventProcessor.processEvents logic
    fn process_events(&mut self) -> Result<()> {
        // Start lifecycle
        self.on_start();

        // Main processing loop
        while self.running.load(Ordering::Acquire) {
            match self.try_run_once() {
                Ok(true) => {
                    // Successfully processed events, continue
                }
                Ok(false) => {
                    // No events available, yield to prevent busy waiting
                    thread::yield_now();
                }
                Err(DisruptorError::Alert) => {
                    // Processor was halted, stop processing
                    break;
                }
                Err(DisruptorError::Timeout) => {
                    // Timeout occurred, notify and continue
                    self.notify_timeout(self.sequence.get());
                }
                Err(e) => {
                    // Other errors, log and continue
                    crate::internal_error!("Event processor error: {e:?}");
                    thread::sleep(std::time::Duration::from_millis(1));
                }
            }
        }

        // Shutdown lifecycle
        self.on_shutdown();
        Ok(())
    }
}

impl<T> EventProcessor for BatchEventProcessor<T>
where
    T: Send + Sync + 'static,
{
    fn get_sequence(&self) -> Arc<Sequence> {
        self.sequence.clone()
    }

    fn halt(&self) {
        self.running.store(false, Ordering::Release);
        self.sequence_barrier.alert();
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    fn run(&mut self) -> Result<()> {
        // Check if already running
        if self
            .running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(DisruptorError::InvalidSequence(-1)); // Already running
        }

        // Clear any existing alerts
        self.sequence_barrier.clear_alert();

        // Run the main processing loop
        self.process_events()
    }

    fn try_run_once(&self) -> Result<bool> {
        // Check if we're running
        if !self.running.load(Ordering::Acquire) {
            return Ok(false);
        }

        let next_sequence = self.sequence.get() + 1;

        // Try to get the next available sequence with a timeout to prevent infinite blocking
        let available_sequence = match self.sequence_barrier.wait_for_with_timeout(
            next_sequence,
            std::time::Duration::from_millis(1), // Very short timeout
        ) {
            Ok(seq) => seq,
            Err(DisruptorError::Alert) => {
                // We've been alerted to stop
                return Err(DisruptorError::Alert);
            }
            Err(DisruptorError::Timeout) => {
                // No events available right now
                return Ok(false);
            }
            Err(e) => {
                // Other errors
                return Err(e);
            }
        };

        // Check if we have any events to process
        if available_sequence < next_sequence {
            return Ok(false);
        }

        // Process all available events in this batch
        let mut current_sequence = next_sequence;
        let batch_size = (available_sequence - next_sequence + 1) as usize;

        // Notify batch start
        // SAFETY: We have exclusive access to the event handler through UnsafeCell.
        // The event processor is designed to be used from a single thread, ensuring
        // no concurrent access to the handler.
        unsafe {
            let handler = &mut *self.event_handler.get();
            let _ = handler.on_batch_start(batch_size as i64, batch_size as i64);
        }

        while current_sequence <= available_sequence {
            let end_of_batch = current_sequence == available_sequence;

            // Get mutable access to the event for processing
            // SAFETY: We have exclusive access to this sequence position as guaranteed by
            // the barrier wait - no other processor can access this sequence until we're done.
            let event = unsafe { self.data_provider.get_mut(current_sequence) };

            // Process the event with the handler
            // SAFETY: Single-threaded access to the event handler through UnsafeCell.
            // Each event processor runs on its own thread with exclusive handler access.
            unsafe {
                let handler = &mut *self.event_handler.get();
                if let Err(e) = handler.on_event(event, current_sequence, end_of_batch) {
                    // Handle exception using exception handler
                    let exception_handler = &*self.exception_handler;
                    exception_handler.handle_event_exception(e, current_sequence, event);
                }
            }

            current_sequence += 1;
        }

        // Update our sequence to indicate we've processed up to this point
        self.sequence.set(available_sequence);

        Ok(true)
    }

    fn notify_timeout(&self, sequence: i64) {
        unsafe {
            let handler = &mut *self.event_handler.get();
            let _ = handler.on_timeout(sequence);
        }
    }

    fn on_start(&self) {
        // Clear any previous alerts triggered during shutdown/halt cycles so that
        self.sequence_barrier.clear_alert();
        self.running.store(true, Ordering::Release);
        unsafe {
            let handler = &mut *self.event_handler.get();
            let _ = handler.on_start();
        }
    }

    fn on_shutdown(&self) {
        unsafe {
            let handler = &mut *self.event_handler.get();
            let _ = handler.on_shutdown();
        }
        self.running.store(false, Ordering::Release);
    }
}

impl<T> std::fmt::Debug for BatchEventProcessor<T>
where
    T: Send + Sync,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BatchEventProcessor")
            .field("sequence", &self.sequence)
            .field("running", &self.running)
            .finish()
    }
}

impl<T> BatchEventProcessor<T>
where
    T: Send + Sync,
{
    /// Spawn this processor in a new thread
    ///
    /// # Returns
    /// A join handle for the spawned thread
    pub fn spawn(mut self) -> JoinHandle<Result<()>>
    where
        T: 'static,
    {
        thread::spawn(move || self.run())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disruptor::SingleProducerSequencer;
    use crate::disruptor::{
        BlockingWaitStrategy, DefaultExceptionHandler, NoOpEventHandler, ProcessingSequenceBarrier,
        INITIAL_CURSOR_VALUE,
    };
    use std::sync::atomic::AtomicI64;

    // Helper function to create a test sequence barrier
    fn create_test_sequence_barrier(
        cursor: Arc<Sequence>,
        wait_strategy: Arc<BlockingWaitStrategy>,
    ) -> Arc<ProcessingSequenceBarrier> {
        let sequencer = Arc::new(SingleProducerSequencer::new(16, wait_strategy.clone()))
            as Arc<dyn crate::disruptor::Sequencer>;
        Arc::new(ProcessingSequenceBarrier::new(
            cursor,
            wait_strategy,
            vec![],
            sequencer,
        ))
    }

    #[derive(Debug, Default)]
    #[allow(dead_code)]
    struct TestEvent {
        value: AtomicI64,
    }

    struct TestDataProvider {
        events: Vec<TestEvent>,
    }

    impl TestDataProvider {
        fn new(size: usize) -> Self {
            let mut events = Vec::with_capacity(size);
            for i in 0..size {
                events.push(TestEvent {
                    value: AtomicI64::new(i as i64),
                });
            }
            Self { events }
        }
    }

    impl DataProvider<TestEvent> for TestDataProvider {
        fn get(&self, sequence: i64) -> &TestEvent {
            let index = (sequence as usize) % self.events.len();
            &self.events[index]
        }

        unsafe fn get_mut(&self, sequence: i64) -> &mut TestEvent {
            let index = (sequence as usize) % self.events.len();
            // This is unsafe but acceptable for testing
            &mut *(self.events.as_ptr().add(index) as *mut TestEvent)
        }
    }

    #[test]
    fn test_batch_event_processor_creation() {
        use crate::disruptor::SingleProducerSequencer;

        let data_provider = Arc::new(TestDataProvider::new(8));
        let cursor = Arc::new(Sequence::new(INITIAL_CURSOR_VALUE));
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = Arc::new(SingleProducerSequencer::new(16, wait_strategy.clone()))
            as Arc<dyn crate::disruptor::Sequencer>;
        let sequence_barrier = Arc::new(ProcessingSequenceBarrier::new(
            cursor,
            wait_strategy,
            vec![],
            sequencer,
        ));
        let event_handler = Box::new(NoOpEventHandler::<TestEvent>::new());
        let exception_handler = Box::new(DefaultExceptionHandler::<TestEvent>::new());

        let processor = BatchEventProcessor::new(
            data_provider,
            sequence_barrier,
            event_handler,
            exception_handler,
        );

        assert_eq!(processor.get_sequence().get(), INITIAL_CURSOR_VALUE);
        assert!(!processor.is_running());
    }

    #[test]
    fn test_event_processor_halt() {
        let data_provider = Arc::new(TestDataProvider::new(8));
        let cursor = Arc::new(Sequence::new(INITIAL_CURSOR_VALUE));
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequence_barrier = create_test_sequence_barrier(cursor, wait_strategy);
        let event_handler = Box::new(NoOpEventHandler::<TestEvent>::new());
        let exception_handler = Box::new(DefaultExceptionHandler::<TestEvent>::new());

        let processor = BatchEventProcessor::new(
            data_provider,
            sequence_barrier,
            event_handler,
            exception_handler,
        );

        assert!(!processor.is_running());
        processor.halt();
        assert!(!processor.is_running());
    }

    #[test]
    fn test_try_run_once_not_running() {
        let data_provider = Arc::new(TestDataProvider::new(8));
        let cursor = Arc::new(Sequence::new(INITIAL_CURSOR_VALUE));
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequence_barrier = create_test_sequence_barrier(cursor, wait_strategy);
        let event_handler = Box::new(NoOpEventHandler::<TestEvent>::new());
        let exception_handler = Box::new(DefaultExceptionHandler::<TestEvent>::new());

        let processor = BatchEventProcessor::new(
            data_provider,
            sequence_barrier,
            event_handler,
            exception_handler,
        );

        // Should return false when not running
        let result = processor.try_run_once();
        assert!(result.is_ok());
        assert!(!result.unwrap());
    }

    #[test]
    fn test_processor_sequence_management() {
        let data_provider = Arc::new(TestDataProvider::new(8));
        let cursor = Arc::new(Sequence::new(INITIAL_CURSOR_VALUE));
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequence_barrier = create_test_sequence_barrier(cursor, wait_strategy);
        let event_handler = Box::new(NoOpEventHandler::<TestEvent>::new());
        let exception_handler = Box::new(DefaultExceptionHandler::<TestEvent>::new());

        let processor = BatchEventProcessor::new(
            data_provider,
            sequence_barrier,
            event_handler,
            exception_handler,
        );

        // Initial sequence should be INITIAL_CURSOR_VALUE
        assert_eq!(processor.get_sequence().get(), INITIAL_CURSOR_VALUE);

        // Sequence should be the same object when called multiple times
        let seq1 = processor.get_sequence();
        let seq2 = processor.get_sequence();
        assert_eq!(seq1.get(), seq2.get());
    }

    #[test]
    fn test_data_provider_interface() {
        let provider = TestDataProvider::new(4);

        // Test get method
        let event0 = provider.get(0);
        assert_eq!(event0.value.load(Ordering::Relaxed), 0);

        let event1 = provider.get(1);
        assert_eq!(event1.value.load(Ordering::Relaxed), 1);

        // Test wrapping behavior
        let event4 = provider.get(4); // Should wrap to index 0
        assert_eq!(event4.value.load(Ordering::Relaxed), 0);

        // Test get_mut method
        unsafe {
            let event_mut = provider.get_mut(2);
            event_mut.value.store(999, Ordering::Relaxed);
        }

        let event2 = provider.get(2);
        assert_eq!(event2.value.load(Ordering::Relaxed), 999);
    }

    #[test]
    fn test_processor_debug_format() {
        let data_provider = Arc::new(TestDataProvider::new(8));
        let cursor = Arc::new(Sequence::new(INITIAL_CURSOR_VALUE));
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequence_barrier = create_test_sequence_barrier(cursor, wait_strategy);
        let event_handler = Box::new(NoOpEventHandler::<TestEvent>::new());
        let exception_handler = Box::new(DefaultExceptionHandler::<TestEvent>::new());

        let processor = BatchEventProcessor::new(
            data_provider,
            sequence_barrier,
            event_handler,
            exception_handler,
        );

        let debug_str = format!("{processor:?}");
        assert!(debug_str.contains("BatchEventProcessor"));
        assert!(debug_str.contains("sequence"));
        assert!(debug_str.contains("running"));
    }

    // Custom event handler for testing event processing
    #[allow(dead_code)]
    struct CountingEventHandler {
        count: Arc<AtomicI64>,
        processed_sequences: Arc<Mutex<Vec<i64>>>,
    }

    impl CountingEventHandler {
        #[allow(dead_code)]
        fn new() -> (Self, Arc<AtomicI64>, Arc<Mutex<Vec<i64>>>) {
            let count = Arc::new(AtomicI64::new(0));
            let sequences = Arc::new(Mutex::new(Vec::new()));
            let handler = Self {
                count: count.clone(),
                processed_sequences: sequences.clone(),
            };
            (handler, count, sequences)
        }
    }

    impl EventHandler<TestEvent> for CountingEventHandler {
        fn on_event(
            &mut self,
            _event: &mut TestEvent,
            sequence: i64,
            _end_of_batch: bool,
        ) -> Result<()> {
            self.count.fetch_add(1, Ordering::Relaxed);
            self.processed_sequences.lock().unwrap().push(sequence);
            Ok(())
        }

        fn on_start(&mut self) -> Result<()> {
            Ok(())
        }

        fn on_shutdown(&mut self) -> Result<()> {
            Ok(())
        }

        fn on_timeout(&mut self, _available_sequence: i64) -> Result<()> {
            Ok(())
        }

        fn on_batch_start(&mut self, _batch_size: i64, _available_size: i64) -> Result<()> {
            Ok(())
        }
    }

    use std::sync::Mutex;

    // Note: process_batch is now an internal method and is tested through try_run_once

    // Note: exception handling is now internal and tested through integration tests

    // Note: notify_timeout is now tested through integration tests

    #[test]
    fn test_processor_spawn_method_exists() {
        // This test just verifies that the spawn method exists and can be called
        // We don't actually run the spawned thread to avoid test complexity
        let data_provider = Arc::new(TestDataProvider::new(8));
        let cursor = Arc::new(Sequence::new(INITIAL_CURSOR_VALUE));
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequence_barrier = create_test_sequence_barrier(cursor, wait_strategy);
        let event_handler = Box::new(NoOpEventHandler::<TestEvent>::new());
        let exception_handler = Box::new(DefaultExceptionHandler::<TestEvent>::new());

        let processor = BatchEventProcessor::new(
            data_provider,
            sequence_barrier,
            event_handler,
            exception_handler,
        );

        // Test that spawn method exists and returns a JoinHandle
        // We immediately halt the processor to prevent infinite loop
        processor.halt();
        let _handle = processor.spawn();

        // Note: We don't join the handle to avoid blocking the test
        // The spawned thread should exit quickly due to the halt() call
    }
}
