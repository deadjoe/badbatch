//! Disruptor Main Class Implementation
//!
//! This module provides the main Disruptor class that serves as the entry point
//! for configuring and using the Disruptor pattern. It provides a DSL-style
//! interface for setting up the ring buffer, sequencers, and event processors.

use crate::disruptor::event_processor::DataProvider;
use crate::disruptor::{
    is_power_of_two, BatchEventProcessor, BlockingWaitStrategy, DisruptorError, EventFactory,
    EventHandler, EventProcessor, MultiProducerSequencer, ProducerType, Result, RingBuffer,
    Sequence, Sequencer, SingleProducerSequencer, WaitStrategy,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

/// The main Disruptor class
///
/// This is the primary entry point for using the Disruptor pattern. It provides
/// a fluent DSL-style interface for configuring the ring buffer, sequencers,
/// event processors, and their dependencies. This follows the exact design from
/// the original LMAX Disruptor class.
///
/// # Type Parameters
/// * `T` - The event type stored in the ring buffer
///
/// # Examples
/// ```
/// use badbatch::disruptor::{Disruptor, ProducerType, BlockingWaitStrategy, DefaultEventFactory};
///
/// #[derive(Default, Debug)]
/// struct MyEvent {
///     data: i32,
/// }
///
/// let event_factory = DefaultEventFactory::<MyEvent>::new();
/// let disruptor = Disruptor::new(
///     event_factory,
///     1024,
///     ProducerType::Single,
///     Box::new(BlockingWaitStrategy::new()),
/// ).unwrap();
/// ```
#[derive(Debug)]
pub struct Disruptor<T>
where
    T: Send + Sync + 'static,
{
    /// The ring buffer for storing events
    ring_buffer: Arc<RingBuffer<T>>,
    /// The sequencer for coordinating access
    sequencer: Arc<dyn Sequencer>,
    /// The buffer size
    buffer_size: usize,
    /// Started event processors
    event_processors: Vec<Arc<dyn EventProcessor>>,
    /// Join handles for spawned threads
    thread_handles: Vec<JoinHandle<Result<()>>>,
    /// Flag indicating if the disruptor has been started
    started: bool,
    /// Shutdown flag for coordinating thread shutdown
    shutdown_flag: Arc<AtomicBool>,
}

impl<T> Disruptor<T>
where
    T: Send + Sync + 'static + std::fmt::Debug,
{
    /// Create a new Disruptor
    ///
    /// # Arguments
    /// * `event_factory` - Factory for creating events
    /// * `buffer_size` - Size of the ring buffer (must be a power of 2)
    /// * `producer_type` - Whether to use single or multi producer
    /// * `wait_strategy` - Strategy for waiting for events
    ///
    /// # Returns
    /// A new Disruptor instance
    ///
    /// # Errors
    /// Returns an error if the buffer size is invalid
    pub fn new<F>(
        event_factory: F,
        buffer_size: usize,
        producer_type: ProducerType,
        wait_strategy: Box<dyn WaitStrategy>,
    ) -> Result<Self>
    where
        F: EventFactory<T>,
    {
        if !is_power_of_two(buffer_size) {
            return Err(DisruptorError::InvalidBufferSize(buffer_size));
        }

        // Create the ring buffer
        let ring_buffer = Arc::new(RingBuffer::new(buffer_size, event_factory)?);

        // Create the appropriate sequencer based on producer type
        let wait_strategy = Arc::from(wait_strategy);
        let sequencer: Arc<dyn Sequencer> = match producer_type {
            ProducerType::Single => {
                Arc::new(SingleProducerSequencer::new(buffer_size, wait_strategy))
            }
            ProducerType::Multi => {
                Arc::new(MultiProducerSequencer::new(buffer_size, wait_strategy))
            }
        };

        Ok(Self {
            ring_buffer,
            sequencer,
            buffer_size,
            event_processors: Vec::new(),
            thread_handles: Vec::new(),
            started: false,
            shutdown_flag: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Create a new Disruptor with default settings
    ///
    /// Uses single producer and blocking wait strategy by default.
    ///
    /// # Arguments
    /// * `event_factory` - Factory for creating events
    /// * `buffer_size` - Size of the ring buffer (must be a power of 2)
    ///
    /// # Returns
    /// A new Disruptor instance with default settings
    pub fn with_defaults<F>(event_factory: F, buffer_size: usize) -> Result<Self>
    where
        F: EventFactory<T>,
    {
        Self::new(
            event_factory,
            buffer_size,
            ProducerType::Single,
            Box::new(BlockingWaitStrategy::new()),
        )
    }

    /// Get the ring buffer
    ///
    /// # Returns
    /// A reference to the ring buffer
    pub fn get_ring_buffer(&self) -> &Arc<RingBuffer<T>> {
        &self.ring_buffer
    }

    /// Get the cursor sequence
    ///
    /// # Returns
    /// The cursor sequence from the sequencer
    pub fn get_cursor(&self) -> Arc<Sequence> {
        self.sequencer.get_cursor()
    }

    /// Get the buffer size
    ///
    /// # Returns
    /// The size of the ring buffer
    pub fn get_buffer_size(&self) -> usize {
        self.buffer_size
    }

    /// Get the remaining capacity
    ///
    /// # Returns
    /// The remaining capacity in the ring buffer
    pub fn get_remaining_capacity(&self) -> i64 {
        self.sequencer.remaining_capacity()
    }

    /// Handle events with the specified event handler
    ///
    /// This creates an event processor for the given handler and adds it
    /// to the processing chain.
    ///
    /// # Arguments
    /// * `event_handler` - The handler to process events
    ///
    /// # Returns
    /// A DisruptorBuilder for further configuration
    pub fn handle_events_with<H>(mut self, event_handler: H) -> DisruptorBuilder<T>
    where
        H: EventHandler<T> + 'static,
    {
        // Create a sequence barrier
        let barrier = self.sequencer.new_barrier(vec![]);

        // Create the event processor
        let processor = BatchEventProcessor::new(
            self.ring_buffer.clone() as Arc<dyn DataProvider<T>>,
            barrier,
            Box::new(event_handler),
            Box::new(crate::disruptor::DefaultExceptionHandler::new()),
        );

        let processor_sequence = processor.get_sequence();
        let processor = Arc::new(processor);

        // Add the processor's sequence as a gating sequence
        self.sequencer
            .add_gating_sequences(&[processor_sequence.clone()]);

        self.event_processors.push(processor.clone());

        DisruptorBuilder {
            disruptor: self,
            last_processor_sequences: vec![processor_sequence],
        }
    }

    /// Start the Disruptor
    ///
    /// This starts all configured event processors in their own threads.
    /// Each processor runs in its own thread and processes events from the ring buffer.
    ///
    /// # Returns
    /// Ok(()) if started successfully
    ///
    /// # Errors
    /// Returns an error if already started or if starting fails
    pub fn start(&mut self) -> Result<()> {
        if self.started {
            return Err(DisruptorError::InvalidSequence(-1)); // Already started
        }

        // Reset shutdown flag
        self.shutdown_flag.store(false, Ordering::Release);

        // Start all event processors in their own threads
        // Each processor runs its actual event processing logic
        for (index, processor) in self.event_processors.iter().enumerate() {
            // Clone the processor for the thread
            let processor_clone = Arc::clone(processor);
            let shutdown_flag = Arc::clone(&self.shutdown_flag);

            let handle = thread::spawn(move || -> Result<()> {
                println!("Event processor {index} starting");

                // Start the processor's lifecycle
                processor_clone.on_start();

                // Main event processing loop - this is the real implementation
                let mut running = true;
                let mut consecutive_no_events = 0;
                const MAX_CONSECUTIVE_NO_EVENTS: u32 = 1000; // Prevent infinite waiting

                while running && !shutdown_flag.load(Ordering::Acquire) {
                    match processor_clone.try_run_once() {
                        Ok(true) => {
                            // Successfully processed events, continue
                            consecutive_no_events = 0;
                        }
                        Ok(false) => {
                            // No events available, yield to prevent busy waiting
                            consecutive_no_events += 1;
                            if consecutive_no_events >= MAX_CONSECUTIVE_NO_EVENTS {
                                // Check shutdown flag more frequently when no events
                                if shutdown_flag.load(Ordering::Acquire) {
                                    break;
                                }
                                consecutive_no_events = 0;
                            }
                            thread::yield_now();
                        }
                        Err(DisruptorError::Alert) => {
                            // Processor was halted, stop processing
                            running = false;
                        }
                        Err(DisruptorError::Timeout) => {
                            // Timeout occurred, notify and continue
                            processor_clone.notify_timeout(processor_clone.get_sequence().get());
                        }
                        Err(e) => {
                            // Other errors, log and continue
                            eprintln!("Event processor {index} error: {e:?}");
                            thread::sleep(std::time::Duration::from_millis(1));
                        }
                    }
                }

                // Shutdown lifecycle
                processor_clone.on_shutdown();
                println!("Event processor {index} shutting down");
                Ok(())
            });

            self.thread_handles.push(handle);
        }

        self.started = true;
        Ok(())
    }

    /// Shutdown the Disruptor
    ///
    /// This halts all event processors and waits for them to complete.
    /// All threads are gracefully stopped and joined.
    ///
    /// # Returns
    /// Ok(()) if shutdown successfully
    pub fn shutdown(&mut self) -> Result<()> {
        if !self.started {
            return Ok(()); // Not started, nothing to shutdown
        }

        // Signal shutdown to all threads atomically
        self.shutdown_flag.store(true, Ordering::Release);

        // Ensure memory ordering before halting processors
        std::sync::atomic::fence(Ordering::SeqCst);

        // Halt all event processors to ensure they stop processing
        for processor in &self.event_processors {
            processor.halt();
        }

        // Give threads a brief moment to notice the shutdown signal
        std::thread::sleep(std::time::Duration::from_millis(1));

        // Wait for all threads to complete with better error handling
        let mut join_errors = Vec::new();
        while let Some(handle) = self.thread_handles.pop() {
            match handle.join() {
                Ok(Ok(())) => {
                    // Thread completed successfully
                }
                Ok(Err(e)) => {
                    let err_msg = format!("Event processor thread returned error: {e:?}");
                    eprintln!("{err_msg}");
                    join_errors.push(err_msg);
                }
                Err(_) => {
                    let err_msg = "Event processor thread panicked".to_string();
                    eprintln!("{err_msg}");
                    join_errors.push(err_msg);
                }
            }
        }

        self.started = false;

        // Return error if any threads failed to shutdown cleanly
        if !join_errors.is_empty() {
            return Err(DisruptorError::ShutdownError(format!(
                "Some threads failed to shutdown cleanly: {join_errors:?}"
            )));
        }

        Ok(())
    }

    /// Publish an event using an event translator
    ///
    /// # Arguments
    /// * `translator` - The translator to populate the event
    ///
    /// # Returns
    /// Ok(()) if published successfully
    pub fn publish_event<Tr>(&self, translator: Tr) -> Result<()>
    where
        Tr: crate::disruptor::EventTranslator<T>,
    {
        let sequence = self.sequencer.next()?;

        // Get the event from the ring buffer at the claimed sequence
        // This is safe because we have exclusive access to this sequence until we publish
        let event = unsafe { self.ring_buffer.get_mut(sequence) };

        // Use the translator to populate the event with data
        translator.translate_to(event, sequence);

        // Publish the sequence to make it available to consumers
        self.sequencer.publish(sequence);
        Ok(())
    }

    /// Try to publish an event without blocking
    ///
    /// # Arguments
    /// * `translator` - The translator to populate the event
    ///
    /// # Returns
    /// True if published successfully, false if buffer is full
    pub fn try_publish_event<Tr>(&self, translator: Tr) -> bool
    where
        Tr: crate::disruptor::EventTranslator<T>,
    {
        if let Some(sequence) = self.sequencer.try_next() {
            // Get the event from the ring buffer at the claimed sequence
            // This is safe because we have exclusive access to this sequence until we publish
            let event = unsafe { self.ring_buffer.get_mut(sequence) };

            // Use the translator to populate the event with data
            translator.translate_to(event, sequence);

            // Publish the sequence to make it available to consumers
            self.sequencer.publish(sequence);
            true
        } else {
            false
        }
    }
}

/// Builder for configuring Disruptor event processing chains
///
/// This provides a fluent interface for building complex event processing
/// topologies with dependencies between processors.
pub struct DisruptorBuilder<T>
where
    T: Send + Sync + 'static,
{
    disruptor: Disruptor<T>,
    last_processor_sequences: Vec<Arc<Sequence>>,
}

impl<T> DisruptorBuilder<T>
where
    T: Send + Sync + 'static + std::fmt::Debug,
{
    /// Add another event handler that depends on the previous handlers
    ///
    /// # Arguments
    /// * `event_handler` - The handler to process events
    ///
    /// # Returns
    /// A new DisruptorBuilder for further configuration
    pub fn then<H>(mut self, event_handler: H) -> Self
    where
        H: EventHandler<T> + 'static,
    {
        // Create a barrier that depends on the last processor sequences
        let barrier = self
            .disruptor
            .sequencer
            .new_barrier(self.last_processor_sequences.clone());

        // Create the event processor
        let processor = BatchEventProcessor::new(
            self.disruptor.ring_buffer.clone() as Arc<dyn DataProvider<T>>,
            barrier,
            Box::new(event_handler),
            Box::new(crate::disruptor::DefaultExceptionHandler::new()),
        );

        let processor_sequence = processor.get_sequence();
        let processor = Arc::new(processor);

        // Add the processor's sequence as a gating sequence
        self.disruptor
            .sequencer
            .add_gating_sequences(&[processor_sequence.clone()]);

        self.disruptor.event_processors.push(processor);
        self.last_processor_sequences = vec![processor_sequence];

        self
    }

    /// Finish building and return the configured Disruptor
    ///
    /// # Returns
    /// The configured Disruptor instance
    pub fn build(self) -> Disruptor<T> {
        self.disruptor
    }
}

// Implement DataProvider for RingBuffer
impl<T> DataProvider<T> for RingBuffer<T>
where
    T: Send + Sync,
{
    fn get(&self, sequence: i64) -> &T {
        self.get(sequence)
    }

    unsafe fn get_mut(&self, sequence: i64) -> &mut T {
        // SAFETY: The caller must ensure exclusive access to the sequence slot.
        // This is guaranteed by the Disruptor pattern where:
        // - Producers only access sequences they've claimed from the sequencer
        // - Consumers only access sequences after all producers have published
        // - The sequencer coordinates access to prevent overlapping claims
        &mut *self.get_mut_unchecked(sequence)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disruptor::{
        DefaultEventFactory, NoOpEventHandler, SleepingWaitStrategy, YieldingWaitStrategy,
    };
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    #[derive(Debug, Default, Clone)]
    #[allow(dead_code)]
    struct TestEvent {
        value: i64,
    }

    #[test]
    fn test_disruptor_creation() {
        let factory = DefaultEventFactory::<TestEvent>::new();
        let disruptor = Disruptor::new(
            factory,
            1024,
            ProducerType::Single,
            Box::new(BlockingWaitStrategy::new()),
        )
        .unwrap();

        assert_eq!(disruptor.get_buffer_size(), 1024);
        assert!(!disruptor.started);
    }

    #[test]
    fn test_disruptor_creation_multi_producer() {
        let factory = DefaultEventFactory::<TestEvent>::new();
        let disruptor = Disruptor::new(
            factory,
            512,
            ProducerType::Multi,
            Box::new(YieldingWaitStrategy::new()),
        )
        .unwrap();

        assert_eq!(disruptor.get_buffer_size(), 512);
        assert!(!disruptor.started);
    }

    #[test]
    fn test_disruptor_with_defaults() {
        let factory = DefaultEventFactory::<TestEvent>::new();
        let disruptor = Disruptor::with_defaults(factory, 512).unwrap();

        assert_eq!(disruptor.get_buffer_size(), 512);
    }

    #[test]
    fn test_disruptor_invalid_buffer_size() {
        let factory = DefaultEventFactory::<TestEvent>::new();
        let result = Disruptor::new(
            factory,
            1023, // Not a power of 2
            ProducerType::Single,
            Box::new(BlockingWaitStrategy::new()),
        );

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            DisruptorError::InvalidBufferSize(1023)
        ));
    }

    #[test]
    fn test_disruptor_zero_buffer_size() {
        let factory = DefaultEventFactory::<TestEvent>::new();
        let result = Disruptor::new(
            factory,
            0,
            ProducerType::Single,
            Box::new(BlockingWaitStrategy::new()),
        );

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            DisruptorError::InvalidBufferSize(0)
        ));
    }

    #[test]
    fn test_disruptor_builder() {
        let factory = DefaultEventFactory::<TestEvent>::new();
        let disruptor = Disruptor::with_defaults(factory, 1024)
            .unwrap()
            .handle_events_with(NoOpEventHandler::<TestEvent>::new())
            .then(NoOpEventHandler::<TestEvent>::new())
            .build();

        assert_eq!(disruptor.event_processors.len(), 2);
    }

    #[test]
    fn test_disruptor_get_ring_buffer() {
        let factory = DefaultEventFactory::<TestEvent>::new();
        let disruptor = Disruptor::with_defaults(factory, 256).unwrap();

        let ring_buffer = disruptor.get_ring_buffer();
        assert_eq!(ring_buffer.buffer_size(), 256);
    }

    #[test]
    fn test_disruptor_get_cursor() {
        let factory = DefaultEventFactory::<TestEvent>::new();
        let disruptor = Disruptor::with_defaults(factory, 128).unwrap();

        let cursor = disruptor.get_cursor();
        assert_eq!(cursor.get(), -1); // Initial cursor value
    }

    #[test]
    fn test_disruptor_get_remaining_capacity() {
        let factory = DefaultEventFactory::<TestEvent>::new();
        let disruptor = Disruptor::with_defaults(factory, 64).unwrap();

        let capacity = disruptor.get_remaining_capacity();
        // With no consumers registered, should return full buffer capacity
        assert_eq!(capacity, 64);
    }

    #[test]
    fn test_disruptor_start_and_shutdown() {
        let factory = DefaultEventFactory::<TestEvent>::new();
        let mut disruptor = Disruptor::with_defaults(factory, 32)
            .unwrap()
            .handle_events_with(NoOpEventHandler::<TestEvent>::new())
            .build();

        // Test start
        assert!(!disruptor.started);
        disruptor.start().unwrap();
        assert!(disruptor.started);

        // Test double start fails
        let result = disruptor.start();
        assert!(result.is_err());

        // Test shutdown - give a brief moment for threads to initialize, then shutdown immediately
        // The key fix: don't let the event processors wait indefinitely for events that will never come
        std::thread::sleep(Duration::from_millis(5)); // Minimal delay for thread startup
        disruptor.shutdown().unwrap();
        assert!(!disruptor.started);

        // Test double shutdown is ok
        disruptor.shutdown().unwrap();
    }

    #[test]
    fn test_disruptor_shutdown_without_start() {
        let factory = DefaultEventFactory::<TestEvent>::new();
        let mut disruptor = Disruptor::with_defaults(factory, 16).unwrap();

        // Should be ok to shutdown without starting
        disruptor.shutdown().unwrap();
        assert!(!disruptor.started);
    }

    #[test]
    fn test_disruptor_with_different_wait_strategies() {
        let factory = DefaultEventFactory::<TestEvent>::new();

        // Test with SleepingWaitStrategy
        let factory2 = DefaultEventFactory::<TestEvent>::new();
        let disruptor1 = Disruptor::new(
            factory,
            64,
            ProducerType::Single,
            Box::new(SleepingWaitStrategy::new()),
        )
        .unwrap();
        assert_eq!(disruptor1.get_buffer_size(), 64);

        // Test with YieldingWaitStrategy
        let disruptor2 = Disruptor::new(
            factory2,
            128,
            ProducerType::Multi,
            Box::new(YieldingWaitStrategy::new()),
        )
        .unwrap();
        assert_eq!(disruptor2.get_buffer_size(), 128);
    }

    // Custom event handler for testing event publishing
    #[allow(dead_code)]
    struct CountingEventHandler {
        count: Arc<AtomicI64>,
    }

    #[allow(dead_code)]
    impl CountingEventHandler {
        fn new() -> Self {
            Self {
                count: Arc::new(AtomicI64::new(0)),
            }
        }

        fn get_count(&self) -> i64 {
            self.count.load(Ordering::Acquire)
        }
    }

    impl crate::disruptor::EventHandler<TestEvent> for CountingEventHandler {
        fn on_event(
            &mut self,
            _event: &mut TestEvent,
            _sequence: i64,
            _end_of_batch: bool,
        ) -> crate::disruptor::Result<()> {
            self.count.fetch_add(1, Ordering::Release);
            Ok(())
        }
    }

    // Custom event translator for testing
    struct TestEventTranslator {
        value: i64,
    }

    impl TestEventTranslator {
        fn new(value: i64) -> Self {
            Self { value }
        }
    }

    impl crate::disruptor::EventTranslator<TestEvent> for TestEventTranslator {
        fn translate_to(&self, event: &mut TestEvent, _sequence: i64) {
            event.value = self.value;
        }
    }

    #[test]
    fn test_disruptor_publish_event() {
        let factory = DefaultEventFactory::<TestEvent>::new();
        let disruptor = Disruptor::with_defaults(factory, 32).unwrap();

        // Test successful publish
        let translator = TestEventTranslator::new(42);
        let result = disruptor.publish_event(translator);
        assert!(result.is_ok());

        // Verify cursor advanced
        assert_eq!(disruptor.get_cursor().get(), 0);
    }

    #[test]
    fn test_disruptor_try_publish_event() {
        let factory = DefaultEventFactory::<TestEvent>::new();
        let disruptor = Disruptor::with_defaults(factory, 4).unwrap();

        // Test successful publish
        let translator = TestEventTranslator::new(123);
        let result = disruptor.try_publish_event(translator);
        assert!(result);

        // Verify cursor advanced
        assert_eq!(disruptor.get_cursor().get(), 0);
    }

    #[test]
    fn test_disruptor_builder_chain() {
        let factory = DefaultEventFactory::<TestEvent>::new();
        let handler1 = NoOpEventHandler::<TestEvent>::new();
        let handler2 = NoOpEventHandler::<TestEvent>::new();
        let handler3 = NoOpEventHandler::<TestEvent>::new();

        let disruptor = Disruptor::with_defaults(factory, 64)
            .unwrap()
            .handle_events_with(handler1)
            .then(handler2)
            .then(handler3)
            .build();

        assert_eq!(disruptor.event_processors.len(), 3);
        assert_eq!(disruptor.get_buffer_size(), 64);
    }

    #[test]
    fn test_data_provider_implementation() {
        let factory = DefaultEventFactory::<TestEvent>::new();
        let ring_buffer = RingBuffer::new(16, factory).unwrap();

        // Test get method
        let event = ring_buffer.get(0);
        assert_eq!(event.value, 0); // Default value

        // Test get_mut method (unsafe)
        unsafe {
            let event_mut = ring_buffer.get_mut(0);
            event_mut.value = 999;
        }

        // Verify the change
        let event = ring_buffer.get(0);
        assert_eq!(event.value, 999);
    }
}
