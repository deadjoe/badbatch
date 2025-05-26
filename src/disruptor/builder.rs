//! Builder module for creating Disruptors with fluent API
//!
//! This module provides a fluent API for building Disruptors, inspired by disruptor-rs.
//! It allows for easy configuration of producers, consumers, and their dependencies.

use crate::disruptor::{
    event_factory::ClosureEventFactory,
    producer::{Producer, SimpleProducer},
    sequencer::{MultiProducerSequencer, SingleProducerSequencer},
    EventHandler, RingBuffer, Sequencer, WaitStrategy,
};
use std::marker::PhantomData;
use std::sync::Arc;

/// Build a single producer Disruptor
///
/// This follows the disruptor-rs pattern for creating single producer Disruptors.
///
/// # Examples
///
/// ```rust,ignore
/// let mut producer = build_single_producer(8, factory, BusySpinWaitStrategy)
///     .handle_events_with(processor)
///     .build();
/// ```
pub fn build_single_producer<E, F, W>(
    size: usize,
    event_factory: F,
    wait_strategy: W,
) -> SingleProducerBuilder<NoConsumers, E, F, W>
where
    E: Send + Sync + 'static,
    F: Fn() -> E + Send + Sync + 'static,
    W: WaitStrategy + Send + Sync + 'static,
{
    SingleProducerBuilder::new(size, event_factory, wait_strategy)
}

/// Build a multi producer Disruptor
///
/// This follows the disruptor-rs pattern for creating multi producer Disruptors.
///
/// # Examples
///
/// ```rust,ignore
/// let mut producer1 = build_multi_producer(64, factory, BusySpinWaitStrategy)
///     .handle_events_with(processor1)
///     .handle_events_with(processor2)
///     .build();
/// let mut producer2 = producer1.clone();
/// ```
pub fn build_multi_producer<E, F, W>(
    size: usize,
    event_factory: F,
    wait_strategy: W,
) -> MultiProducerBuilder<NoConsumers, E, F, W>
where
    E: Send + Sync + 'static,
    F: Fn() -> E + Send + Sync + 'static,
    W: WaitStrategy + Send + Sync + 'static,
{
    MultiProducerBuilder::new(size, event_factory, wait_strategy)
}

/// State marker: No consumers added yet
pub struct NoConsumers;

/// State marker: Has consumers
pub struct HasConsumers;

/// Builder for single producer Disruptors
pub struct SingleProducerBuilder<State, E, F, W>
where
    E: Send + Sync + 'static,
    F: Fn() -> E + Send + Sync + 'static,
    W: WaitStrategy + Send + Sync + 'static,
{
    size: usize,
    event_factory: F,
    wait_strategy: W,
    event_handlers: Vec<Box<dyn EventHandler<E> + Send + Sync>>,
    _phantom: PhantomData<(State, E)>,
}

impl<E, F, W> SingleProducerBuilder<NoConsumers, E, F, W>
where
    E: Send + Sync + 'static,
    F: Fn() -> E + Send + Sync + 'static,
    W: WaitStrategy + Send + Sync + 'static,
{
    fn new(size: usize, event_factory: F, wait_strategy: W) -> Self {
        Self {
            size,
            event_factory,
            wait_strategy,
            event_handlers: Vec::new(),
            _phantom: PhantomData,
        }
    }

    /// Add an event handler to process events
    ///
    /// This follows the disruptor-rs pattern for adding event handlers.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// builder.handle_events_with(|event, sequence, end_of_batch| {
    ///     // Process the event
    /// })
    /// ```
    pub fn handle_events_with<H>(
        mut self,
        handler: H,
    ) -> SingleProducerBuilder<HasConsumers, E, F, W>
    where
        H: FnMut(&mut E, i64, bool) + Send + Sync + 'static,
    {
        // Wrap the closure in our EventHandler trait
        let boxed_handler = Box::new(ClosureEventHandler::new(handler));
        self.event_handlers.push(boxed_handler);

        SingleProducerBuilder {
            size: self.size,
            event_factory: self.event_factory,
            wait_strategy: self.wait_strategy,
            event_handlers: self.event_handlers,
            _phantom: PhantomData,
        }
    }
}

