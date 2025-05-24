//! Event Processor implementation for the Disruptor
//!
//! Event processors handle the consumption of events from the ring buffer.
//! They coordinate with sequence barriers to ensure proper ordering and
//! provide efficient batch processing capabilities.

use crate::disruptor::{
    Event, EventHandler, Sequence, SequenceBarrier, Result, DisruptorError
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Trait for event processors
pub trait EventProcessor: Send {
    /// Get the sequence being tracked by this processor
    fn get_sequence(&self) -> &Arc<Sequence>;

    /// Halt the event processor
    fn halt(&self);

    /// Check if the processor is running
    fn is_running(&self) -> bool;

    /// Run the event processor (blocking)
    fn run(&mut self) -> Result<()>;
}

/// Batch event processor that processes events in batches for efficiency
pub struct BatchEventProcessor<T: Event, H: EventHandler<T>> {
    /// The sequence being tracked by this processor
    sequence: Arc<Sequence>,
    /// The sequence barrier for coordination
    sequence_barrier: SequenceBarrier,
    /// The event handler for processing events
    event_handler: H,
    /// Reference to the ring buffer data
    data_provider: Arc<dyn DataProvider<T>>,
    /// Running state
    running: AtomicBool,
    /// Exception handler for error handling
    exception_handler: Option<Box<dyn ExceptionHandler<T> + Send>>,
}

/// Trait for providing data to event processors
pub trait DataProvider<T: Event>: Send + Sync {
    /// Get a mutable reference to the event at the specified sequence
    ///
    /// # Safety
    /// The caller must ensure that only one thread accesses this sequence at a time
    unsafe fn get_mut(&self, sequence: i64) -> &mut T;
}

/// Trait for handling exceptions during event processing
pub trait ExceptionHandler<T: Event>: Send {
    /// Handle an exception that occurred during event processing
    fn handle_event_exception(
        &mut self,
        ex: &dyn std::error::Error,
        sequence: i64,
        event: &T,
    );

    /// Handle an exception that occurred on startup
    fn handle_on_start_exception(&mut self, ex: &dyn std::error::Error);

    /// Handle an exception that occurred on shutdown
    fn handle_on_shutdown_exception(&mut self, ex: &dyn std::error::Error);
}

/// Default exception handler that logs errors
pub struct DefaultExceptionHandler;

impl<T: Event> ExceptionHandler<T> for DefaultExceptionHandler {
    fn handle_event_exception(
        &mut self,
        ex: &dyn std::error::Error,
        sequence: i64,
        _event: &T,
    ) {
        eprintln!("Exception processing event at sequence {}: {}", sequence, ex);
    }

    fn handle_on_start_exception(&mut self, ex: &dyn std::error::Error) {
        eprintln!("Exception during startup: {}", ex);
    }

    fn handle_on_shutdown_exception(&mut self, ex: &dyn std::error::Error) {
        eprintln!("Exception during shutdown: {}", ex);
    }
}

impl<T: Event, H: EventHandler<T>> BatchEventProcessor<T, H> {
    /// Create a new batch event processor
    pub fn new(
        data_provider: Arc<dyn DataProvider<T>>,
        sequence_barrier: SequenceBarrier,
        event_handler: H,
    ) -> Self {
        Self {
            sequence: Arc::new(Sequence::default()),
            sequence_barrier,
            event_handler,
            data_provider,
            running: AtomicBool::new(false),
            exception_handler: Some(Box::new(DefaultExceptionHandler)),
        }
    }

    /// Set a custom exception handler
    pub fn set_exception_handler(&mut self, handler: Box<dyn ExceptionHandler<T> + Send>) {
        self.exception_handler = Some(handler);
    }

    /// Process events in a batch
    fn process_events(&mut self) -> Result<()> {
        let mut next_sequence = self.sequence.get() + 1;

        loop {
            let available_sequence = match self.sequence_barrier.wait_for(next_sequence) {
                Ok(seq) => seq,
                Err(DisruptorError::Shutdown) => {
                    // Shutdown requested
                    return Ok(());
                }
                Err(e) => return Err(e),
            };

            if available_sequence >= next_sequence {
                self.process_batch(next_sequence, available_sequence)?;
                next_sequence = available_sequence + 1;
                self.sequence.set(available_sequence);
            }

            if !self.is_running() {
                break;
            }
        }

        Ok(())
    }

    /// Process a batch of events
    fn process_batch(&mut self, start_sequence: i64, end_sequence: i64) -> Result<()> {
        for sequence in start_sequence..=end_sequence {
            let end_of_batch = sequence == end_sequence;

            // Safety: We have exclusive access to this sequence range
            let event = unsafe {
                self.data_provider.get_mut(sequence)
            };

            match self.event_handler.on_event(event, sequence, end_of_batch) {
                Ok(()) => {}
                Err(e) => {
                    if let Some(ref mut handler) = self.exception_handler {
                        handler.handle_event_exception(&e, sequence, event);
                    }
                    // Continue processing other events in the batch
                }
            }
        }

        Ok(())
    }

    /// Notify startup
    fn notify_start(&mut self) {
        if let Err(e) = self.on_start() {
            if let Some(ref mut handler) = self.exception_handler {
                handler.handle_on_start_exception(&e);
            }
        }
    }

    /// Notify shutdown
    fn notify_shutdown(&mut self) {
        if let Err(e) = self.on_shutdown() {
            if let Some(ref mut handler) = self.exception_handler {
                handler.handle_on_shutdown_exception(&e);
            }
        }
    }

    /// Called when the processor starts
    fn on_start(&mut self) -> Result<()> {
        // Override in subclasses if needed
        Ok(())
    }

    /// Called when the processor shuts down
    fn on_shutdown(&mut self) -> Result<()> {
        // Override in subclasses if needed
        Ok(())
    }
}

impl<T: Event, H: EventHandler<T>> EventProcessor for BatchEventProcessor<T, H> {
    fn get_sequence(&self) -> &Arc<Sequence> {
        &self.sequence
    }

    fn halt(&self) {
        self.running.store(false, Ordering::Release);
        self.sequence_barrier.alert();
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    fn run(&mut self) -> Result<()> {
        if self.running.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() {
            return Err(DisruptorError::InvalidSequence(-1)); // Already running
        }

        self.sequence_barrier.clear_alert();
        self.notify_start();

        let result = self.process_events();

        self.notify_shutdown();
        self.running.store(false, Ordering::Release);

        result
    }
}

/// A simple data provider that wraps a ring buffer
pub struct RingBufferDataProvider<T: Event> {
    ring_buffer: crate::disruptor::SharedRingBuffer<T>,
}

impl<T: Event> RingBufferDataProvider<T> {
    pub fn new(ring_buffer: crate::disruptor::SharedRingBuffer<T>) -> Self {
        Self { ring_buffer }
    }
}

impl<T: Event> DataProvider<T> for RingBufferDataProvider<T> {
    unsafe fn get_mut(&self, sequence: i64) -> &mut T {
        // This is unsafe but necessary for the lock-free design
        // The caller must ensure exclusive access to this sequence
        let mut guard = self.ring_buffer.get_mut(sequence);
        // We need to extend the lifetime here, which is safe because
        // the caller guarantees exclusive access to this sequence
        std::mem::transmute(&mut *guard)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disruptor::{YieldingWaitStrategy, SingleProducerSequencer, SharedRingBuffer, Sequencer};
    use std::sync::atomic::AtomicI64;

    #[derive(Debug)]
    struct TestEvent {
        value: AtomicI64,
    }

    impl Event for TestEvent {
        fn clear(&mut self) {
            self.value.store(0, Ordering::Relaxed);
        }
    }

    struct TestEventFactory;

    impl crate::disruptor::EventFactory<TestEvent> for TestEventFactory {
        fn new_instance(&self) -> TestEvent {
            TestEvent {
                value: AtomicI64::new(0),
            }
        }
    }

    struct TestEventHandler {
        processed_count: AtomicI64,
    }

    impl TestEventHandler {
        fn new() -> Self {
            Self {
                processed_count: AtomicI64::new(0),
            }
        }

        fn get_processed_count(&self) -> i64 {
            self.processed_count.load(Ordering::Acquire)
        }
    }

    impl EventHandler<TestEvent> for TestEventHandler {
        fn on_event(&mut self, event: &mut TestEvent, sequence: i64, _end_of_batch: bool) -> Result<()> {
            event.value.store(sequence, Ordering::Relaxed);
            self.processed_count.fetch_add(1, Ordering::AcqRel);
            Ok(())
        }
    }

    #[test]
    fn test_batch_event_processor_creation() {
        let ring_buffer = SharedRingBuffer::new(8, TestEventFactory).unwrap();
        let data_provider = Arc::new(RingBufferDataProvider::new(ring_buffer));
        let wait_strategy = Arc::new(YieldingWaitStrategy::new());
        let sequencer = SingleProducerSequencer::new(8, wait_strategy);
        let barrier = sequencer.new_barrier(vec![]);
        let handler = TestEventHandler::new();

        let processor = BatchEventProcessor::new(data_provider, barrier, handler);

        assert!(!processor.is_running());
        assert_eq!(processor.get_sequence().get(), crate::disruptor::INITIAL_CURSOR_VALUE);
    }

    #[test]
    fn test_event_processor_halt() {
        let ring_buffer = SharedRingBuffer::new(8, TestEventFactory).unwrap();
        let data_provider = Arc::new(RingBufferDataProvider::new(ring_buffer));
        let wait_strategy = Arc::new(YieldingWaitStrategy::new());
        let sequencer = SingleProducerSequencer::new(8, wait_strategy);
        let barrier = sequencer.new_barrier(vec![]);
        let handler = TestEventHandler::new();

        let processor = BatchEventProcessor::new(data_provider, barrier, handler);

        assert!(!processor.is_running());

        processor.halt();
        assert!(!processor.is_running());
    }

    #[test]
    fn test_default_exception_handler() {
        let mut handler: DefaultExceptionHandler = DefaultExceptionHandler;
        let event = TestEvent {
            value: AtomicI64::new(42),
        };
        let error = crate::disruptor::DisruptorError::BufferFull;

        // These should not panic
        <DefaultExceptionHandler as ExceptionHandler<TestEvent>>::handle_event_exception(&mut handler, &error, 1, &event);
        <DefaultExceptionHandler as ExceptionHandler<TestEvent>>::handle_on_start_exception(&mut handler, &error);
        <DefaultExceptionHandler as ExceptionHandler<TestEvent>>::handle_on_shutdown_exception(&mut handler, &error);
    }
}
