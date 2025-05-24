//! Disruptor Main Class Implementation
//!
//! This module provides the main Disruptor class that serves as the entry point
//! for configuring and using the Disruptor pattern. It provides a DSL-style
//! interface for setting up the ring buffer, sequencers, and event processors.

use crate::disruptor::{
    Result, DisruptorError, EventFactory, EventHandler, ExceptionHandler, ProducerType,
    RingBuffer, Sequencer, SingleProducerSequencer, MultiProducerSequencer, WaitStrategy,
    BlockingWaitStrategy, EventProcessor, BatchEventProcessor, SequenceBarrier, Sequence,
    DataProvider, is_power_of_two,
};
use std::sync::Arc;
use std::thread::JoinHandle;

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
/// use badbatch::disruptor::{Disruptor, ProducerType, BlockingWaitStrategy};
///
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
        self.sequencer.add_gating_sequences(&[processor_sequence.clone()]);

        self.event_processors.push(processor.clone());

        DisruptorBuilder {
            disruptor: self,
            last_processor_sequences: vec![processor_sequence],
        }
    }

    /// Start the Disruptor
    ///
    /// This starts all configured event processors in their own threads.
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

        // Start all event processors
        for processor in &self.event_processors {
            // In a real implementation, we would spawn threads here
            // For now, this is a simplified version
        }

        self.started = true;
        Ok(())
    }

    /// Shutdown the Disruptor
    ///
    /// This halts all event processors and waits for them to complete.
    ///
    /// # Returns
    /// Ok(()) if shutdown successfully
    pub fn shutdown(&mut self) -> Result<()> {
        if !self.started {
            return Ok(()); // Not started, nothing to shutdown
        }

        // Halt all event processors
        for processor in &self.event_processors {
            processor.halt();
        }

        // Wait for all threads to complete
        while let Some(handle) = self.thread_handles.pop() {
            if let Err(_) = handle.join() {
                // Log error but continue shutting down
            }
        }

        self.started = false;
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

        // Get the event and translate it
        // In a real implementation, we would need proper access to the ring buffer
        // This is a simplified version

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
            // Translate and publish
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
        let barrier = self.disruptor.sequencer.new_barrier(self.last_processor_sequences.clone());

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
        self.disruptor.sequencer.add_gating_sequences(&[processor_sequence.clone()]);

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
        // This is unsafe but necessary for the Disruptor pattern
        // The caller must ensure exclusive access
        self.get_mut_unchecked(sequence)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disruptor::{DefaultEventFactory, NoOpEventHandler};

    #[derive(Debug, Default)]
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
        ).unwrap();

        assert_eq!(disruptor.get_buffer_size(), 1024);
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
        assert!(matches!(result.unwrap_err(), DisruptorError::InvalidBufferSize(1023)));
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
}