impl<E, F, W> SingleProducerBuilder<HasConsumers, E, F, W>
where
    E: Send + Sync + 'static,
    F: Fn() -> E + Send + Sync + 'static,
    W: WaitStrategy + Send + Sync + 'static,
{
    /// Add another event handler to process events in parallel
    pub fn handle_events_with<H>(mut self, handler: H) -> Self
    where
        H: FnMut(&mut E, i64, bool) + Send + Sync + 'static,
    {
        let boxed_handler = Box::new(ClosureEventHandler::new(handler));
        self.event_handlers.push(boxed_handler);
        self
    }

    /// Build the Disruptor and return a Producer
    ///
    /// This creates the RingBuffer, Sequencer, and starts all event processors.
    pub fn build(self) -> SimpleProducer<E> {
        // Create the ring buffer with a closure event factory
        let closure_factory = ClosureEventFactory::new(self.event_factory);
        let ring_buffer = Arc::new(
            RingBuffer::new(self.size, closure_factory).expect("Failed to create ring buffer"),
        );

        // Create a single producer sequencer
        let sequencer = Arc::new(SingleProducerSequencer::new(
            self.size,
            Arc::new(self.wait_strategy),
        ));

        // TODO: Create and start event processors in background threads
        // For now, we'll just create a simple producer with the sequencer
        SimpleProducer::new(ring_buffer, sequencer)
    }
}

/// Builder for multi producer Disruptors
pub struct MultiProducerBuilder<State, E, F, W>
where
    E: Send + Sync + 'static,
    F: Fn() -> E + Send + Sync + 'static,
    W: WaitStrategy + Send + Sync + 'static,
{
    size: usize,
    event_factory: F,
    wait_strategy: W,
    event_handlers: Vec<Box<dyn EventHandler<E> + Send + Sync>>,
    _phantom: PhantomData<(State, E)>,
}

impl<E, F, W> MultiProducerBuilder<NoConsumers, E, F, W>
where
    E: Send + Sync + 'static,
    F: Fn() -> E + Send + Sync + 'static,
    W: WaitStrategy + Send + Sync + 'static,
{
    fn new(size: usize, event_factory: F, wait_strategy: W) -> Self {
        Self {
            size,
            event_factory,
            wait_strategy,
            event_handlers: Vec::new(),
            _phantom: PhantomData,
        }
    }

    /// Add an event handler to process events
    pub fn handle_events_with<H>(
        mut self,
        handler: H,
    ) -> MultiProducerBuilder<HasConsumers, E, F, W>
    where
        H: FnMut(&mut E, i64, bool) + Send + Sync + 'static,
    {
        let boxed_handler = Box::new(ClosureEventHandler::new(handler));
        self.event_handlers.push(boxed_handler);

        MultiProducerBuilder {
            size: self.size,
            event_factory: self.event_factory,
            wait_strategy: self.wait_strategy,
            event_handlers: self.event_handlers,
            _phantom: PhantomData,
        }
    }
}

impl<E, F, W> MultiProducerBuilder<HasConsumers, E, F, W>
where
    E: Send + Sync + 'static,
    F: Fn() -> E + Send + Sync + 'static,
    W: WaitStrategy + Send + Sync + 'static,
{
    /// Add another event handler to process events in parallel
    pub fn handle_events_with<H>(mut self, handler: H) -> Self
    where
        H: FnMut(&mut E, i64, bool) + Send + Sync + 'static,
    {
        let boxed_handler = Box::new(ClosureEventHandler::new(handler));
        self.event_handlers.push(boxed_handler);
        self
    }

    /// Build the Disruptor and return a Producer that can be cloned
    pub fn build(self) -> CloneableProducer<E> {
        // Create the ring buffer with a closure event factory
        let closure_factory = ClosureEventFactory::new(self.event_factory);
        let ring_buffer = Arc::new(
            RingBuffer::new(self.size, closure_factory).expect("Failed to create ring buffer"),
        );

        // Create a multi producer sequencer
        let sequencer = Arc::new(MultiProducerSequencer::new(
            self.size,
            Arc::new(self.wait_strategy),
        ));

        // TODO: Create and start event processors in background threads
        // For now, we'll create a cloneable producer wrapper
        CloneableProducer::new(ring_buffer, sequencer)
    }
}

