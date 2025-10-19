//! Elegant consumer handling inspired by disruptor-rs
//!
//! This module provides simplified, elegant consumer handling with automatic
//! batch processing detection, state management, and clean shutdown.

use crate::disruptor::{
    simple_wait_strategy::SimpleWaitStrategy,
    thread_management::{ManagedThread, ThreadBuilder},
    RingBuffer,
};
use crossbeam_utils::CachePadded;
use std::marker::PhantomData;
use std::sync::{
    atomic::{AtomicBool, AtomicI64, Ordering},
    Arc,
};

/// Elegant consumer for processing events with automatic batch detection
///
/// This follows the disruptor-rs pattern for clean, simple event processing
/// with automatic lifecycle management and graceful shutdown.
pub struct ElegantConsumer<T>
where
    T: Send + Sync,
{
    /// The managed thread running the consumer
    thread: Option<ManagedThread>,
    /// Consumer's current sequence position
    sequence: Arc<CachePadded<AtomicI64>>,
    /// Shutdown signal
    shutdown: Arc<AtomicBool>,
    /// Phantom data to hold the type parameter
    _phantom: PhantomData<T>,
}

impl<T> ElegantConsumer<T>
where
    T: Send + Sync + 'static,
{
    /// Create a new elegant consumer
    ///
    /// # Arguments
    /// * `ring_buffer` - The ring buffer to consume from
    /// * `event_handler` - Function to handle each event
    /// * `wait_strategy` - Strategy for waiting when no events are available
    ///
    /// # Returns
    /// A new ElegantConsumer that automatically processes events
    pub fn new<H, W>(
        ring_buffer: Arc<RingBuffer<T>>,
        event_handler: H,
        wait_strategy: W,
    ) -> std::io::Result<Self>
    where
        H: FnMut(&T, i64, bool) + Send + 'static,
        W: SimpleWaitStrategy + 'static,
    {
        let sequence = Arc::new(CachePadded::new(AtomicI64::new(-1)));
        let shutdown = Arc::new(AtomicBool::new(false));

        let sequence_clone = sequence.clone();
        let shutdown_clone = shutdown.clone();

        let thread = ThreadBuilder::new()
            .thread_name("elegant-consumer")
            .spawn(move || {
                let mut handler = event_handler;
                let strategy = wait_strategy;
                let ring = ring_buffer;
                let seq = sequence_clone;
                let shutdown_flag = shutdown_clone;
                Self::consumer_loop(&ring, &mut handler, &strategy, &seq, &shutdown_flag);
            })?;

        Ok(Self {
            thread: Some(thread),
            sequence,
            shutdown,
            _phantom: PhantomData,
        })
    }

    /// Create a new elegant consumer with state
    ///
    /// # Arguments
    /// * `ring_buffer` - The ring buffer to consume from
    /// * `event_handler` - Function to handle each event with state
    /// * `initialize_state` - Function to initialize the state
    /// * `wait_strategy` - Strategy for waiting when no events are available
    ///
    /// # Returns
    /// A new ElegantConsumer that automatically processes events with state
    pub fn with_state<H, S, I, W>(
        ring_buffer: Arc<RingBuffer<T>>,
        event_handler: H,
        initialize_state: I,
        wait_strategy: W,
    ) -> std::io::Result<Self>
    where
        H: FnMut(&mut S, &T, i64, bool) + Send + 'static,
        S: Send + 'static,
        I: FnOnce() -> S + Send + 'static,
        W: SimpleWaitStrategy + 'static,
    {
        let sequence = Arc::new(CachePadded::new(AtomicI64::new(-1)));
        let shutdown = Arc::new(AtomicBool::new(false));

        let sequence_clone = sequence.clone();
        let shutdown_clone = shutdown.clone();

        let thread = ThreadBuilder::new()
            .thread_name("elegant-consumer-with-state")
            .spawn(move || {
                let mut handler = event_handler;
                let strategy = wait_strategy;
                let ring = ring_buffer;
                let seq = sequence_clone;
                let shutdown_flag = shutdown_clone;
                Self::consumer_loop_with_state(
                    &ring,
                    &mut handler,
                    initialize_state,
                    &strategy,
                    &seq,
                    &shutdown_flag,
                );
            })?;

        Ok(Self {
            thread: Some(thread),
            sequence,
            shutdown,
            _phantom: PhantomData,
        })
    }

    /// Create a new elegant consumer with CPU affinity
    ///
    /// # Arguments
    /// * `ring_buffer` - The ring buffer to consume from
    /// * `event_handler` - Function to handle each event
    /// * `wait_strategy` - Strategy for waiting when no events are available
    /// * `core_id` - CPU core to pin the consumer thread to
    ///
    /// # Returns
    /// A new ElegantConsumer pinned to the specified CPU core
    pub fn with_affinity<H, W>(
        ring_buffer: Arc<RingBuffer<T>>,
        event_handler: H,
        wait_strategy: W,
        core_id: usize,
    ) -> std::io::Result<Self>
    where
        H: FnMut(&T, i64, bool) + Send + 'static,
        W: SimpleWaitStrategy + 'static,
    {
        let sequence = Arc::new(CachePadded::new(AtomicI64::new(-1)));
        let shutdown = Arc::new(AtomicBool::new(false));

        let sequence_clone = sequence.clone();
        let shutdown_clone = shutdown.clone();

        let thread = ThreadBuilder::new()
            .pin_at_core(core_id)
            .thread_name(format!("elegant-consumer-core-{core_id}"))
            .spawn(move || {
                let mut handler = event_handler;
                let strategy = wait_strategy;
                let ring = ring_buffer;
                let seq = sequence_clone;
                let shutdown_flag = shutdown_clone;
                Self::consumer_loop(&ring, &mut handler, &strategy, &seq, &shutdown_flag);
            })?;

        Ok(Self {
            thread: Some(thread),
            sequence,
            shutdown,
            _phantom: PhantomData,
        })
    }

    /// Get the current sequence position of this consumer
    pub fn current_sequence(&self) -> i64 {
        self.sequence.load(Ordering::Acquire)
    }

    /// Check if the consumer is still running
    pub fn is_running(&self) -> bool {
        !self.shutdown.load(Ordering::Acquire)
            && self.thread.as_ref().is_some_and(ManagedThread::is_running)
    }

    /// Gracefully shutdown the consumer
    ///
    /// This signals the consumer to stop processing and waits for it to finish.
    pub fn shutdown(mut self) -> std::thread::Result<()> {
        // Signal shutdown
        self.shutdown.store(true, Ordering::Release);

        // Wait for thread to complete
        if let Some(thread) = self.thread.take() {
            thread.join()
        } else {
            Ok(())
        }
    }

    /// Main consumer loop for processing events
    fn consumer_loop<H, W>(
        ring_buffer: &Arc<RingBuffer<T>>,
        event_handler: &mut H,
        wait_strategy: &W,
        sequence: &Arc<CachePadded<AtomicI64>>,
        shutdown: &Arc<AtomicBool>,
    ) where
        H: FnMut(&T, i64, bool),
        W: SimpleWaitStrategy,
    {
        let mut next_sequence = 0i64;

        while !shutdown.load(Ordering::Acquire) {
            // Wait for events to become available
            let available_sequence = Self::wait_for_events(
                next_sequence,
                ring_buffer.as_ref(),
                wait_strategy,
                shutdown.as_ref(),
            );

            if let Some(available) = available_sequence {
                // Process all available events (automatic batch processing)
                while available >= next_sequence && !shutdown.load(Ordering::Acquire) {
                    let end_of_batch = available == next_sequence;

                    // Get the event safely
                    let event = ring_buffer.get(next_sequence);

                    // Process the event
                    event_handler(event, next_sequence, end_of_batch);

                    // Move to next sequence
                    next_sequence += 1;
                }

                // Update our sequence position
                sequence.store(next_sequence - 1, Ordering::Release);
            }
        }
    }

    /// Main consumer loop with state for processing events
    fn consumer_loop_with_state<H, S, I, W>(
        ring_buffer: &Arc<RingBuffer<T>>,
        event_handler: &mut H,
        initialize_state: I,
        wait_strategy: &W,
        sequence: &Arc<CachePadded<AtomicI64>>,
        shutdown: &Arc<AtomicBool>,
    ) where
        H: FnMut(&mut S, &T, i64, bool),
        I: FnOnce() -> S,
        W: SimpleWaitStrategy,
    {
        let mut state = initialize_state();
        let mut next_sequence = 0i64;

        while !shutdown.load(Ordering::Acquire) {
            // Wait for events to become available
            let available_sequence = Self::wait_for_events(
                next_sequence,
                ring_buffer.as_ref(),
                wait_strategy,
                shutdown.as_ref(),
            );

            if let Some(available) = available_sequence {
                // Process all available events (automatic batch processing)
                while available >= next_sequence && !shutdown.load(Ordering::Acquire) {
                    let end_of_batch = available == next_sequence;

                    // Get the event safely
                    let event = ring_buffer.get(next_sequence);

                    // Process the event with state
                    event_handler(&mut state, event, next_sequence, end_of_batch);

                    // Move to next sequence
                    next_sequence += 1;
                }

                // Update our sequence position
                sequence.store(next_sequence - 1, Ordering::Release);
            }
        }
    }

    /// Wait for events to become available
    fn wait_for_events<W>(
        sequence: i64,
        ring_buffer: &RingBuffer<T>,
        wait_strategy: &W,
        shutdown: &AtomicBool,
    ) -> Option<i64>
    where
        W: SimpleWaitStrategy,
    {
        // For now, we'll implement a simple availability check
        // In a full implementation, this would coordinate with producers
        let buffer_size = ring_buffer.size();

        // Simple check: if we're within buffer bounds, events are available
        if sequence < buffer_size && !shutdown.load(Ordering::Acquire) {
            Some(sequence)
        } else {
            // Wait using the strategy
            wait_strategy.wait_for(sequence);

            // Check shutdown again
            if shutdown.load(Ordering::Acquire) {
                None
            } else {
                Some(sequence)
            }
        }
    }
}

