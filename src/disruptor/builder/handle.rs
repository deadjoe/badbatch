//! Runtime handle for a built disruptor (producer + consumer lifecycle).

use super::core::DisruptorCore;
use crate::disruptor::{
    producer::{Producer, SimpleProducer},
    WaitStrategy,
};
use std::marker::PhantomData;

/// Marker type: the handle was built by `build_single_producer`.
///
/// Single-mode handles own the only producer handle in existence and expose
/// no way to create another one — the single-producer sequencer's claim state
/// is not synchronized between threads (soundness audit 2026-07-18):
///
/// ```compile_fail
/// use badbatch::disruptor::{build_single_producer, BusySpinWaitStrategy};
/// #[derive(Default)]
/// struct MyEvent {
///     value: i64,
/// }
/// let handle = build_single_producer(8, MyEvent::default, BusySpinWaitStrategy)
///     .handle_events_with(|_e: &mut MyEvent, _s, _b| {})
///     .build();
/// let extra = handle.create_producer(); // ERROR: single mode has no create_producer
/// ```
#[derive(Debug)]
pub struct SingleProducerMode;

/// Marker type: the handle was built by `build_multi_producer`.
///
/// Multi-mode handles may hand out one producer per publishing thread via
/// [`DisruptorHandle::create_producer`].
#[derive(Debug)]
pub struct MultiProducerMode;

/// Handle for managing a Disruptor with running consumer threads
///
/// This handle provides access to the producer and manages the lifecycle
/// of consumer threads. When dropped, it will attempt to gracefully
/// shutdown all consumer threads.
///
/// The `M` type parameter records the producer mode at the type level:
/// only `DisruptorHandle<_, _, MultiProducerMode>` has `create_producer`.
#[derive(Debug)]
pub struct DisruptorHandle<E, W, M = SingleProducerMode>
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
    /// Producer mode marker (single vs multi)
    _mode: PhantomData<M>,
}

impl<E, W, M> DisruptorHandle<E, W, M>
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
            _mode: PhantomData,
        }
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

impl<E, W> DisruptorHandle<E, W, MultiProducerMode>
where
    E: Send + Sync + 'static,
    W: WaitStrategy + 'static,
{
    /// Creates a new producer that shares the same ring buffer and sequencer
    ///
    /// Multi-producer mode only: create one producer handle per publishing
    /// thread. Single-mode handles deliberately do not have this method — a
    /// second handle over a single-producer sequencer races on its non-atomic
    /// claim state (soundness audit 2026-07-18).
    pub fn create_producer(&self) -> SimpleProducer<E, W> {
        self.core.create_producer()
    }
}

impl<E, W, M> Drop for DisruptorHandle<E, W, M>
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
