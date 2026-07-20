//! Runtime handle for a built disruptor (producer + consumer lifecycle).

use super::core::DisruptorCore;
use crate::disruptor::{
    producer::{Producer, SimpleProducer},
    FailureRecord, Sequencer, WaitStrategy,
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

    /// Return benchmark-only single-producer capacity-wait counters.
    ///
    /// `None` means this is a multi-producer handle. This API is compiled only
    /// for the opt-in per-round H2H diagnostics build.
    #[cfg(feature = "bench-round-diagnostics")]
    #[doc(hidden)]
    pub fn bench_producer_backpressure_snapshot(
        &self,
    ) -> Option<crate::disruptor::sequencer::BenchProducerBackpressureSnapshot> {
        self.core.sequencer.bench_producer_backpressure_snapshot()
    }

    /// Consume the handle and return the producer
    ///
    /// Stops consumer threads and removes gating so the ring can wrap freely,
    /// but **does not** close the claim path — the returned producer must still
    /// publish. For a full stop that rejects further claims, use [`Self::halt`]
    /// or [`Self::shutdown`] instead.
    pub fn into_producer(mut self) -> SimpleProducer<E, W> {
        // Stop consumers without sequencer.close() (see stop_consumers_keep_claims).
        self.core.stop_consumers_keep_claims();
        self.is_shutdown = true;

        // Move the producer out while leaving a placeholder behind so Drop can run normally.
        let placeholder = self.core.create_producer();
        std::mem::replace(&mut self.producer, placeholder)
    }

    /// Publish an event using a closure (delegated to producer)
    ///
    /// # Errors
    /// Propagates the claim error (2026-07-18 audit: no silent drops).
    pub fn publish<F>(&mut self, update: F) -> crate::disruptor::Result<i64>
    where
        F: FnOnce(&mut E),
    {
        if self.is_shutdown {
            return Err(crate::disruptor::DisruptorError::Shutdown);
        }
        self.producer.publish(update)
    }

    /// Try to publish an event (delegated to producer)
    ///
    /// # Errors
    /// [`TryPublishError::Full`](crate::disruptor::producer::TryPublishError::Full) /
    /// [`TryPublishError::Contended`](crate::disruptor::producer::TryPublishError::Contended)
    /// on transient backpressure (retry is meaningful); a terminal
    /// [`TryPublishError::Shutdown`](crate::disruptor::producer::TryPublishError::Shutdown)
    /// after shutdown/halt or
    /// [`TryPublishError::Poisoned`](crate::disruptor::producer::TryPublishError::Poisoned)
    /// on a poisoned pipeline (retrying can never succeed).
    pub fn try_publish<F>(
        &mut self,
        update: F,
    ) -> std::result::Result<i64, crate::disruptor::producer::TryPublishError>
    where
        F: FnOnce(&mut E),
    {
        if self.is_shutdown {
            return Err(crate::disruptor::producer::TryPublishError::Shutdown);
        }
        self.producer.try_publish(update)
    }

    /// Publish a batch of events (delegated to producer)
    ///
    /// # Errors
    /// Propagates the claim error (2026-07-18 audit: no silent drops).
    pub fn batch_publish<F>(&mut self, n: usize, update: F) -> crate::disruptor::Result<i64>
    where
        F: for<'a> FnOnce(crate::disruptor::ring_buffer::BatchIterMut<'a, E>),
    {
        if self.is_shutdown {
            return Err(crate::disruptor::DisruptorError::Shutdown);
        }
        self.producer.batch_publish(n, update)
    }

    /// Try to publish a batch of events (delegated to producer)
    ///
    /// # Errors
    /// [`TryPublishError::MissingFreeSlots`](crate::disruptor::producer::TryPublishError::MissingFreeSlots) /
    /// [`TryPublishError::Contended`](crate::disruptor::producer::TryPublishError::Contended)
    /// on transient backpressure;
    /// [`TryPublishError::InvalidBatchSize`](crate::disruptor::producer::TryPublishError::InvalidBatchSize)
    /// for a zero or over-capacity request; or a terminal
    /// [`TryPublishError::Shutdown`](crate::disruptor::producer::TryPublishError::Shutdown) /
    /// [`TryPublishError::Poisoned`](crate::disruptor::producer::TryPublishError::Poisoned)
    /// where retrying can never succeed.
    pub fn try_batch_publish<F>(
        &mut self,
        n: usize,
        update: F,
    ) -> std::result::Result<i64, crate::disruptor::producer::TryPublishError>
    where
        F: for<'a> FnOnce(crate::disruptor::ring_buffer::BatchIterMut<'a, E>),
    {
        if self.is_shutdown {
            return Err(crate::disruptor::producer::TryPublishError::Shutdown);
        }
        self.producer.try_batch_publish(n, update)
    }

    /// Draining shutdown: wait until consumers have processed the published
    /// backlog, then stop and join all consumer threads (LMAX `shutdown()`).
    ///
    /// The drain aborts early (proceeding straight to the abrupt stop) when
    /// the pipeline is poisoned or a consumer thread has already died. Stop
    /// publishing before calling this — with a live consumer that never
    /// catches up this blocks indefinitely; use [`Self::shutdown_timeout`]
    /// for a bounded wait or [`Self::halt`] for an immediate stop.
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
        let _ = self.core.shutdown_with_timeout(None);
        self.is_shutdown = true;
    }

    /// Draining shutdown with a deadline.
    ///
    /// Drains like [`Self::shutdown`] but gives up after `timeout` and stops
    /// abruptly, reporting what interrupted the drain.
    ///
    /// # Errors
    /// - [`crate::disruptor::DisruptorError::Timeout`] — backlog remained when the deadline hit
    /// - [`crate::disruptor::DisruptorError::Poisoned`] — pipeline was poisoned
    /// - [`crate::disruptor::DisruptorError::ShutdownError`] — a consumer died before draining
    ///
    /// The disruptor is stopped in every case; the error only reports drain
    /// completeness.
    pub fn shutdown_timeout(
        &mut self,
        timeout: std::time::Duration,
    ) -> crate::disruptor::Result<()> {
        if self.is_shutdown {
            return Ok(());
        }
        let result = self.core.shutdown_with_timeout(Some(timeout));
        self.is_shutdown = true;
        result
    }

    /// Abrupt stop (LMAX `halt()`): signal consumers to stop and join them
    /// without draining the published backlog.
    pub fn halt(&mut self) {
        if self.is_shutdown {
            return;
        }
        self.core.halt();
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

    /// Whether the pipeline was poisoned by a fatal consumer/producer failure.
    ///
    /// Once true, further publishes fail with
    /// [`crate::disruptor::DisruptorError::Poisoned`] /
    /// [`crate::disruptor::TryPublishError::Poisoned`].
    #[must_use]
    pub fn is_poisoned(&self) -> bool {
        self.core.sequencer.is_poisoned()
    }

    /// Return the first structured failure retained by the pipeline.
    ///
    /// This is the diagnostic companion to [`Self::is_poisoned`] and terminal
    /// `Poisoned` publish errors. Handler errors include phase, thread, Builder
    /// stage and event sequence when available; event payloads are never stored.
    #[must_use]
    pub fn first_failure(&self) -> Option<FailureRecord> {
        self.core.sequencer.first_failure()
    }

    /// Whether the sequencer claim path is closed (after halt/shutdown).
    ///
    /// Distinct from [`Self::is_shutdown`]: the handle flag is set when stop
    /// methods return; the claim path closes first so in-flight producers fail
    /// fast.
    #[must_use]
    pub fn is_claim_closed(&self) -> bool {
        self.core.sequencer.is_closed()
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

    /// Shared multi-producer handle that is [`Clone`] and can mint further
    /// [`SimpleProducer`]s — the supported public path to
    /// [`super::CloneableProducer`] (wired residual 2026-07-19).
    ///
    /// ```compile_fail
    /// use badbatch::disruptor::{build_single_producer, BusySpinWaitStrategy};
    /// #[derive(Default)]
    /// struct MyEvent { value: i64 }
    /// let handle = build_single_producer(8, MyEvent::default, BusySpinWaitStrategy)
    ///     .handle_events_with(|_e: &mut MyEvent, _s, _b| {})
    ///     .build();
    /// let _ = handle.cloneable_producer(); // ERROR: single mode has no cloneable_producer
    /// ```
    pub fn cloneable_producer(&self) -> super::CloneableProducer<E, W> {
        super::CloneableProducer::new(
            std::sync::Arc::clone(&self.core.ring_buffer),
            self.core.sequencer.clone(),
        )
    }
}

impl<E, W, M> Drop for DisruptorHandle<E, W, M>
where
    E: Send + Sync + 'static,
    W: WaitStrategy + 'static,
{
    fn drop(&mut self) {
        if !self.is_shutdown {
            // Bounded drain so Drop cannot hang forever on a stuck consumer;
            // falls back to an abrupt stop when the deadline hits.
            let _ = self.shutdown_timeout(std::time::Duration::from_secs(2));
        }
    }
}
