//! Event Processor Implementation
//!
//! This module provides event processors for consuming events from the Disruptor.
//! Event processors run the main event processing loop and coordinate with
//! sequence barriers to ensure proper ordering and dependencies.

use crate::disruptor::{
    consumer_engine::{self, StopFlag},
    DisruptorError, EventHandler, ExceptionHandler, ProcessingSequenceBarrier, Result, RingBuffer,
    Sequence, SequenceBarrier, WaitStrategy,
};
use parking_lot::Mutex;
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
    fn run(&self) -> Result<()>;

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
    ///
    /// # Safety
    /// The caller must guarantee, via the sequencing protocol, that the slot at
    /// `sequence` is published and will not be written (by a producer claim or
    /// wrap-around) for as long as the returned reference is alive. Holding the
    /// reference across a concurrent write is undefined behavior.
    unsafe fn get(&self, sequence: i64) -> &T;

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
/// The processor stores a concrete `Arc<RingBuffer<T>>`, monomorphized
/// `ProcessingSequenceBarrier<W>`, and owned handler `H` so the hot loop has no
/// dyn vtable or Mutex lock per event/batch.
///
/// # Type Parameters
/// * `T` - The event type being processed
/// * `H` - The event handler type (owned, monomorphized)
/// * `W` - The wait strategy type (monomorphized through the barrier)
#[repr(align(64))] // Cache line alignment for performance
pub struct BatchEventProcessor<T, H, W>
where
    T: Send + Sync,
    H: EventHandler<T>,
    W: WaitStrategy + 'static,
{
    /// The ring buffer that stores and serves events
    data_provider: Arc<RingBuffer<T>>,
    /// The sequence barrier for coordination (monomorphized wait strategy)
    sequence_barrier: Arc<ProcessingSequenceBarrier<W>>,
    /// Owned event handler. The mutex is taken once per `run()` (held across the
    /// whole processing loop) and with `try_lock` by the cold lifecycle methods,
    /// so there is no per-event locking cost while concurrent misuse degrades to
    /// a skipped callback instead of aliased `&mut H` (soundness audit 2026-07-18).
    event_handler: Mutex<H>,
    /// The exception handler for error handling
    exception_handler: Box<dyn ExceptionHandler<T>>,
    /// The current sequence being processed
    sequence: Arc<Sequence>,
    /// Flag indicating if the processor is running
    running: AtomicBool,
}

impl<T, H, W> BatchEventProcessor<T, H, W>
where
    T: Send + Sync + 'static,
    H: EventHandler<T> + 'static,
    W: WaitStrategy + 'static,
{
    /// Create a new batch event processor
    ///
    /// # Arguments
    /// * `data_provider` - The ring buffer serving events to this processor
    /// * `sequence_barrier` - The monomorphized sequence barrier for coordination
    /// * `event_handler` - The event handler for processing events (owned)
    /// * `exception_handler` - The exception handler for error handling
    ///
    /// # Returns
    /// A new BatchEventProcessor instance
    ///
    /// # Safety
    /// The processor takes `&mut` access to every slot the barrier reports
    /// available. The caller must guarantee the topology invariant: the barrier
    /// only releases sequences that are published and that no other processor
    /// or consumer accesses mutably while this processor is between the
    /// barrier wait and its sequence update. The DSL (`Disruptor`) discharges
    /// this by giving each pipeline stage an exclusive barrier-ordered window.
    pub unsafe fn new(
        data_provider: Arc<RingBuffer<T>>,
        sequence_barrier: Arc<ProcessingSequenceBarrier<W>>,
        event_handler: H,
        exception_handler: Box<dyn ExceptionHandler<T>>,
    ) -> Self {
        Self {
            data_provider,
            sequence_barrier,
            event_handler: Mutex::new(event_handler),
            exception_handler,
            sequence: Arc::new(Sequence::new_with_initial_value()),
            running: AtomicBool::new(false),
        }
    }

    /// Process events using the unified sequential consumer engine.
    fn process_events(&self) {
        self.on_start();

        // Halt may race with on_start (start() returns as soon as `running` is
        // true, which is set by `run()`'s CAS before on_start). Re-check here so
        // we never enter the wait loop after an early halt undid the race window.
        if !self.running.load(Ordering::Acquire) {
            self.on_shutdown();
            return;
        }

        // One lock acquisition for the whole loop: no per-event cost.
        let mut handler = self.event_handler.lock();
        consumer_engine::run_sequential_batch_loop_with_exceptions(
            &self.data_provider,
            &self.sequence_barrier,
            &mut *handler,
            &self.sequence,
            StopFlag::Running(&self.running),
            "batch-event-processor",
            Some(self.exception_handler.as_ref()),
        );
        drop(handler);

        self.on_shutdown();
    }
}