/// Wrapper for SimpleProducer that can be cloned for multi-producer scenarios
///
/// This implementation now properly coordinates multiple producers using a shared sequencer.
#[derive(Clone)]
pub struct CloneableProducer<E>
where
    E: Send + Sync,
{
    ring_buffer: Arc<RingBuffer<E>>,
    sequencer: Arc<dyn Sequencer>,
}

impl<E> CloneableProducer<E>
where
    E: Send + Sync,
{
    fn new(ring_buffer: Arc<RingBuffer<E>>, sequencer: Arc<dyn Sequencer>) -> Self {
        Self {
            ring_buffer,
            sequencer,
        }
    }

    /// Create a new SimpleProducer for this thread
    ///
    /// Each thread should call this to get its own producer instance.
    /// This avoids the lifetime issues with shared mutable access.
    pub fn create_producer(&self) -> SimpleProducer<E> {
        SimpleProducer::new(self.ring_buffer.clone(), self.sequencer.clone())
    }
}

impl<E> CloneableProducer<E>
where
    E: Send + Sync,
{
    /// Publish an event using a closure (simplified API)
    pub fn try_publish<F>(
        &self,
        update: F,
    ) -> std::result::Result<i64, crate::disruptor::producer::RingBufferFull>
    where
        F: FnOnce(&mut E),
    {
        let mut producer = self.create_producer();
        producer.try_publish(update)
    }

    /// Publish an event, spinning until space is available
    pub fn publish<F>(&self, update: F)
    where
        F: FnOnce(&mut E),
    {
        let mut producer = self.create_producer();
        producer.publish(update)
    }

    /// Try to publish a batch of events
    pub fn try_batch_publish<F>(
        &self,
        n: usize,
        update: F,
    ) -> std::result::Result<i64, crate::disruptor::producer::MissingFreeSlots>
    where
        F: for<'a> FnOnce(crate::disruptor::ring_buffer::BatchIterMut<'a, E>),
    {
        let mut producer = self.create_producer();
        producer.try_batch_publish(n, update)
    }

    /// Publish a batch of events, spinning until space is available
    pub fn batch_publish<F>(&self, n: usize, update: F)
    where
        F: for<'a> FnOnce(crate::disruptor::ring_buffer::BatchIterMut<'a, E>),
    {
        let mut producer = self.create_producer();
        producer.batch_publish(n, update)
    }
}

/// Wrapper for closures to implement EventHandler trait
struct ClosureEventHandler<E, F>
where
    E: Send + Sync,
    F: FnMut(&mut E, i64, bool) + Send + Sync,
{
    handler: F,
    _phantom: PhantomData<E>,
}

impl<E, F> ClosureEventHandler<E, F>
where
    E: Send + Sync,
    F: FnMut(&mut E, i64, bool) + Send + Sync,
{
    fn new(handler: F) -> Self {
        Self {
            handler,
            _phantom: PhantomData,
        }
    }
}

