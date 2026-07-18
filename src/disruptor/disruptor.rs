//! Disruptor Main Class Implementation (classic LMAX-style DSL)
//!
//! This module provides the main Disruptor class that serves as the entry point
//! for configuring and using the Disruptor pattern. It provides a DSL-style
//! interface for setting up the ring buffer, sequencers, and event processors.
//!
//! **Compatibility surface.** This DSL mirrors the Java LMAX API for users
//! porting existing topologies. New code should prefer the type-state Builder
//! ([`crate::disruptor::build_single_producer`] /
//! [`crate::disruptor::build_multi_producer`]): it encodes producer-mode
//! capabilities in the type system and has the richer lifecycle
//! (draining shutdown with timeout, abrupt halt).

use crate::disruptor::{
    is_power_of_two, sequencer::SequencerEnum, BatchEventProcessor, BlockingWaitStrategy,
    DisruptorError, EventFactory, EventHandler, EventProcessor, MultiProducerSequencer,
    ProducerType, Result, RingBuffer, Sequence, Sequencer, SingleProducerSequencer, WaitStrategy,
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
///     BlockingWaitStrategy::new(),
/// ).unwrap();
/// ```
#[derive(Debug)]
pub struct Disruptor<T, W>
where
    T: Send + Sync + std::fmt::Debug + 'static,
    W: WaitStrategy + 'static,
{
    /// The ring buffer for storing events
    ring_buffer: Arc<RingBuffer<T>>,
    /// The sequencer for coordinating access (enum dispatch, no vtable)
    sequencer: SequencerEnum<W>,
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
    /// Serializes claim-path access for `ProducerType::Single` when the DSL is
    /// shared across threads (`Disruptor` is `Sync`, publish takes `&self`).
    /// Multi mode never takes this lock (soundness audit 2026-07-18).
    single_publish_lock: parking_lot::Mutex<()>,
}

impl<T, W> Disruptor<T, W>
where
    T: Send + Sync + std::fmt::Debug + 'static,
    W: WaitStrategy + 'static,
{
    /// Create a new Disruptor
    ///
    /// # Arguments
    /// * `event_factory` - Factory for creating events
    /// * `buffer_size` - Size of the ring buffer (must be a power of 2)
    /// * `producer_type` - Whether to use single or multi producer
    /// * `wait_strategy` - Strategy for waiting for events (monomorphized)
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
        wait_strategy: W,
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
        let wait_strategy = Arc::new(wait_strategy);
        let sequencer: SequencerEnum<W> = match producer_type {
            // SAFETY: every DSL path that drives the claim methods
            // (publish_event, try_publish_event, get_remaining_capacity) takes
            // `single_publish_lock` first, so claim access is serialized with
            // proper happens-before even when the Disruptor is shared.
            ProducerType::Single => SequencerEnum::Single(Arc::new(unsafe {
                SingleProducerSequencer::new(buffer_size, wait_strategy)
            })),
            ProducerType::Multi => SequencerEnum::Multi(Arc::new(MultiProducerSequencer::new(
                buffer_size,
                wait_strategy,
            ))),
        };

        Ok(Self {
            ring_buffer,
            sequencer,
            buffer_size,
            event_processors: Vec::new(),
            thread_handles: Vec::new(),
            started: false,
            shutdown_flag: Arc::new(AtomicBool::new(false)),
            single_publish_lock: parking_lot::Mutex::new(()),
        })
    }
}

impl<T> Disruptor<T, BlockingWaitStrategy>
where
    T: Send + Sync + std::fmt::Debug + 'static,
{
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
            BlockingWaitStrategy::new(),
        )
    }
}