impl<T, H, W> EventProcessor for BatchEventProcessor<T, H, W>
where
    T: Send + Sync + 'static,
    H: EventHandler<T> + 'static,
    W: WaitStrategy + 'static,
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

    fn run(&self) -> Result<()> {
        // Check if already running
        if self
            .running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(DisruptorError::AlreadyRunning);
        }

        // Clear any existing alerts
        self.sequence_barrier.clear_alert();

        // Run the main processing loop. A panicking handler kills this
        // processor; poison the producers so blocking publishes fail fast
        // instead of spinning on a dead gating sequence (2026-07-18 audit).
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.process_events();
        }));
        if let Err(payload) = result {
            self.sequence_barrier.poison_producers();
            self.running.store(false, Ordering::Release);
            std::panic::resume_unwind(payload);
        }
        // process_events → on_shutdown normally clears this; belt-and-suspenders
        // so a clean exit always reports not-running.
        self.running.store(false, Ordering::Release);
        Ok(())
    }

    fn try_run_once(&self) -> Result<bool> {
        // Check if we're running
        if !self.running.load(Ordering::Acquire) {
            return Ok(false);
        }

        // Exclusive handler access for the whole probe+process step. If the lock
        // is contended the main run() loop is processing; report no work done.
        let Some(mut handler_guard) = self.event_handler.try_lock() else {
            return Ok(false);
        };
        let handler = &mut *handler_guard;

        let next_sequence = self.sequence.get() + 1;

        // Probe once without blocking. This is intended for tests and polling-style
        // integrations, not the main high-performance run loop.
        let available_sequence = match self
            .sequence_barrier
            .wait_for_with_timeout(next_sequence, std::time::Duration::ZERO)
        {
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
        let batch_span = available_sequence - next_sequence + 1;

        // Notify batch start with batch size and queue depth
        let queue_depth = available_sequence - self.sequence.get();
        let _ = handler.on_batch_start(batch_span, queue_depth);

        while current_sequence <= available_sequence {
            let end_of_batch = current_sequence == available_sequence;

            // SAFETY: We have exclusive access to this sequence position as guaranteed by
            // the barrier wait — no other processor can access this sequence until we're done.
            let event = unsafe { &mut *self.data_provider.get_mut_unchecked(current_sequence) };

            if let Err(e) = handler.on_event(event, current_sequence, end_of_batch) {
                let exception_handler = &*self.exception_handler;
                let decision = exception_handler.handle_event_exception(e, current_sequence, event);
                if decision == crate::disruptor::ErrorDecision::Stop {
                    // Freeze at the last fully processed event, poison the
                    // producers, and stop this processor (LMAX fatal default).
                    self.sequence.set(current_sequence - 1);
                    self.sequence_barrier.poison_producers();
                    self.running.store(false, Ordering::Release);
                    return Err(DisruptorError::Poisoned);
                }
            }

            current_sequence += 1;
        }

        // Update our sequence to indicate we've processed up to this point.
        // Release ordering ensures all prior event reads/writes are visible
        // before the sequence advance. Producers read this with Acquire,
        // forming a correct Release-Acquire pair. SeqCst is unnecessary here
        // (LMAX Java BatchEventProcessor also uses lazySet = Release).
        self.sequence.set(available_sequence);

        Ok(true)
    }

    fn notify_timeout(&self, sequence: i64) {
        // Contended lock means the run() loop owns the handler; it observes
        // timeouts itself, so skipping the callback here loses nothing.
        if let Some(mut handler) = self.event_handler.try_lock() {
            let _ = handler.on_timeout(sequence);
        }
    }

    fn on_start(&self) {
        // Contended lock means the processor is already being driven by another
        // thread; starting it twice concurrently is a misuse we degrade gracefully.
        let Some(mut handler) = self.event_handler.try_lock() else {
            return;
        };
        // Do NOT clear_alert() or store running=true here.
        // `run()` already CAS-sets running and clears the alert before this
        // callback. Re-asserting those flags races with `halt()`: start() returns
        // as soon as is_running() is true (post-CAS), so a quick shutdown can
        // alert and set running=false, only for this callback to undo both and
        // leave the consumer parked forever on an empty ring (seen hanging
        // doctests for hours on BlockingWaitStrategy).
        if let Err(e) = handler.on_start() {
            self.exception_handler.handle_on_start_exception(e);
        }
    }

    fn on_shutdown(&self) {
        let Some(mut handler) = self.event_handler.try_lock() else {
            return;
        };
        if let Err(e) = handler.on_shutdown() {
            self.exception_handler.handle_on_shutdown_exception(e);
        }
        drop(handler);
        self.running.store(false, Ordering::Release);
    }
}