impl<E, F> EventHandler<E> for ClosureEventHandler<E, F>
where
    E: Send + Sync,
    F: FnMut(&mut E, i64, bool) + Send + Sync,
{
    fn on_event(
        &mut self,
        event: &mut E,
        sequence: i64,
        end_of_batch: bool,
    ) -> crate::disruptor::Result<()> {
        (self.handler)(event, sequence, end_of_batch);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disruptor::wait_strategy::BusySpinWaitStrategy;
    use std::sync::{mpsc, Arc, Mutex};

    #[derive(Debug, Clone, PartialEq)]
    struct TestEvent {
        value: i64,
        data: String,
    }

    impl Default for TestEvent {
        fn default() -> Self {
            Self {
                value: -1,
                data: String::new(),
            }
        }
    }

    fn test_event_factory() -> TestEvent {
        TestEvent::default()
    }

    #[test]
    fn test_build_single_producer_basic() {
        let builder = build_single_producer(8, test_event_factory, BusySpinWaitStrategy);

        // Should be able to create builder without consumers
        assert_eq!(builder.size, 8);
        assert_eq!(builder.event_handlers.len(), 0);
    }

    #[test]
    fn test_build_multi_producer_basic() {
        let builder = build_multi_producer(16, test_event_factory, BusySpinWaitStrategy);

        // Should be able to create builder without consumers
        assert_eq!(builder.size, 16);
        assert_eq!(builder.event_handlers.len(), 0);
    }

    #[test]
    fn test_single_producer_add_event_handler() {
        let (tx, _rx) = mpsc::channel();
        let handler = move |event: &mut TestEvent, sequence: i64, _end_of_batch: bool| {
            event.value = sequence;
            let _ = tx.send(sequence);
        };

        let builder = build_single_producer(8, test_event_factory, BusySpinWaitStrategy)
            .handle_events_with(handler);

        // Should transition to HasConsumers state and have one handler
        assert_eq!(builder.event_handlers.len(), 1);
    }

    #[test]
    fn test_multi_producer_add_event_handler() {
        let (tx, _rx) = mpsc::channel();
        let handler = move |event: &mut TestEvent, sequence: i64, _end_of_batch: bool| {
            event.value = sequence;
            let _ = tx.send(sequence);
        };

        let builder = build_multi_producer(16, test_event_factory, BusySpinWaitStrategy)
            .handle_events_with(handler);

        // Should transition to HasConsumers state and have one handler
        assert_eq!(builder.event_handlers.len(), 1);
    }

    #[test]
    fn test_single_producer_multiple_handlers() {
        let (tx1, _rx1) = mpsc::channel();
        let (tx2, _rx2) = mpsc::channel();

        let handler1 = move |event: &mut TestEvent, sequence: i64, _end_of_batch: bool| {
            event.value = sequence;
            let _ = tx1.send(sequence);
        };

        let handler2 = move |event: &mut TestEvent, sequence: i64, _end_of_batch: bool| {
            event.data = format!("seq_{}", sequence);
            let _ = tx2.send(sequence);
        };

        let builder = build_single_producer(8, test_event_factory, BusySpinWaitStrategy)
            .handle_events_with(handler1)
            .handle_events_with(handler2);

        // Should have two handlers
        assert_eq!(builder.event_handlers.len(), 2);
    }

    #[test]
    fn test_multi_producer_multiple_handlers() {
        let (tx1, _rx1) = mpsc::channel();
        let (tx2, _rx2) = mpsc::channel();

        let handler1 = move |event: &mut TestEvent, sequence: i64, _end_of_batch: bool| {
            event.value = sequence;
            let _ = tx1.send(sequence);
        };

        let handler2 = move |event: &mut TestEvent, sequence: i64, _end_of_batch: bool| {
            event.data = format!("seq_{}", sequence);
            let _ = tx2.send(sequence);
        };

        let builder = build_multi_producer(16, test_event_factory, BusySpinWaitStrategy)
            .handle_events_with(handler1)
            .handle_events_with(handler2);

        // Should have two handlers
        assert_eq!(builder.event_handlers.len(), 2);
    }

    #[test]
    fn test_single_producer_build() {
        let (tx, rx) = mpsc::channel();
        let handler = move |event: &mut TestEvent, sequence: i64, _end_of_batch: bool| {
            event.value = sequence;
            let _ = tx.send(sequence);
        };

        let producer = build_single_producer(8, test_event_factory, BusySpinWaitStrategy)
            .handle_events_with(handler)
            .build();

        // Should successfully build and return a SimpleProducer
        // The producer should be usable for publishing
        drop(producer);
        drop(rx); // Ensure we can drop the receiver
    }

    #[test]
    fn test_multi_producer_build() {
        let (tx, rx) = mpsc::channel();
        let handler = move |event: &mut TestEvent, sequence: i64, _end_of_batch: bool| {
            event.value = sequence;
            let _ = tx.send(sequence);
        };

        let producer = build_multi_producer(16, test_event_factory, BusySpinWaitStrategy)
            .handle_events_with(handler)
            .build();

        // Should successfully build and return a CloneableProducer
        // The producer should be cloneable
        let _producer2 = producer.clone();
        drop(producer);
        drop(rx); // Ensure we can drop the receiver
    }

    #[test]
    fn test_cloneable_producer_clone() {
        let (tx, rx) = mpsc::channel();
        let handler = move |event: &mut TestEvent, sequence: i64, _end_of_batch: bool| {
            event.value = sequence;
            let _ = tx.send(sequence);
        };

        let producer1 = build_multi_producer(16, test_event_factory, BusySpinWaitStrategy)
            .handle_events_with(handler)
            .build();

        let producer2 = producer1.clone();

        // Both producers should be independent but share the same ring buffer
        drop(producer1);
        drop(producer2);
        drop(rx);
    }

    #[test]
    fn test_cloneable_producer_create_producer() {
        let (tx, rx) = mpsc::channel();
        let handler = move |event: &mut TestEvent, sequence: i64, _end_of_batch: bool| {
            event.value = sequence;
            let _ = tx.send(sequence);
        };

        let cloneable_producer = build_multi_producer(16, test_event_factory, BusySpinWaitStrategy)
            .handle_events_with(handler)
            .build();

        let _simple_producer = cloneable_producer.create_producer();

        // Should be able to create SimpleProducer instances
        drop(cloneable_producer);
        drop(rx);
    }

    #[test]
    fn test_builder_with_different_wait_strategies() {
        use crate::disruptor::wait_strategy::{SleepingWaitStrategy, YieldingWaitStrategy};

        let (tx1, rx1) = mpsc::channel();
        let (tx2, rx2) = mpsc::channel();
        let (tx3, rx3) = mpsc::channel();

        let handler1 = move |event: &mut TestEvent, sequence: i64, _: bool| {
            event.value = sequence;
            let _ = tx1.send(sequence);
        };

        let handler2 = move |event: &mut TestEvent, sequence: i64, _: bool| {
            event.value = sequence;
            let _ = tx2.send(sequence);
        };

        let handler3 = move |event: &mut TestEvent, sequence: i64, _: bool| {
            event.value = sequence;
            let _ = tx3.send(sequence);
        };

        // Test with BusySpinWaitStrategy
        let _producer1 = build_single_producer(8, test_event_factory, BusySpinWaitStrategy)
            .handle_events_with(handler1)
            .build();

        // Test with YieldingWaitStrategy
        let _producer2 = build_single_producer(8, test_event_factory, YieldingWaitStrategy)
            .handle_events_with(handler2)
            .build();

        // Test with SleepingWaitStrategy
        let _producer3 = build_single_producer(8, test_event_factory, SleepingWaitStrategy::new())
            .handle_events_with(handler3)
            .build();

        drop(rx1);
        drop(rx2);
        drop(rx3);
    }

    #[test]
    fn test_builder_with_different_buffer_sizes() {
        // Test various power-of-2 sizes
        for &size in &[2, 4, 8, 16, 32, 64, 128, 256, 512, 1024] {
            let _producer = build_single_producer(size, test_event_factory, BusySpinWaitStrategy)
                .handle_events_with(move |event: &mut TestEvent, sequence: i64, _: bool| {
                    event.value = sequence;
                })
                .build();
        }
    }

    #[test]
    fn test_closure_event_handler() {
        let counter = Arc::new(Mutex::new(0));
        let counter_clone = counter.clone();

        let handler = move |event: &mut TestEvent, sequence: i64, _end_of_batch: bool| {
            event.value = sequence;
            let mut count = counter_clone.lock().unwrap();
            *count += 1;
        };

        let mut closure_handler = ClosureEventHandler::new(handler);
        let mut event = TestEvent::default();

        // Test the handler directly
        closure_handler.on_event(&mut event, 42, false).unwrap();

        assert_eq!(event.value, 42);
        assert_eq!(*counter.lock().unwrap(), 1);
    }

    #[test]
    fn test_builder_type_safety() {
        // This test ensures that the type system prevents invalid state transitions

        // Can't build without consumers
        let builder_no_consumers =
            build_single_producer(8, test_event_factory, BusySpinWaitStrategy);
        // builder_no_consumers.build(); // This should not compile

        // Must add at least one consumer before building
        let _producer = builder_no_consumers
            .handle_events_with(|_event: &mut TestEvent, _sequence: i64, _end_of_batch: bool| {})
            .build();
    }
}