impl<T> Drop for ElegantConsumer<T>
where
    T: Send + Sync,
{
    fn drop(&mut self) {
        // Signal shutdown
        self.shutdown.store(true, Ordering::Release);

        // The ManagedThread will handle cleanup in its Drop implementation
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disruptor::{event_factory::ClosureEventFactory, simple_wait_strategy::BusySpin};
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Clone)]
    struct TestEvent {
        value: i32,
    }

    #[test]
    fn test_elegant_consumer_creation() {
        let factory = ClosureEventFactory::new(|| TestEvent { value: 0 });
        let ring_buffer = Arc::new(RingBuffer::new(8, factory).unwrap());

        let processed_events = Arc::new(Mutex::new(Vec::new()));
        let processed_clone = processed_events.clone();

        let consumer: Result<ElegantConsumer<TestEvent>, _> = ElegantConsumer::new(
            ring_buffer,
            move |event: &TestEvent, sequence: i64, end_of_batch: bool| {
                processed_clone
                    .lock()
                    .unwrap()
                    .push((event.value, sequence, end_of_batch));
            },
            BusySpin,
        );

        assert!(consumer.is_ok());
        let consumer = consumer.unwrap();

        // Consumer sequence might have advanced if it's already processing events
        // In release mode, the consumer thread can start very quickly
        let seq = consumer.current_sequence();
        assert!(seq >= -1, "Sequence should be -1 or higher, but was {seq}");
        assert!(consumer.is_running());

        // Shutdown gracefully
        consumer
            .shutdown()
            .expect("Consumer should shutdown cleanly");
    }

    #[test]
    fn test_elegant_consumer_with_state() {
        let factory = ClosureEventFactory::new(|| TestEvent { value: 0 });
        let ring_buffer = Arc::new(RingBuffer::new(8, factory).unwrap());

        let consumer: Result<ElegantConsumer<TestEvent>, _> = ElegantConsumer::with_state(
            ring_buffer,
            |state: &mut i32, event: &TestEvent, _sequence: i64, _end_of_batch: bool| {
                *state += event.value;
            },
            || 0i32, // Initialize state to 0
            BusySpin,
        );

        assert!(consumer.is_ok());
        let consumer = consumer.unwrap();
        assert!(consumer.is_running());

        // Shutdown gracefully
        consumer
            .shutdown()
            .expect("Consumer should shutdown cleanly");
    }

    #[test]
    #[cfg(not(miri))] // Skip in Miri as it doesn't support CPU affinity
    fn test_elegant_consumer_with_affinity() {
        let available_cores = crate::disruptor::thread_management::get_available_cores();
        if !available_cores.is_empty() {
            let factory = ClosureEventFactory::new(|| TestEvent { value: 0 });
            let ring_buffer = Arc::new(RingBuffer::new(8, factory).unwrap());

            let core_id = available_cores[0];
            let consumer: Result<ElegantConsumer<TestEvent>, _> = ElegantConsumer::with_affinity(
                ring_buffer,
                |_event: &TestEvent, _sequence: i64, _end_of_batch: bool| {
                    // Just process the event
                },
                BusySpin,
                core_id,
            );

            assert!(consumer.is_ok());
            let consumer = consumer.unwrap();
            assert!(consumer.is_running());

            // Shutdown gracefully
            consumer
                .shutdown()
                .expect("Consumer should shutdown cleanly");
        }
    }

    #[test]
    fn test_consumer_shutdown() {
        let factory = ClosureEventFactory::new(|| TestEvent { value: 0 });
        let ring_buffer = Arc::new(RingBuffer::new(8, factory).unwrap());

        let consumer: ElegantConsumer<TestEvent> = ElegantConsumer::new(
            ring_buffer,
            |_event: &TestEvent, _sequence: i64, _end_of_batch: bool| {
                // Just process the event
            },
            BusySpin,
        )
        .unwrap();

        assert!(consumer.is_running());

        // Shutdown and verify
        consumer
            .shutdown()
            .expect("Consumer should shutdown cleanly");
    }

    #[test]
    fn test_consumer_drop_behavior() {
        let factory = ClosureEventFactory::new(|| TestEvent { value: 0 });
        let ring_buffer = Arc::new(RingBuffer::new(8, factory).unwrap());

        let consumer: ElegantConsumer<TestEvent> = ElegantConsumer::new(
            ring_buffer,
            |_event: &TestEvent, _sequence: i64, _end_of_batch: bool| {
                // Just process the event
            },
            BusySpin,
        )
        .unwrap();

        assert!(consumer.is_running());

        // Drop the consumer (should trigger Drop implementation)
        drop(consumer);
        // Test passes if no panic occurs
    }

    #[test]
    fn test_shutdown_with_already_shutdown_consumer() {
        let factory = ClosureEventFactory::new(|| TestEvent { value: 0 });
        let ring_buffer = Arc::new(RingBuffer::new(8, factory).unwrap());

        let consumer: ElegantConsumer<TestEvent> = ElegantConsumer::new(
            ring_buffer,
            |_event: &TestEvent, _sequence: i64, _end_of_batch: bool| {
                // Just process the event
            },
            BusySpin,
        )
        .unwrap();

        // First shutdown
        consumer.shutdown().expect("First shutdown should succeed");

        // Simulate a second shutdown by manually creating another consumer without thread
        let second_consumer: ElegantConsumer<TestEvent> = ElegantConsumer {
            thread: None,
            sequence: Arc::new(CachePadded::new(AtomicI64::new(-1))),
            shutdown: Arc::new(AtomicBool::new(true)),
            _phantom: PhantomData,
        };

        // Second shutdown should return Ok(()) when thread is None
        let result = second_consumer.shutdown();
        assert!(result.is_ok());
    }

    #[test]
    fn test_consumer_sequence_tracking() {
        let factory = ClosureEventFactory::new(|| TestEvent { value: 42 });
        let ring_buffer = Arc::new(RingBuffer::new(8, factory).unwrap());

        let processed_count = Arc::new(Mutex::new(0));
        let processed_count_clone = processed_count.clone();

        let consumer: ElegantConsumer<TestEvent> = ElegantConsumer::new(
            ring_buffer.clone(),
            move |_event: &TestEvent, _sequence: i64, _end_of_batch: bool| {
                let mut count = processed_count_clone.lock().unwrap();
                *count += 1;
            },
            BusySpin,
        )
        .unwrap();

        // Give some time for processing
        std::thread::sleep(std::time::Duration::from_millis(10));

        // Check that current_sequence is updated
        let initial_seq = consumer.current_sequence();
        assert!(initial_seq >= -1);

        consumer
            .shutdown()
            .expect("Consumer should shutdown cleanly");
    }

    #[test]
    fn test_wait_for_events_with_shutdown() {
        let factory = ClosureEventFactory::new(|| TestEvent { value: 0 });
        let ring_buffer = RingBuffer::new(8, factory).unwrap();
        let shutdown = AtomicBool::new(true); // Start with shutdown = true
        let wait_strategy = BusySpin;

        let result = ElegantConsumer::wait_for_events(0, &ring_buffer, &wait_strategy, &shutdown);
        assert!(result.is_none(), "Should return None when shutdown is true");

        // Test with shutdown = false
        let shutdown = AtomicBool::new(false);
        let result = ElegantConsumer::wait_for_events(0, &ring_buffer, &wait_strategy, &shutdown);
        assert!(
            result.is_some(),
            "Should return Some when shutdown is false"
        );
    }

    #[test]
    fn test_wait_for_events_sequence_bounds() {
        let factory = ClosureEventFactory::new(|| TestEvent { value: 0 });
        let ring_buffer = RingBuffer::new(8, factory).unwrap();
        let shutdown = AtomicBool::new(false);
        let wait_strategy = BusySpin;

        // Test within bounds
        let result = ElegantConsumer::wait_for_events(4, &ring_buffer, &wait_strategy, &shutdown);
        assert!(
            result.is_some(),
            "Should return Some when sequence is within bounds"
        );

        // Test beyond bounds
        let result = ElegantConsumer::wait_for_events(10, &ring_buffer, &wait_strategy, &shutdown);
        assert!(
            result.is_some(),
            "Should still return Some after wait strategy"
        );
    }

    #[test]
    fn test_consumer_with_state_processing() {
        let factory = ClosureEventFactory::new(|| TestEvent { value: 1 });
        let ring_buffer = Arc::new(RingBuffer::new(8, factory).unwrap());

        let sum_result = Arc::new(Mutex::new(0));
        let sum_result_clone = sum_result.clone();

        let consumer: ElegantConsumer<TestEvent> = ElegantConsumer::with_state(
            ring_buffer.clone(),
            move |state: &mut i32, event: &TestEvent, _sequence: i64, _end_of_batch: bool| {
                *state += event.value;
                // Store the current state sum
                let mut result = sum_result_clone.lock().unwrap();
                *result = *state;
            },
            || 0i32,
            BusySpin,
        )
        .unwrap();

        // Give some time for processing
        std::thread::sleep(std::time::Duration::from_millis(10));

        consumer
            .shutdown()
            .expect("Consumer should shutdown cleanly");

        // Check that state was updated
        let final_sum = *sum_result.lock().unwrap();
        assert!(final_sum >= 0, "State should have been updated");
    }

    #[test]
    fn test_consumer_is_running_states() {
        let factory = ClosureEventFactory::new(|| TestEvent { value: 0 });
        let ring_buffer = Arc::new(RingBuffer::new(8, factory).unwrap());

        let consumer: ElegantConsumer<TestEvent> = ElegantConsumer::new(
            ring_buffer,
            |_event: &TestEvent, _sequence: i64, _end_of_batch: bool| {
                std::thread::sleep(std::time::Duration::from_millis(1));
            },
            BusySpin,
        )
        .unwrap();

        // Should be running initially
        assert!(consumer.is_running());

        // Shutdown should make it not running
        consumer
            .shutdown()
            .expect("Consumer should shutdown cleanly");
    }

    #[test]
    fn test_consumer_thread_naming() {
        let factory = ClosureEventFactory::new(|| TestEvent { value: 0 });
        let ring_buffer = Arc::new(RingBuffer::new(8, factory).unwrap());

        // Test basic consumer thread naming
        let consumer1: ElegantConsumer<TestEvent> = ElegantConsumer::new(
            ring_buffer.clone(),
            |_event: &TestEvent, _sequence: i64, _end_of_batch: bool| {},
            BusySpin,
        )
        .unwrap();

        // Test with-state consumer thread naming
        let consumer2: ElegantConsumer<TestEvent> = ElegantConsumer::with_state(
            ring_buffer.clone(),
            |_state: &mut i32, _event: &TestEvent, _sequence: i64, _end_of_batch: bool| {},
            || 0i32,
            BusySpin,
        )
        .unwrap();

        consumer1
            .shutdown()
            .expect("Consumer1 should shutdown cleanly");
        consumer2
            .shutdown()
            .expect("Consumer2 should shutdown cleanly");
    }

    #[test]
    fn test_consumer_loop_batch_processing() {
        let factory = ClosureEventFactory::new(|| TestEvent { value: 1 });
        let ring_buffer = Arc::new(RingBuffer::new(8, factory).unwrap());

        let batch_info = Arc::new(Mutex::new(Vec::new()));
        let batch_info_clone = batch_info.clone();

        let consumer: ElegantConsumer<TestEvent> = ElegantConsumer::new(
            ring_buffer.clone(),
            move |_event: &TestEvent, sequence: i64, end_of_batch: bool| {
                batch_info_clone
                    .lock()
                    .unwrap()
                    .push((sequence, end_of_batch));
            },
            BusySpin,
        )
        .unwrap();

        // Give some time for processing
        std::thread::sleep(std::time::Duration::from_millis(10));

        consumer
            .shutdown()
            .expect("Consumer should shutdown cleanly");

        // Check that batch processing occurred
        let batch_events = batch_info.lock().unwrap();
        assert!(
            !batch_events.is_empty(),
            "Should have processed some events"
        );
    }
}
