//! Runtime handle for a built disruptor (producer + consumer lifecycle).

use super::core::DisruptorCore;
use crate::disruptor::{
    producer::{Producer, SimpleProducer},
    WaitStrategy,
};

/// Handle for managing a Disruptor with running consumer threads
///
/// This handle provides access to the producer and manages the lifecycle
/// of consumer threads. When dropped, it will attempt to gracefully
/// shutdown all consumer threads.
#[derive(Debug)]
pub struct DisruptorHandle<E, W>
where
    E: Send + Sync + 'static,
    W: WaitStrategy + 'static,
{
    /// Core disruptor components
    pub(crate) core: DisruptorCore<E, W>,
    /// The producer for publishing events
    producer: SimpleProducer<E, W>,
    /// Indicates whether shutdown() has been invoked
    is_shutdown: bool,
}

impl<E, W> DisruptorHandle<E, W>
where
    E: Send + Sync + 'static,
    W: WaitStrategy + 'static,
{
    pub(crate) fn new(core: DisruptorCore<E, W>) -> Self {
        let producer = core.create_producer();
        Self {
            core,
            producer,
            is_shutdown: false,
        }
    }

    /// Creates a new producer that shares the same ring buffer and sequencer
    ///
    /// This is useful for multi-producer scenarios where you need multiple threads
    /// to publish events to the same ring buffer.
    pub fn create_producer(&self) -> SimpleProducer<E, W> {
        self.core.create_producer()
    }

    /// Get a reference to the producer
    pub fn producer(&mut self) -> &mut SimpleProducer<E, W> {
        &mut self.producer
    }

    /// Consume the handle and return the producer
    ///
    /// Note: This will shutdown all consumer threads before returning the producer
    pub fn into_producer(mut self) -> SimpleProducer<E, W> {
        // Ensure the system is stopped before handing out the producer.
        self.shutdown();

        // Move the producer out while leaving a placeholder behind so Drop can run normally.
        let placeholder = self.core.create_producer();
        std::mem::replace(&mut self.producer, placeholder)
    }

    /// Publish an event using a closure (delegated to producer)
    pub fn publish<F>(&mut self, update: F)
    where
        F: FnOnce(&mut E),
    {
        self.producer.publish(update);
    }

    /// Try to publish an event (delegated to producer)
    pub fn try_publish<F>(
        &mut self,
        update: F,
    ) -> std::result::Result<i64, crate::disruptor::producer::RingBufferFull>
    where
        F: FnOnce(&mut E),
    {
        self.producer.try_publish(update)
    }

    /// Publish a batch of events (delegated to producer)
    pub fn batch_publish<F>(&mut self, n: usize, update: F)
    where
        F: for<'a> FnOnce(crate::disruptor::ring_buffer::BatchIterMut<'a, E>),
    {
        self.producer.batch_publish(n, update);
    }

    /// Try to publish a batch of events (delegated to producer)
    pub fn try_batch_publish<F>(
        &mut self,
        n: usize,
        update: F,
    ) -> std::result::Result<i64, crate::disruptor::producer::MissingFreeSlots>
    where
        F: for<'a> FnOnce(crate::disruptor::ring_buffer::BatchIterMut<'a, E>),
    {
        self.producer.try_batch_publish(n, update)
    }

    /// Shutdown the disruptor and wait for all consumer threads to complete.
    ///
    /// **Recommended Usage**: Call this method explicitly before dropping the
    /// `DisruptorHandle` to ensure controlled shutdown and avoid potential
    /// blocking in the `Drop` implementation of individual consumers.
    ///
    /// This method:
    /// 1. Sets the shutdown flag to signal all consumer threads to stop
    /// 2. Waits for all consumer threads to complete gracefully
    /// 3. Prevents blocking behavior when the disruptor is dropped
    ///
    /// # Example
    /// ```rust,no_run
    /// # use badbatch::disruptor::{build_single_producer, BusySpinWaitStrategy};
    /// # #[derive(Default)]
    /// # struct TestEvent { value: i32 }
    /// let mut disruptor = build_single_producer(1024, TestEvent::default, BusySpinWaitStrategy)
    ///     .handle_events_with(|_event: &mut TestEvent, _seq, _end_of_batch| {})
    ///     .build();
    ///
    /// // Do work...
    ///
    /// // Explicitly shutdown before drop (recommended!)
    /// disruptor.shutdown();
    /// ```
    pub fn shutdown(&mut self) {
        if self.is_shutdown {
            return;
        }
        self.core.shutdown();
        self.is_shutdown = true;
    }

    /// Get the number of active consumer threads
    pub fn consumer_count(&self) -> usize {
        self.core.consumer_count()
    }

    /// Check whether the disruptor has been shut down
    pub fn is_shutdown(&self) -> bool {
        self.is_shutdown
    }
}

impl<E, W> Drop for DisruptorHandle<E, W>
where
    E: Send + Sync + 'static,
    W: WaitStrategy + 'static,
{
    fn drop(&mut self) {
        if !self.is_shutdown {
            self.shutdown();
        }
    }
}