impl<T, H, W> std::fmt::Debug for BatchEventProcessor<T, H, W>
where
    T: Send + Sync,
    H: EventHandler<T>,
    W: WaitStrategy + 'static,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BatchEventProcessor")
            .field("sequence", &self.sequence)
            .field("running", &self.running)
            .finish_non_exhaustive()
    }
}

impl<T, H, W> BatchEventProcessor<T, H, W>
where
    T: Send + Sync + 'static,
    H: EventHandler<T> + 'static,
    W: WaitStrategy + 'static,
{
    /// Spawn this processor in a new thread
    ///
    /// # Returns
    /// A join handle for the spawned thread
    pub fn spawn(self) -> JoinHandle<Result<()>> {
        thread::spawn(move || self.run())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disruptor::sequencer::Sequencer;
    use crate::disruptor::SingleProducerSequencer;
    use crate::disruptor::{
        BlockingWaitStrategy, DefaultEventFactory, DefaultExceptionHandler, DisruptorError,
        ExceptionHandler, NoOpEventHandler, ProcessingSequenceBarrier, RingBuffer, SequencerEnum,
        INITIAL_CURSOR_VALUE,
    };
    use std::convert::TryFrom;
    use std::sync::atomic::{AtomicI64, AtomicUsize};
    use std::time::{Duration, Instant};

    // Helper function to create a test sequence barrier
    fn create_test_sequence_barrier(
        cursor: Arc<Sequence>,
        wait_strategy: Arc<BlockingWaitStrategy>,
    ) -> Arc<ProcessingSequenceBarrier<BlockingWaitStrategy>> {
        let sequencer = crate::disruptor::sequencer::SequencerEnum::Single(Arc::new(unsafe {
            SingleProducerSequencer::new(16, wait_strategy.clone())
        }));
        Arc::new(ProcessingSequenceBarrier::new(
            cursor,
            wait_strategy,
            vec![],
            sequencer,
        ))
    }

    // Helper: create a small RingBuffer backed by the default event factory.
    // BatchEventProcessor now stores a concrete Arc<RingBuffer<T>>, so tests
    // that only need a data source use this instead of the trait-based mock.
    fn create_test_ring_buffer(size: usize) -> Arc<RingBuffer<TestEvent>> {
        Arc::new(
            RingBuffer::new(size, DefaultEventFactory::<TestEvent>::new())
                .expect("test ring buffer creation must succeed"),
        )
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
                    value: AtomicI64::new(i64::try_from(i).expect("test index fits in i64")),
                });
            }
            Self { events }
        }
    }

    impl DataProvider<TestEvent> for TestDataProvider {
        unsafe fn get(&self, sequence: i64) -> &TestEvent {
            let len = self.events.len();
            let len_i64 = i64::try_from(len).expect("events length fits in i64");
            let normalized = sequence.rem_euclid(len_i64);
            let index = usize::try_from(normalized).expect("normalized index must fit");
            &self.events[index]
        }

        unsafe fn get_mut(&self, sequence: i64) -> &mut TestEvent {
            let len = self.events.len();
            let len_i64 = i64::try_from(len).expect("events length fits in i64");
            let normalized = sequence.rem_euclid(len_i64);
            let index = usize::try_from(normalized).expect("normalized index must fit");
            // This is unsafe but acceptable for testing
            &mut *self.events.as_ptr().add(index).cast_mut()
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

    #[test]
    fn test_batch_event_processor_creation() {
        use crate::disruptor::SingleProducerSequencer;

        let data_provider = create_test_ring_buffer(8);
        let cursor = Arc::new(Sequence::new(INITIAL_CURSOR_VALUE));
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = crate::disruptor::sequencer::SequencerEnum::Single(Arc::new(unsafe {
            SingleProducerSequencer::new(16, wait_strategy.clone())
        }));
        let sequence_barrier = Arc::new(ProcessingSequenceBarrier::new(
            cursor,
            wait_strategy,
            vec![],
            sequencer,
        ));
        let event_handler = NoOpEventHandler::<TestEvent>::new();
        let exception_handler = Box::new(DefaultExceptionHandler::<TestEvent>::new());

        let processor = unsafe {
            BatchEventProcessor::new(
                data_provider,
                sequence_barrier,
                event_handler,
                exception_handler,
            )
        };

        assert_eq!(processor.get_sequence().get(), INITIAL_CURSOR_VALUE);
        assert!(!processor.is_running());
    }

    #[test]
    fn test_event_processor_halt() {
        let data_provider = create_test_ring_buffer(8);
        let cursor = Arc::new(Sequence::new(INITIAL_CURSOR_VALUE));
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequence_barrier = create_test_sequence_barrier(cursor, wait_strategy);
        let event_handler = NoOpEventHandler::<TestEvent>::new();
        let exception_handler = Box::new(DefaultExceptionHandler::<TestEvent>::new());

        let processor = unsafe {
            BatchEventProcessor::new(
                data_provider,
                sequence_barrier,
                event_handler,
                exception_handler,
            )
        };

        assert!(!processor.is_running());
        processor.halt();
        assert!(!processor.is_running());
    }

    #[test]
    fn test_try_run_once_not_running() {
        let data_provider = create_test_ring_buffer(8);
        let cursor = Arc::new(Sequence::new(INITIAL_CURSOR_VALUE));
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequence_barrier = create_test_sequence_barrier(cursor, wait_strategy);
        let event_handler = NoOpEventHandler::<TestEvent>::new();
        let exception_handler = Box::new(DefaultExceptionHandler::<TestEvent>::new());

        let processor = unsafe {
            BatchEventProcessor::new(
                data_provider,
                sequence_barrier,
                event_handler,
                exception_handler,
            )
        };

        // Should return false when not running
        let result = processor.try_run_once();
        assert!(result.is_ok());
        assert!(!result.unwrap());
    }

    #[test]
    fn test_try_run_once_is_non_blocking_without_events() {
        use std::time::{Duration, Instant};

        let data_provider = create_test_ring_buffer(8);
        let cursor = Arc::new(Sequence::new(INITIAL_CURSOR_VALUE));
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequence_barrier = create_test_sequence_barrier(cursor, wait_strategy);
        let event_handler = NoOpEventHandler::<TestEvent>::new();
        let exception_handler = Box::new(DefaultExceptionHandler::<TestEvent>::new());

        let processor = unsafe {
            BatchEventProcessor::new(
                data_provider,
                sequence_barrier,
                event_handler,
                exception_handler,
            )
        };

        processor.running.store(true, Ordering::Release);

        let start = Instant::now();
        let result = processor.try_run_once();
        let elapsed = start.elapsed();

        assert!(matches!(result, Ok(false)));
        assert!(
            elapsed < Duration::from_millis(5),
            "try_run_once should probe immediately, elapsed={elapsed:?}"
        );
    }

    #[test]
    fn test_processor_sequence_management() {
        let data_provider = create_test_ring_buffer(8);
        let cursor = Arc::new(Sequence::new(INITIAL_CURSOR_VALUE));
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequence_barrier = create_test_sequence_barrier(cursor, wait_strategy);
        let event_handler = NoOpEventHandler::<TestEvent>::new();
        let exception_handler = Box::new(DefaultExceptionHandler::<TestEvent>::new());

        let processor = unsafe {
            BatchEventProcessor::new(
                data_provider,
                sequence_barrier,
                event_handler,
                exception_handler,
            )
        };

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
        let event0 = unsafe { provider.get(0) };
        assert_eq!(event0.value.load(Ordering::Relaxed), 0);

        let event1 = unsafe { provider.get(1) };
        assert_eq!(event1.value.load(Ordering::Relaxed), 1);

        // Test wrapping behavior
        let event4 = unsafe { provider.get(4) }; // Should wrap to index 0
        assert_eq!(event4.value.load(Ordering::Relaxed), 0);

        // Test get_mut method
        unsafe {
            let event_mut = provider.get_mut(2);
            event_mut.value.store(999, Ordering::Relaxed);
        }

        let event2 = unsafe { provider.get(2) };
        assert_eq!(event2.value.load(Ordering::Relaxed), 999);
    }

    #[test]
    fn test_processor_debug_format() {
        let data_provider = create_test_ring_buffer(8);
        let cursor = Arc::new(Sequence::new(INITIAL_CURSOR_VALUE));
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequence_barrier = create_test_sequence_barrier(cursor, wait_strategy);
        let event_handler = NoOpEventHandler::<TestEvent>::new();
        let exception_handler = Box::new(DefaultExceptionHandler::<TestEvent>::new());

        let processor = unsafe {
            BatchEventProcessor::new(
                data_provider,
                sequence_barrier,
                event_handler,
                exception_handler,
            )
        };

        let debug_str = format!("{processor:?}");
        assert!(debug_str.contains("BatchEventProcessor"));
        assert!(debug_str.contains("sequence"));
        assert!(debug_str.contains("running"));
    }

    #[derive(Default)]
    struct HandlerState {
        successful_events: AtomicI64,
        processed_sequences: Mutex<Vec<i64>>,
        batch_starts: Mutex<Vec<(i64, i64)>>,
        start_calls: AtomicUsize,
        shutdown_calls: AtomicUsize,
        last_timeout_sequence: AtomicI64,
    }

    struct TrackingEventHandler {
        state: Arc<HandlerState>,
        fail_on_sequence: Option<i64>,
        fail_on_start: bool,
        fail_on_shutdown: bool,
    }

    impl TrackingEventHandler {
        fn new(state: Arc<HandlerState>) -> Self {
            Self {
                state,
                fail_on_sequence: None,
                fail_on_start: false,
                fail_on_shutdown: false,
            }
        }

        fn failing_on_sequence(state: Arc<HandlerState>, sequence: i64) -> Self {
            Self {
                state,
                fail_on_sequence: Some(sequence),
                fail_on_start: false,
                fail_on_shutdown: false,
            }
        }

        fn failing_lifecycle(
            state: Arc<HandlerState>,
            fail_on_start: bool,
            fail_on_shutdown: bool,
        ) -> Self {
            Self {
                state,
                fail_on_sequence: None,
                fail_on_start,
                fail_on_shutdown,
            }
        }
    }

    impl EventHandler<TestEvent> for TrackingEventHandler {
        fn on_event(
            &mut self,
            event: &mut TestEvent,
            sequence: i64,
            _end_of_batch: bool,
        ) -> Result<()> {
            if self.fail_on_sequence == Some(sequence) {
                return Err(DisruptorError::InvalidSequence(sequence));
            }

            self.state.successful_events.fetch_add(1, Ordering::Relaxed);
            self.state
                .processed_sequences
                .lock()
                .unwrap()
                .push(sequence);
            event.value.store(sequence, Ordering::Relaxed);
            Ok(())
        }

        fn on_start(&mut self) -> Result<()> {
            self.state.start_calls.fetch_add(1, Ordering::Relaxed);
            if self.fail_on_start {
                return Err(DisruptorError::Shutdown);
            }
            Ok(())
        }

        fn on_shutdown(&mut self) -> Result<()> {
            self.state.shutdown_calls.fetch_add(1, Ordering::Relaxed);
            if self.fail_on_shutdown {
                return Err(DisruptorError::Shutdown);
            }
            Ok(())
        }

        fn on_timeout(&mut self, available_sequence: i64) -> Result<()> {
            self.state
                .last_timeout_sequence
                .store(available_sequence, Ordering::Relaxed);
            Ok(())
        }

        fn on_batch_start(&mut self, batch_size: i64, available_size: i64) -> Result<()> {
            self.state
                .batch_starts
                .lock()
                .unwrap()
                .push((batch_size, available_size));
            Ok(())
        }
    }

    #[derive(Default)]
    struct ExceptionState {
        event_error_sequences: Mutex<Vec<i64>>,
        start_errors: AtomicUsize,
        shutdown_errors: AtomicUsize,
    }

    struct RecordingExceptionHandler {
        state: Arc<ExceptionState>,
    }

    impl RecordingExceptionHandler {
        fn new(state: Arc<ExceptionState>) -> Self {
            Self { state }
        }
    }

    impl ExceptionHandler<TestEvent> for RecordingExceptionHandler {
        fn handle_event_exception(
            &self,
            _error: DisruptorError,
            sequence: i64,
            _event: &TestEvent,
        ) -> crate::disruptor::ErrorDecision {
            self.state
                .event_error_sequences
                .lock()
                .unwrap()
                .push(sequence);
            // Recording handler skips and continues (tests assert sequences
            // keep advancing past failing events).
            crate::disruptor::ErrorDecision::Continue
        }

        fn handle_on_start_exception(&self, _error: DisruptorError) {
            self.state.start_errors.fetch_add(1, Ordering::Relaxed);
        }

        fn handle_on_shutdown_exception(&self, _error: DisruptorError) {
            self.state.shutdown_errors.fetch_add(1, Ordering::Relaxed);
        }
    }

    use std::sync::Mutex;

    #[test]
    fn test_try_run_once_processes_available_batch_and_updates_sequence() {
        let factory = DefaultEventFactory::<TestEvent>::new();
        let ring_buffer = Arc::new(RingBuffer::new(16, factory).unwrap());
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer =
            Arc::new(unsafe { SingleProducerSequencer::new(16, wait_strategy.clone()) });
        let barrier = Arc::new(ProcessingSequenceBarrier::new(
            sequencer.get_cursor(),
            wait_strategy,
            vec![],
            SequencerEnum::Single(sequencer.clone()),
        ));

        let handler_state = Arc::new(HandlerState::default());
        let exception_state = Arc::new(ExceptionState::default());
        let processor = unsafe {
            BatchEventProcessor::new(
                ring_buffer.clone(),
                barrier,
                TrackingEventHandler::new(handler_state.clone()),
                Box::new(RecordingExceptionHandler::new(exception_state.clone())),
            )
        };

        processor.on_start();

        for sequence in 0..=1 {
            let claimed = sequencer.next().unwrap();
            assert_eq!(claimed, sequence);
            unsafe {
                ring_buffer
                    .get_mut(claimed)
                    .value
                    .store(sequence, Ordering::Relaxed);
            }
            sequencer.publish(claimed);
        }

        assert!(processor.try_run_once().unwrap());
        assert_eq!(processor.get_sequence().get(), 1);
        assert_eq!(handler_state.successful_events.load(Ordering::Relaxed), 2);
        assert_eq!(
            *handler_state.processed_sequences.lock().unwrap(),
            vec![0, 1]
        );
        assert_eq!(*handler_state.batch_starts.lock().unwrap(), vec![(2, 2)]);
        assert!(exception_state
            .event_error_sequences
            .lock()
            .unwrap()
            .is_empty());

        processor.on_shutdown();
    }

    #[test]
    fn test_try_run_once_forwards_event_exceptions_and_continues_batch() {
        let factory = DefaultEventFactory::<TestEvent>::new();
        let ring_buffer = Arc::new(RingBuffer::new(16, factory).unwrap());
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer =
            Arc::new(unsafe { SingleProducerSequencer::new(16, wait_strategy.clone()) });
        let barrier = Arc::new(ProcessingSequenceBarrier::new(
            sequencer.get_cursor(),
            wait_strategy,
            vec![],
            SequencerEnum::Single(sequencer.clone()),
        ));

        let handler_state = Arc::new(HandlerState::default());
        let exception_state = Arc::new(ExceptionState::default());
        let processor = unsafe {
            BatchEventProcessor::new(
                ring_buffer.clone(),
                barrier,
                TrackingEventHandler::failing_on_sequence(handler_state.clone(), 0),
                Box::new(RecordingExceptionHandler::new(exception_state.clone())),
            )
        };

        processor.on_start();

        for sequence in 0..=1 {
            let claimed = sequencer.next().unwrap();
            unsafe {
                ring_buffer
                    .get_mut(claimed)
                    .value
                    .store(sequence + 10, Ordering::Relaxed);
            }
            sequencer.publish(claimed);
        }

        assert!(processor.try_run_once().unwrap());
        assert_eq!(processor.get_sequence().get(), 1);
        assert_eq!(handler_state.successful_events.load(Ordering::Relaxed), 1);
        assert_eq!(*handler_state.processed_sequences.lock().unwrap(), vec![1]);
        assert_eq!(
            *exception_state.event_error_sequences.lock().unwrap(),
            vec![0]
        );

        processor.on_shutdown();
    }

    #[test]
    fn test_run_processes_events_and_invokes_lifecycle_hooks() {
        let factory = DefaultEventFactory::<TestEvent>::new();
        let ring_buffer = Arc::new(RingBuffer::new(16, factory).unwrap());
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer =
            Arc::new(unsafe { SingleProducerSequencer::new(16, wait_strategy.clone()) });
        let barrier = Arc::new(ProcessingSequenceBarrier::new(
            sequencer.get_cursor(),
            wait_strategy,
            vec![],
            SequencerEnum::Single(sequencer.clone()),
        ));

        let handler_state = Arc::new(HandlerState::default());
        let processor = Arc::new(unsafe {
            BatchEventProcessor::new(
                ring_buffer.clone(),
                barrier,
                TrackingEventHandler::new(handler_state.clone()),
                Box::new(DefaultExceptionHandler::<TestEvent>::new()),
            )
        });

        let processor_thread = {
            let processor = Arc::clone(&processor);
            std::thread::spawn(move || processor.run())
        };

        wait_until(
            Duration::from_secs(1),
            || processor.is_running(),
            "processor to enter running state",
        );

        for sequence in 0..=2 {
            let claimed = sequencer.next().unwrap();
            unsafe {
                ring_buffer
                    .get_mut(claimed)
                    .value
                    .store(sequence, Ordering::Relaxed);
            }
            sequencer.publish(claimed);
        }

        wait_until(
            Duration::from_secs(1),
            || handler_state.successful_events.load(Ordering::Relaxed) == 3,
            "processor run loop to consume all published events",
        );

        processor.halt();
        processor_thread.join().unwrap().unwrap();

        assert_eq!(handler_state.start_calls.load(Ordering::Relaxed), 1);
        assert_eq!(handler_state.shutdown_calls.load(Ordering::Relaxed), 1);
        assert_eq!(
            *handler_state.processed_sequences.lock().unwrap(),
            vec![0, 1, 2]
        );
    }

    #[test]
    fn test_run_returns_already_running_when_invoked_twice() {
        let data_provider = create_test_ring_buffer(8);
        let cursor = Arc::new(Sequence::new(INITIAL_CURSOR_VALUE));
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequence_barrier = create_test_sequence_barrier(cursor, wait_strategy);
        let event_handler = NoOpEventHandler::<TestEvent>::new();
        let exception_handler = Box::new(DefaultExceptionHandler::<TestEvent>::new());

        let processor = unsafe {
            BatchEventProcessor::new(
                data_provider,
                sequence_barrier,
                event_handler,
                exception_handler,
            )
        };

        processor.running.store(true, Ordering::Release);
        assert!(matches!(
            processor.run(),
            Err(DisruptorError::AlreadyRunning)
        ));
    }

    #[test]
    fn test_notify_timeout_invokes_handler() {
        let data_provider = create_test_ring_buffer(8);
        let cursor = Arc::new(Sequence::new(INITIAL_CURSOR_VALUE));
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequence_barrier = create_test_sequence_barrier(cursor, wait_strategy);
        let handler_state = Arc::new(HandlerState::default());
        let processor = unsafe {
            BatchEventProcessor::new(
                data_provider,
                sequence_barrier,
                TrackingEventHandler::new(handler_state.clone()),
                Box::new(DefaultExceptionHandler::<TestEvent>::new()),
            )
        };

        processor.notify_timeout(42);
        assert_eq!(
            handler_state.last_timeout_sequence.load(Ordering::Relaxed),
            42
        );
    }

    #[test]
    fn test_lifecycle_errors_are_forwarded_to_exception_handler() {
        let data_provider = create_test_ring_buffer(8);
        let cursor = Arc::new(Sequence::new(INITIAL_CURSOR_VALUE));
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequence_barrier = create_test_sequence_barrier(cursor, wait_strategy);
        let handler_state = Arc::new(HandlerState::default());
        let exception_state = Arc::new(ExceptionState::default());
        let processor = unsafe {
            BatchEventProcessor::new(
                data_provider,
                sequence_barrier,
                TrackingEventHandler::failing_lifecycle(handler_state.clone(), true, true),
                Box::new(RecordingExceptionHandler::new(exception_state.clone())),
            )
        };

        processor.on_start();
        processor.on_shutdown();

        assert_eq!(handler_state.start_calls.load(Ordering::Relaxed), 1);
        assert_eq!(handler_state.shutdown_calls.load(Ordering::Relaxed), 1);
        assert_eq!(exception_state.start_errors.load(Ordering::Relaxed), 1);
        assert_eq!(exception_state.shutdown_errors.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_processor_spawn_method_exists() {
        #[allow(clippy::type_complexity)]
        let spawn_fn: fn(
            BatchEventProcessor<TestEvent, NoOpEventHandler<TestEvent>, BlockingWaitStrategy>,
        ) -> JoinHandle<Result<()>> = BatchEventProcessor::spawn;
        let _ = spawn_fn;
    }

    // Soundness regression (P0-3, audit 2026-07-18): lifecycle methods used to
    // hand out aliased `&mut H` from `&self` via UnsafeCell when driven from two
    // threads. With the handler behind a Mutex this must be race-free (Miri-clean)
    // and the handler callbacks must never run concurrently.
    #[test]
    fn test_concurrent_on_start_is_sound() {
        #[derive(Debug)]
        struct CountingHandler {
            starts: u64,
        }
        impl crate::disruptor::EventHandler<TestEvent> for CountingHandler {
            fn on_event(
                &mut self,
                _event: &mut TestEvent,
                _sequence: i64,
                _end_of_batch: bool,
            ) -> Result<()> {
                Ok(())
            }
            fn on_start(&mut self) -> Result<()> {
                // Non-atomic mutation: aliased &mut H would be a data race here.
                self.starts += 1;
                Ok(())
            }
        }

        let ring_buffer = create_test_ring_buffer(16);
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let cursor = Arc::new(Sequence::new(INITIAL_CURSOR_VALUE));
        let barrier = create_test_sequence_barrier(cursor, wait_strategy);
        let processor = Arc::new(unsafe {
            BatchEventProcessor::new(
                ring_buffer,
                barrier,
                CountingHandler { starts: 0 },
                Box::new(DefaultExceptionHandler::new()),
            )
        });

        let threads: Vec<_> = (0..4)
            .map(|_| {
                let processor = Arc::clone(&processor);
                std::thread::spawn(move || {
                    processor.on_start();
                })
            })
            .collect();
        for t in threads {
            t.join().expect("on_start thread must not panic");
        }

        // Contended callers skip the callback; between 1 and 4 callbacks ran,
        // each under exclusive access.
        let starts = processor.event_handler.lock().starts;
        assert!((1..=4).contains(&starts), "starts = {starts}");
    }
}