impl<T, W> Disruptor<T, W>
where
    T: Send + Sync + std::fmt::Debug + 'static,
    W: WaitStrategy + 'static,
{
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
        let _guard = self.claim_guard();
        self.sequencer.remaining_capacity()
    }

    /// Lock serializing single-producer claim access; no-op (None) for multi.
    fn claim_guard(&self) -> Option<parking_lot::MutexGuard<'_, ()>> {
        match &self.sequencer {
            SequencerEnum::Single(_) => Some(self.single_publish_lock.lock()),
            SequencerEnum::Multi(_) => None,
        }
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
    pub fn handle_events_with<H>(self, event_handler: H) -> DisruptorBuilder<T, W>
    where
        H: EventHandler<T> + 'static,
    {
        // Default exception handler follows LMAX FatalExceptionHandler
        // semantics: log and stop the processor (2026-07-18 audit).
        self.handle_events_with_exception_handler(
            event_handler,
            Box::new(crate::disruptor::DefaultExceptionHandler::new()),
        )
    }

    /// Handle events with an explicit [`crate::disruptor::ExceptionHandler`]
    ///
    /// The exception handler's [`crate::disruptor::ErrorDecision`] controls
    /// whether a handler error stops this processor (LMAX fatal default) or
    /// skips the failing event and continues
    /// (e.g. `IgnoreExceptionHandler`).
    ///
    /// # Arguments
    /// * `event_handler` - The handler to process events
    /// * `exception_handler` - Policy for handler errors
    ///
    /// # Returns
    /// A DisruptorBuilder for further configuration
    pub fn handle_events_with_exception_handler<H>(
        mut self,
        event_handler: H,
        exception_handler: Box<dyn crate::disruptor::ExceptionHandler<T>>,
    ) -> DisruptorBuilder<T, W>
    where
        H: EventHandler<T> + 'static,
    {
        // Create a sequence barrier
        let barrier = self.sequencer.new_barrier(vec![]);

        // Create the event processor (owned handler; dyn only at EventProcessor trait)
        // SAFETY: this DSL builds a sequential chain — the first stage gates on
        // the cursor and every later stage (via then()) gates on the previous
        // stage's sequence, so each processor has an exclusive barrier-ordered
        // window over the slots it mutates.
        let processor = unsafe {
            BatchEventProcessor::new(
                self.ring_buffer.clone(),
                barrier,
                event_handler,
                exception_handler,
            )
        };

        let processor_sequence = processor.get_sequence();
        let processor = Arc::new(processor);

        // Add the processor's sequence as a gating sequence
        self.sequencer
            .add_gating_sequences(std::slice::from_ref(&processor_sequence));

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
            return Err(DisruptorError::AlreadyRunning);
        }

        // Reset shutdown flag
        self.shutdown_flag.store(false, Ordering::Release);

        // Start all event processors in their own threads
        // Each processor runs its own blocking event loop.
        for (index, processor) in self.event_processors.iter().enumerate() {
            // Clone the processor for the thread
            let processor_clone = Arc::clone(processor);
            #[cfg(not(debug_assertions))]
            let _ = index;

            let handle = thread::spawn(move || -> Result<()> {
                crate::internal_debug!("Event processor {index} starting");
                processor_clone.run()?;
                crate::internal_debug!("Event processor {index} shutting down");
                Ok(())
            });

            while !processor.is_running() {
                if handle.is_finished() {
                    break;
                }
                thread::yield_now();
            }

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

        // Halt all event processors: running=false (Release) + barrier alert
        // wakes waiters. The alert/join pair below is the synchronization —
        // the old extra SeqCst fence and fixed 1ms sleep added nothing
        // (2026-07-18 audit cleanup).
        for processor in &self.event_processors {
            processor.halt();
        }

        // Wait for all threads to complete with better error handling
        let mut join_errors = Vec::new();
        while let Some(handle) = self.thread_handles.pop() {
            match handle.join() {
                Ok(Ok(())) => {
                    // Thread completed successfully
                }
                Ok(Err(e)) => {
                    let err_msg = format!("Event processor thread returned error: {e:?}");
                    crate::internal_error!("{err_msg}");
                    join_errors.push(err_msg);
                }
                Err(_) => {
                    let err_msg = "Event processor thread panicked".to_string();
                    crate::internal_error!("{err_msg}");
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
        // Serializes single-producer claim state when the DSL is shared
        // across threads; no-op for multi (soundness audit 2026-07-18).
        let _guard = self.claim_guard();
        let sequence = self.sequencer.next()?;

        // SAFETY: The sequencer granted exclusive access to this sequence until
        // we publish it. Use the unchecked access path to avoid the `&mut self`
        // borrow requirement on the ring buffer (we only hold `&self`).
        let event = unsafe { &mut *self.ring_buffer.get_mut_unchecked(sequence) };

        // Use the translator to populate the event with data. Poison on panic:
        // an unpublished claim would expose a never-written slot later.
        let poison_guard = crate::disruptor::producer::PoisonOnPanic(&self.sequencer);
        translator.translate_to(event, sequence);
        drop(poison_guard);

        // Publish the sequence to make it available to consumers
        self.sequencer.publish(sequence);
        drop(translator);
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
        // Serializes single-producer claim state when the DSL is shared
        // across threads; no-op for multi (soundness audit 2026-07-18).
        let _guard = self.claim_guard();
        if let Some(sequence) = self.sequencer.try_next() {
            // SAFETY: The sequencer granted exclusive access to this sequence.
            let event = unsafe { &mut *self.ring_buffer.get_mut_unchecked(sequence) };

            // Use the translator to populate the event with data. Poison on
            // panic: an unpublished claim would expose a never-written slot.
            let poison_guard = crate::disruptor::producer::PoisonOnPanic(&self.sequencer);
            translator.translate_to(event, sequence);
            drop(poison_guard);

            // Publish the sequence to make it available to consumers
            self.sequencer.publish(sequence);
            drop(translator);
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
pub struct DisruptorBuilder<T, W>
where
    T: Send + Sync + std::fmt::Debug + 'static,
    W: WaitStrategy + 'static,
{
    disruptor: Disruptor<T, W>,
    last_processor_sequences: Vec<Arc<Sequence>>,
}

impl<T, W> DisruptorBuilder<T, W>
where
    T: Send + Sync + std::fmt::Debug + 'static,
    W: WaitStrategy + 'static,
{
    /// Add another event handler that depends on the previous handlers
    ///
    /// # Arguments
    /// * `event_handler` - The handler to process events
    ///
    /// # Returns
    /// A new DisruptorBuilder for further configuration
    #[must_use]
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
        let processor = unsafe {
            BatchEventProcessor::new(
                self.disruptor.ring_buffer.clone(),
                barrier,
                event_handler,
                Box::new(crate::disruptor::DefaultExceptionHandler::new()),
            )
        };

        let processor_sequence = processor.get_sequence();
        let processor = Arc::new(processor);

        // Add the processor's sequence as a gating sequence
        self.disruptor
            .sequencer
            .add_gating_sequences(std::slice::from_ref(&processor_sequence));

        self.disruptor.event_processors.push(processor);
        self.last_processor_sequences = vec![processor_sequence];

        self
    }

    /// Finish building and return the configured Disruptor
    ///
    /// # Returns
    /// The configured Disruptor instance
    pub fn build(self) -> Disruptor<T, W> {
        self.disruptor
    }
}

// DataProvider implementation for RingBuffer is now provided in ring_buffer.rs
// to avoid duplication and maintain a single source of truth

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disruptor::{
        DefaultEventFactory, NoOpEventHandler, SleepingWaitStrategy, YieldingWaitStrategy,
    };
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::sync::Arc;

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
            BlockingWaitStrategy::new(),
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
            YieldingWaitStrategy::new(),
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
            BlockingWaitStrategy::new(),
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
            BlockingWaitStrategy::new(),
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
        use std::time::{Duration, Instant};

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

        // Give the blocking processor time to park, then verify alert-driven shutdown.
        std::thread::sleep(Duration::from_millis(5));
        let shutdown_start = Instant::now();
        disruptor.shutdown().unwrap();
        let shutdown_elapsed = shutdown_start.elapsed();

        assert!(!disruptor.started);
        assert!(
            shutdown_elapsed < Duration::from_millis(100),
            "shutdown should interrupt blocking waits promptly, elapsed={shutdown_elapsed:?}"
        );

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
            SleepingWaitStrategy::new(),
        )
        .unwrap();
        assert_eq!(disruptor1.get_buffer_size(), 64);

        // Test with YieldingWaitStrategy
        let disruptor2 = Disruptor::new(
            factory2,
            128,
            ProducerType::Multi,
            YieldingWaitStrategy::new(),
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
        let event = unsafe { ring_buffer.get(0) };
        assert_eq!(event.value, 0); // Default value

        // Test unchecked mutable access (the path used by producers/processors)
        unsafe {
            let event_mut = &mut *ring_buffer.get_mut_unchecked(0);
            event_mut.value = 999;
        }

        // Verify the change
        let event = unsafe { ring_buffer.get(0) };
        assert_eq!(event.value, 999);
    }
}
