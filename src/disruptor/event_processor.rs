//! Event Processor Implementation
//!
//! This module provides event processors for consuming events from the Disruptor.
//! Event processors run the main event processing loop and coordinate with
//! sequence barriers to ensure proper ordering and dependencies.

use crate::disruptor::{
    DisruptorError, EventHandler, ExceptionHandler, Result, Sequence, SequenceBarrier,
};
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
    /// Returns the number of events processed.
    ///
    /// # Returns
    /// The number of events processed in this batch
    fn try_run_once(&self) -> Result<usize>;
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
    /// The event handler for processing events
    event_handler: Box<dyn EventHandler<T>>,
    /// The exception handler for error handling
    exception_handler: Box<dyn ExceptionHandler<T>>,
    /// The current sequence being processed
    sequence: Arc<Sequence>,
    /// Flag indicating if the processor is running
    running: AtomicBool,
    /// Cache line padding after critical fields
    _pad2: [u8; 64],
}

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
            event_handler,
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
        let mut next_sequence = self.sequence.get() + 1;

        loop {
            // Check if we should stop
            if !self.running.load(Ordering::Acquire) {
                break;
            }

            let _start_of_batch_sequence = next_sequence;

            // This follows the exact LMAX exception handling structure
            match self.process_batch(&mut next_sequence) {
                Ok(()) => {
                    // Batch processed successfully
                }
                Err(DisruptorError::Alert) => {
                    // Check if we're still supposed to be running
                    if !self.running.load(Ordering::Acquire) {
                        break;
                    }
                }
                Err(DisruptorError::Timeout) => {
                    // Handle timeout
                    self.notify_timeout(self.sequence.get());
                }
                Err(e) => {
                    // Handle other exceptions
                    self.handle_event_exception(e, next_sequence, None);
                    self.sequence.set(next_sequence);
                    next_sequence += 1;
                }
            }
        }

        Ok(())
    }

    /// Process a single batch of events
    /// This matches the inner try block in LMAX BatchEventProcessor
    fn process_batch(&mut self, next_sequence: &mut i64) -> Result<()> {
        // Wait for the next available sequence
        let available_sequence = self.sequence_barrier.wait_for(*next_sequence)?;

        // Calculate end of batch (with potential batch size limit)
        let end_of_batch_sequence = available_sequence; // Simplified - LMAX has batch size limits

        // Call onBatchStart if we have events to process
        if *next_sequence <= end_of_batch_sequence {
            let batch_size = end_of_batch_sequence - *next_sequence + 1;
            let available_size = available_sequence - *next_sequence + 1;

            self.event_handler
                .on_batch_start(batch_size, available_size)?;
        }

        // Process all events in this batch
        while *next_sequence <= end_of_batch_sequence {
            let end_of_batch = *next_sequence == end_of_batch_sequence;

            // Get mutable access to the event
            let event = unsafe { self.data_provider.get_mut(*next_sequence) };

            // Process the event
            self.event_handler
                .on_event(event, *next_sequence, end_of_batch)?;

            *next_sequence += 1;
        }

        // Update our sequence to indicate we've processed up to this point
        self.sequence.set(end_of_batch_sequence);

        Ok(())
    }

    /// Handle event processing exceptions
    fn handle_event_exception(&mut self, error: DisruptorError, sequence: i64, event: Option<&T>) {
        if let Some(event) = event {
            self.exception_handler
                .handle_event_exception(error, sequence, event);
        } else {
            // If we don't have the event, get it for error reporting
            let event = self.data_provider.get(sequence);
            self.exception_handler
                .handle_event_exception(error, sequence, event);
        }
    }

    /// Handle timeout notifications
    fn notify_timeout(&mut self, available_sequence: i64) {
        if let Err(e) = self.event_handler.on_timeout(available_sequence) {
            self.handle_event_exception(e, available_sequence, None);
        }
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

        // Notify start
        if let Err(e) = self.event_handler.on_start() {
            self.exception_handler.handle_on_start_exception(e);
        }

        // Run the main processing loop
        let result = self.process_events();

        // Notify shutdown
        if let Err(e) = self.event_handler.on_shutdown() {
            self.exception_handler.handle_on_shutdown_exception(e);
        }

        // Mark as not running
        self.running.store(false, Ordering::Release);

        result
    }

    fn try_run_once(&self) -> Result<usize> {
        // Check if we're running
        if !self.running.load(Ordering::Acquire) {
            return Ok(0);
        }

        let mut next_sequence = self.sequence.get() + 1;
        let mut events_processed = 0;

        // Try to get the next available sequence without blocking
        let available_sequence = match self.sequence_barrier.wait_for(next_sequence) {
            Ok(seq) => seq,
            Err(DisruptorError::Alert) => {
                // We've been alerted to stop
                return Err(DisruptorError::Alert);
            }
            Err(DisruptorError::Timeout) => {
                // No events available right now
                return Ok(0);
            }
            Err(e) => {
                // Other errors
                return Err(e);
            }
        };

        // Process all available events in this batch
        while next_sequence <= available_sequence {
            let _end_of_batch = next_sequence == available_sequence;

            // Get the event (we need to work around the mutable access issue)
            // For now, we'll use immutable access and handle this limitation
            let _event = self.data_provider.get(next_sequence);

            // Process the event - we need to work around the mutable handler issue
            // This is a temporary solution until we can properly handle interior mutability
            // In a real implementation, we'd need to restructure this to allow mutable access

            next_sequence += 1;
            events_processed += 1;
        }

        // Update our sequence to indicate we've processed up to this point
        self.sequence.set(available_sequence);

        Ok(events_processed)
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
    use crate::disruptor::{
        BlockingWaitStrategy, DefaultExceptionHandler, NoOpEventHandler, ProcessingSequenceBarrier,
        INITIAL_CURSOR_VALUE,
    };
    use std::sync::atomic::AtomicI64;

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
        let data_provider = Arc::new(TestDataProvider::new(8));
        let cursor = Arc::new(Sequence::new(INITIAL_CURSOR_VALUE));
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequence_barrier = Arc::new(ProcessingSequenceBarrier::new(
            cursor,
            wait_strategy,
            vec![],
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
        let sequence_barrier = Arc::new(ProcessingSequenceBarrier::new(
            cursor,
            wait_strategy,
            vec![],
        ));
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
}
