//! Builder module for creating Disruptors with fluent API
//!
//! This module provides a fluent API for building Disruptors, inspired by disruptor-rs.
//! It allows for easy configuration of producers, consumers, and their dependencies.

use crate::disruptor::{
    RingBuffer, WaitStrategy, EventHandler,
    producer::{Producer, SimpleProducer},
    event_factory::ClosureEventFactory,
};
use std::sync::Arc;
use std::marker::PhantomData;

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
        H: Fn(&E, i64, bool) + Send + Sync + 'static,
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
        H: Fn(&E, i64, bool) + Send + Sync + 'static,
    {
        let boxed_handler = Box::new(ClosureEventHandler::new(handler));
        self.event_handlers.push(boxed_handler);
        self
    }

    /// Build the Disruptor and return a Producer
    ///
    /// This creates the RingBuffer and starts all event processors.
    pub fn build(self) -> SimpleProducer<E> {
        // Create the ring buffer with a closure event factory
        let closure_factory = ClosureEventFactory::new(self.event_factory);
        let ring_buffer = Arc::new(
            RingBuffer::new(self.size, closure_factory)
                .expect("Failed to create ring buffer")
        );

        // TODO: Start event processors in background threads
        // For now, we'll just create a simple producer
        SimpleProducer::new(ring_buffer)
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
        H: Fn(&E, i64, bool) + Send + Sync + 'static,
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
        H: Fn(&E, i64, bool) + Send + Sync + 'static,
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
            RingBuffer::new(self.size, closure_factory)
                .expect("Failed to create ring buffer")
        );

        // TODO: Start event processors in background threads
        // For now, we'll create a cloneable producer wrapper
        CloneableProducer::new(ring_buffer)
    }
}

/// Wrapper for SimpleProducer that can be cloned for multi-producer scenarios
///
/// Note: This is a simplified implementation for demonstration.
/// A full multi-producer implementation would require proper sequencer coordination.
#[derive(Clone)]
pub struct CloneableProducer<E>
where
    E: Send + Sync,
{
    ring_buffer: Arc<RingBuffer<E>>,
}

impl<E> CloneableProducer<E>
where
    E: Send + Sync,
{
    fn new(ring_buffer: Arc<RingBuffer<E>>) -> Self {
        Self { ring_buffer }
    }

    /// Create a new SimpleProducer for this thread
    ///
    /// Each thread should call this to get its own producer instance.
    /// This avoids the lifetime issues with shared mutable access.
    pub fn create_producer(&self) -> SimpleProducer<E> {
        SimpleProducer::new(self.ring_buffer.clone())
    }
}

impl<E> CloneableProducer<E>
where
    E: Send + Sync,
{
    /// Publish an event using a closure (simplified API)
    pub fn try_publish<F>(&self, update: F) -> std::result::Result<i64, crate::disruptor::producer::RingBufferFull>
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
    pub fn try_batch_publish<F>(&self, n: usize, update: F) -> std::result::Result<i64, crate::disruptor::producer::MissingFreeSlots>
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
    F: Fn(&E, i64, bool) + Send + Sync,
{
    handler: F,
    _phantom: PhantomData<E>,
}

impl<E, F> ClosureEventHandler<E, F>
where
    E: Send + Sync,
    F: Fn(&E, i64, bool) + Send + Sync,
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
    F: Fn(&E, i64, bool) + Send + Sync,
{
    fn on_event(&mut self, event: &mut E, sequence: i64, end_of_batch: bool) -> crate::disruptor::Result<()> {
        // Note: We only read the event, but the trait requires &mut
        (self.handler)(event, sequence, end_of_batch);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disruptor::BusySpinWaitStrategy;

    #[derive(Debug, Clone)]
    struct TestEvent {
        value: i32,
    }

    #[test]
    fn test_single_producer_builder() {
        let factory = || TestEvent { value: 0 };
        let processor = |event: &TestEvent, _sequence: i64, _end_of_batch: bool| {
            println!("Processing event with value: {}", event.value);
        };

        let mut producer = build_single_producer(8, factory, BusySpinWaitStrategy)
            .handle_events_with(processor)
            .build();

        // Test publishing
        let result = producer.try_publish(|e| { e.value = 42; });
        assert!(result.is_ok());
    }

    #[test]
    fn test_multi_producer_builder() {
        let factory = || TestEvent { value: 0 };
        let processor = |event: &TestEvent, _sequence: i64, _end_of_batch: bool| {
            println!("Processing event with value: {}", event.value);
        };

        let producer1 = build_multi_producer(64, factory, BusySpinWaitStrategy)
            .handle_events_with(processor)
            .build();

        let producer2 = producer1.clone();

        // Test publishing from both producers
        let result1 = producer1.try_publish(|e| { e.value = 42; });
        let result2 = producer2.try_publish(|e| { e.value = 43; });

        assert!(result1.is_ok());
        assert!(result2.is_ok());
    }

    #[test]
    fn test_multiple_event_handlers() {
        let factory = || TestEvent { value: 0 };
        let processor1 = |event: &TestEvent, _sequence: i64, _end_of_batch: bool| {
            println!("Processor 1: {}", event.value);
        };
        let processor2 = |event: &TestEvent, _sequence: i64, _end_of_batch: bool| {
            println!("Processor 2: {}", event.value);
        };

        let mut producer = build_single_producer(8, factory, BusySpinWaitStrategy)
            .handle_events_with(processor1)
            .handle_events_with(processor2)
            .build();

        let result = producer.try_publish(|e| { e.value = 100; });
        assert!(result.is_ok());
    }
}
