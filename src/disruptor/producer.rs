//! Producer module inspired by disruptor-rs
//!
//! This module provides a simplified, user-friendly API for publishing events
//! into the Disruptor, following the patterns established by disruptor-rs.
//!
//! This implementation has been corrected to properly integrate with the Sequencer
//! component, following the LMAX Disruptor design principles.

use crate::disruptor::sequencer::SequencerEnum;
use crate::disruptor::{
    failure::{current_thread_name, log_failure, FailureDecision},
    ring_buffer::BatchIterMut,
    FailurePhase, FailureRecord, Result, RingBuffer, Sequencer, WaitStrategy,
};
use std::convert::TryFrom;
use std::marker::PhantomData;
use std::sync::Arc;

/// Error indicating that the ring buffer is full
#[derive(Debug, Clone, PartialEq)]
pub struct RingBufferFull;

impl std::fmt::Display for RingBufferFull {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Ring Buffer is full")
    }
}

impl std::error::Error for RingBufferFull {}

/// Error indicating missing free slots for batch publication
#[derive(Debug, Clone, PartialEq)]
pub struct MissingFreeSlots(pub u64);

impl std::fmt::Display for MissingFreeSlots {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Missing free slots in Ring Buffer: {}", self.0)
    }
}

impl std::error::Error for MissingFreeSlots {}

/// Why a non-blocking publish attempt was rejected.
///
/// Distinguishes *transient* backpressure (retrying is meaningful) from
/// *terminal* pipeline states (retrying can never succeed). Before the
/// 2026-07-19 audit the try path reported terminal states as "ring full",
/// which turned a poisoned or shut-down pipeline into an infinite retry loop
/// in caller code.
#[derive(Debug, Clone, PartialEq)]
pub enum TryPublishError {
    /// The ring buffer is full right now — transient; retrying is meaningful.
    Full(RingBufferFull),
    /// A multi-producer claim lost a CAS race while capacity is still
    /// available — transient; retrying is meaningful.
    Contended,
    /// Not enough free slots for the requested batch (carries the deficit).
    /// The batch size has already been validated against the ring capacity, so
    /// this is transient backpressure and retrying is meaningful.
    MissingFreeSlots(MissingFreeSlots),
    /// The requested batch size is zero, exceeds the ring capacity, or cannot
    /// be represented by the sequencer — retrying the same request can never
    /// succeed.
    InvalidBatchSize {
        /// Number of slots requested by the caller.
        requested: usize,
        /// Maximum batch size supported by this ring.
        capacity: usize,
    },
    /// The pipeline was poisoned by a fatal producer/consumer failure —
    /// terminal; retrying will never succeed. Query `first_failure()` on a
    /// shared runtime handle or producer for causal context.
    Poisoned,
    /// The sequencer was closed by shutdown/halt — terminal; retrying will
    /// never succeed.
    Shutdown,
    /// Nested single-producer DSL publish from inside a translator callback.
    /// Not transient: retrying the same nested call will fail the same way.
    ReentrantPublish,
}

impl TryPublishError {
    /// `true` when the pipeline itself is terminal
    /// ([`TryPublishError::Poisoned`] or [`TryPublishError::Shutdown`]).
    ///
    /// The intended retry discipline:
    ///
    /// ```rust
    /// use badbatch::disruptor::{build_single_producer, BusySpinWaitStrategy, TryPublishError};
    ///
    /// #[derive(Default)]
    /// struct MyEvent { price: f64 }
    ///
    /// let mut producer = build_single_producer(8, MyEvent::default, BusySpinWaitStrategy)
    ///     .handle_events_with(|_event, _sequence, _end_of_batch| {})
    ///     .build();
    ///
    /// loop {
    ///     match producer.try_publish(|e| e.price = 42.0) {
    ///         Ok(_) => break,
    ///         Err(e) if e.is_transient() => std::hint::spin_loop(),
    ///         Err(e) => panic!("terminal publish failure: {e}"),
    ///     }
    /// }
    /// # drop(producer); // Clean shutdown
    /// ```
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Poisoned | Self::Shutdown)
    }

    /// `true` when the rejection is transient backpressure and retrying is
    /// meaningful.
    ///
    /// [`TryPublishError::InvalidBatchSize`] and
    /// [`TryPublishError::ReentrantPublish`] are neither transient nor a
    /// terminal pipeline state: the pipeline is still usable, but retrying the
    /// same invalid request can never succeed.
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            Self::Full(_) | Self::Contended | Self::MissingFreeSlots(_)
        )
    }
}

impl std::fmt::Display for TryPublishError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Full(e) => write!(f, "{e}"),
            Self::Contended => write!(f, "Another producer won the claim race"),
            Self::MissingFreeSlots(e) => write!(f, "{e}"),
            Self::InvalidBatchSize {
                requested,
                capacity,
            } => write!(
                f,
                "Invalid batch size {requested}; expected a value in 1..={capacity}"
            ),
            Self::Poisoned => write!(
                f,
                "Pipeline was poisoned by a panic; publishing is disabled"
            ),
            Self::Shutdown => write!(
                f,
                "Sequencer is closed (shutdown/halt); publishing is disabled"
            ),
            Self::ReentrantPublish => write!(
                f,
                "Reentrant publish from inside a single-producer DSL translator"
            ),
        }
    }
}

impl std::error::Error for TryPublishError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Full(e) => Some(e),
            Self::MissingFreeSlots(e) => Some(e),
            Self::Contended
            | Self::InvalidBatchSize { .. }
            | Self::Poisoned
            | Self::Shutdown
            | Self::ReentrantPublish => None,
        }
    }
}

impl From<RingBufferFull> for TryPublishError {
    fn from(e: RingBufferFull) -> Self {
        Self::Full(e)
    }
}

impl From<MissingFreeSlots> for TryPublishError {
    fn from(e: MissingFreeSlots) -> Self {
        Self::MissingFreeSlots(e)
    }
}

/// Producer trait inspired by disruptor-rs
///
/// Provides a simplified API for publishing events into the Disruptor.
/// This follows the exact pattern from disruptor-rs for maximum compatibility
/// and ease of use.
pub trait Producer<E>
where
    E: Send + Sync,
{
    /// Publish an Event into the Disruptor
    ///
    /// Returns a `Result` with the published sequence number or a
    /// [`TryPublishError`] describing why the claim was rejected:
    /// [`TryPublishError::Full`] / [`TryPublishError::Contended`] (transient
    /// backpressure — retrying is meaningful), or a terminal state
    /// ([`TryPublishError::Poisoned`] / [`TryPublishError::Shutdown`]) where
    /// retrying can never succeed.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use badbatch::disruptor::{build_single_producer, BusySpinWaitStrategy};
    ///
    /// #[derive(Default)]
    /// struct MyEvent { price: f64 }
    ///
    /// let mut producer = build_single_producer(8, MyEvent::default, BusySpinWaitStrategy)
    ///     .handle_events_with(|_event, _sequence, _end_of_batch| {
    ///         // Handle event
    ///     })
    ///     .build();
    ///
    /// let sequence = producer.try_publish(|e| { e.price = 42.0; }).unwrap();
    /// # drop(producer); // Clean shutdown
    /// ```
    fn try_publish<F>(&mut self, update: F) -> std::result::Result<i64, TryPublishError>
    where
        F: FnOnce(&mut E);

    /// Publish a batch of Events into the Disruptor
    ///
    /// Returns a `Result` with the upper published sequence number or a
    /// [`TryPublishError`]: [`TryPublishError::MissingFreeSlots`] /
    /// [`TryPublishError::Contended`] for transient backpressure,
    /// [`TryPublishError::InvalidBatchSize`] for a zero or over-capacity
    /// request, or a terminal state ([`TryPublishError::Poisoned`] /
    /// [`TryPublishError::Shutdown`]).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use badbatch::disruptor::{build_single_producer, BusySpinWaitStrategy};
    ///
    /// #[derive(Default)]
    /// struct MyEvent { price: f64 }
    ///
    /// let mut producer = build_single_producer(8, MyEvent::default, BusySpinWaitStrategy)
    ///     .handle_events_with(|_event, _sequence, _end_of_batch| {
    ///         // Handle event
    ///     })
    ///     .build();
    ///
    /// let sequence = producer.try_batch_publish(3, |iter| {
    ///     for e in iter {
    ///         e.price = 42.0;
    ///     }
    /// }).unwrap();
    /// # drop(producer); // Clean shutdown
    /// ```
    fn try_batch_publish<'a, F>(
        &'a mut self,
        n: usize,
        update: F,
    ) -> std::result::Result<i64, TryPublishError>
    where
        E: 'a,
        F: FnOnce(BatchIterMut<'a, E>);

    /// Publish an Event into the Disruptor
    ///
    /// Spins until there is an available slot in case the ring buffer is full.
    ///
    /// # Errors
    /// Returns the underlying claim error instead of silently dropping the
    /// event (2026-07-18 audit): currently this means the sequencer rejected
    /// the claim (invalid state or a failed/poisoned pipeline). On success the
    /// published sequence number is returned.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use badbatch::disruptor::{build_single_producer, BusySpinWaitStrategy};
    ///
    /// #[derive(Default)]
    /// struct MyEvent { price: f64 }
    ///
    /// let mut producer = build_single_producer(8, MyEvent::default, BusySpinWaitStrategy)
    ///     .handle_events_with(|_event, _sequence, _end_of_batch| {
    ///         // Handle event
    ///     })
    ///     .build();
    ///
    /// let sequence = producer.publish(|e| { e.price = 42.0; }).unwrap();
    /// # drop(producer); // Clean shutdown
    /// ```
    fn publish<F>(&mut self, update: F) -> Result<i64>
    where
        F: FnOnce(&mut E);

    /// Publish a batch of Events into the Disruptor
    ///
    /// Spins until there are enough available slots in the ring buffer.
    ///
    /// # Errors
    /// Returns the underlying claim error instead of silently dropping the
    /// batch (2026-07-18 audit): a batch larger than the ring buffer or a
    /// failed/poisoned pipeline yields an error and the update closure never
    /// runs. On success the highest published sequence number is returned.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use badbatch::disruptor::{build_single_producer, BusySpinWaitStrategy};
    ///
    /// #[derive(Default)]
    /// struct MyEvent { price: f64 }
    ///
    /// let mut producer = build_single_producer(8, MyEvent::default, BusySpinWaitStrategy)
    ///     .handle_events_with(|_event, _sequence, _end_of_batch| {
    ///         // Handle event
    ///     })
    ///     .build();
    ///
    /// let last = producer.batch_publish(3, |iter| {
    ///     for e in iter {
    ///         e.price = 42.0;
    ///     }
    /// }).unwrap();
    /// # drop(producer); // Clean shutdown
    /// ```
    fn batch_publish<'a, F>(&'a mut self, n: usize, update: F) -> Result<i64>
    where
        E: 'a,
        F: FnOnce(BatchIterMut<'a, E>);
}

/// Simple producer implementation that properly integrates with Sequencer
///
/// This producer works with any Sequencer implementation (single or multi-producer)
/// and correctly follows the LMAX Disruptor pattern for sequence allocation and publishing.
///
/// # Threading model
///
/// `SimpleProducer` is **`Send` but not `Sync`**: one publishing thread owns each handle.
/// - **Single-producer**: exactly one handle exists; it is deliberately not `Clone`
///   and single-mode builds expose no way to create a second one.
/// - **Multi-producer**: obtain one handle per publishing thread from the
///   multi-mode `DisruptorHandle::create_producer` (handles share the sequencer).
///
/// This encodes LMAX's exclusive-publisher discipline in the type system instead of comments.
///
/// Cloning a producer handle must not compile — a second handle over a
/// single-producer sequencer races on its non-atomic claim state:
///
/// ```compile_fail
/// use badbatch::disruptor::{
///     open_single_producer_poller, BusySpinWaitStrategy, DefaultEventFactory,
/// };
/// let (producer, _poller, _shutdown) = open_single_producer_poller(
///     8,
///     DefaultEventFactory::<i64>::new(),
///     BusySpinWaitStrategy,
/// )
/// .unwrap();
/// let second = producer.clone(); // ERROR: SimpleProducer is not Clone
/// ```
#[derive(Debug)]
pub struct SimpleProducer<T, W>
where
    T: Send + Sync,
    W: WaitStrategy + 'static,
{
    ring_buffer: Arc<RingBuffer<T>>,
    sequencer: SequencerEnum<W>,
    /// Marks this type `!Sync` so handles cannot be shared across threads by accident.
    _not_sync: PhantomData<*const ()>,
}

// SAFETY: SimpleProducer contains only Send fields (Arcs). The `*const ()` phantom
// makes the type !Sync without affecting Send. Each handle is intended for a single
// publishing thread; multi-producer clones are independent handles on the same sequencer.
unsafe impl<T, W> Send for SimpleProducer<T, W>
where
    T: Send + Sync,
    W: WaitStrategy + 'static,
{
}

/// Poisons the sequencer if dropped: a claim was made but the user's update
/// closure panicked before publish, so the slot would otherwise be exposed as
/// a never-written event on the next publish (2026-07-18 audit).
///
/// Usage: construct before calling into user code, `std::mem::forget` after it
/// returns. On the success path the forget makes the guard free (no drop glue
/// runs); the `Drop` impl is only reachable while unwinding past the guard.
/// This deliberately avoids `std::thread::panicking()`, whose per-call TLS
/// access cost ~59% of end-to-end unicast throughput when it sat on the
/// per-event publish path (P3 baseline bisection, 2026-07-19).
pub(crate) struct PoisonOnPanic<'a, W: WaitStrategy + 'static> {
    sequencer: &'a SequencerEnum<W>,
    first_sequence: i64,
    last_sequence: i64,
}

impl<'a, W: WaitStrategy + 'static> PoisonOnPanic<'a, W> {
    pub(crate) fn new(
        sequencer: &'a SequencerEnum<W>,
        first_sequence: i64,
        last_sequence: i64,
    ) -> Self {
        Self {
            sequencer,
            first_sequence,
            last_sequence,
        }
    }
}

impl<W: WaitStrategy + 'static> Drop for PoisonOnPanic<'_, W> {
    fn drop(&mut self) {
        let message = if self.first_sequence == self.last_sequence {
            "producer update panicked after claim and before publish".to_string()
        } else {
            format!(
                "producer batch update panicked after claiming sequences {}..={}",
                self.first_sequence, self.last_sequence
            )
        };
        let failure = FailureRecord::new(FailurePhase::ProducerPanic, message)
            .with_thread_name(current_thread_name())
            .with_sequence(self.first_sequence);
        self.sequencer.record_failure(&failure);
        log_failure(&failure, FailureDecision::Poison);
        self.sequencer.poison();
    }
}

impl<T, W> SimpleProducer<T, W>
where
    T: Send + Sync,
    W: WaitStrategy + 'static,
{
    /// Create a new simple producer
    ///
    /// Crate-private on purpose (soundness audit 2026-07-18): pairing an
    /// arbitrary ring buffer with an arbitrary sequencer from safe code allowed
    /// two producers with independent sequencers to claim the same slots.
    /// Producers are obtained through the builder, `CloneableProducer`, or
    /// [`crate::disruptor::open_single_producer_poller`].
    ///
    /// # Arguments
    /// * `ring_buffer` - The ring buffer to publish to
    /// * `sequencer` - The sequencer for coordinating sequence allocation
    ///
    /// # Returns
    /// A new SimpleProducer instance
    pub(crate) fn new(ring_buffer: Arc<RingBuffer<T>>, sequencer: SequencerEnum<W>) -> Self {
        Self {
            ring_buffer,
            sequencer,
            _not_sync: PhantomData,
        }
    }

    /// Duplicate this handle for another publishing thread.
    ///
    /// Crate-private capability: callers must guarantee the underlying sequencer
    /// is the multi-producer variant. `SimpleProducer` deliberately does not
    /// implement `Clone` — a cloned handle over a `SingleProducerSequencer`
    /// races on its non-atomic claim state (soundness audit 2026-07-18).
    pub(crate) fn duplicate_for_multi(&self) -> Self {
        debug_assert!(
            matches!(self.sequencer, SequencerEnum::Multi(_)),
            "duplicate_for_multi requires a multi-producer sequencer"
        );
        Self {
            ring_buffer: Arc::clone(&self.ring_buffer),
            sequencer: self.sequencer.clone(),
            _not_sync: PhantomData,
        }
    }

    /// Get the current cursor from the sequencer
    pub fn current_sequence(&self) -> i64 {
        self.sequencer.get_cursor().get()
    }

    /// Return the first structured failure retained by the shared pipeline.
    #[must_use]
    pub fn first_failure(&self) -> Option<FailureRecord> {
        self.sequencer.first_failure()
    }

    /// Classify a rejected try-claim.
    ///
    /// Terminal pipeline states must not be reported as transient backpressure
    /// (2026-07-19 audit). The poisoned/closed flags are monotonic — once set
    /// they are never cleared — so observing them after a `None` claim is
    /// conclusive. In the race where the claim actually failed on fullness
    /// *and* a terminal flag was set concurrently, the pipeline is terminal
    /// anyway, so reporting the terminal state is strictly more useful.
    fn classify_try_rejection(&self, fallback: TryPublishError) -> TryPublishError {
        if self.sequencer.is_poisoned() {
            TryPublishError::Poisoned
        } else if self.sequencer.is_closed() {
            TryPublishError::Shutdown
        } else {
            fallback
        }
    }

    // Note: wait_for_capacity is no longer needed as we use sequencer.next() which blocks
}

impl<T, W> Producer<T> for SimpleProducer<T, W>
where
    T: Send + Sync,
    W: WaitStrategy + 'static,
{
    fn try_publish<F>(&mut self, update: F) -> std::result::Result<i64, TryPublishError>
    where
        F: FnOnce(&mut T),
    {
        // Try to claim the next sequence from the sequencer
        let Some(sequence) = self.sequencer.try_next() else {
            let transient = if self.sequencer.remaining_capacity() > 0 {
                // MultiProducerSequencer intentionally makes one CAS attempt on
                // the try path. A failed CAS with capacity still available is
                // contention, not a full ring.
                TryPublishError::Contended
            } else {
                TryPublishError::Full(RingBufferFull)
            };
            return Err(self.classify_try_rejection(transient));
        };

        // Get the event at the claimed sequence and update it
        // SAFETY: We have exclusive access to this sequence from the sequencer
        let event = unsafe { &mut *self.ring_buffer.get_mut_unchecked(sequence) };
        let poison_guard = PoisonOnPanic::new(&self.sequencer, sequence, sequence);
        update(event);
        std::mem::forget(poison_guard); // disarm: Drop only runs while unwinding

        // Publish the sequence to make it available to consumers
        self.sequencer.publish(sequence);
        Ok(sequence)
    }

    fn try_batch_publish<'a, F>(
        &'a mut self,
        n: usize,
        update: F,
    ) -> std::result::Result<i64, TryPublishError>
    where
        T: 'a,
        F: FnOnce(BatchIterMut<'a, T>),
    {
        let capacity = self.sequencer.get_buffer_size();
        if n == 0 || n > capacity {
            return Err(
                self.classify_try_rejection(TryPublishError::InvalidBatchSize {
                    requested: n,
                    capacity,
                }),
            );
        }

        // Try to claim n sequences from the sequencer. A real ring cannot have
        // more than i64::MAX addressable sequence slots, but keep this public
        // fallible API panic-free if an implementation reports otherwise.
        let Ok(batch_size) = i64::try_from(n) else {
            return Err(
                self.classify_try_rejection(TryPublishError::InvalidBatchSize {
                    requested: n,
                    capacity,
                }),
            );
        };
        let Some(end_sequence) = self.sequencer.try_next_n(batch_size) else {
            #[allow(clippy::cast_sign_loss)]
            let remaining = self.sequencer.remaining_capacity().max(0) as u64;
            let deficit = (n as u64).saturating_sub(remaining);
            let transient = if deficit == 0 {
                // Capacity remains, so the single failed CAS was contention.
                TryPublishError::Contended
            } else {
                TryPublishError::MissingFreeSlots(MissingFreeSlots(deficit))
            };
            return Err(self.classify_try_rejection(transient));
        };

        let start_sequence = end_sequence - (batch_size - 1);

        // SAFETY: We have exclusive access to this sequence range from the sequencer
        let iter = unsafe {
            self.ring_buffer
                .batch_iter_mut(start_sequence, end_sequence)
        };
        let poison_guard = PoisonOnPanic::new(&self.sequencer, start_sequence, end_sequence);
        update(iter);
        std::mem::forget(poison_guard); // disarm: Drop only runs while unwinding

        // Publish the entire range to make it available to consumers
        self.sequencer.publish_range(start_sequence, end_sequence);
        Ok(end_sequence)
    }

    fn publish<F>(&mut self, update: F) -> Result<i64>
    where
        F: FnOnce(&mut T),
    {
        // Claim the next sequence from the sequencer (blocking). Claim errors
        // are delivered to the caller, never logged-and-dropped (2026-07-18 audit).
        let sequence = self.sequencer.next()?;

        // Get the event at the claimed sequence and update it
        // SAFETY: We have exclusive access to this sequence from the sequencer
        let event = unsafe { &mut *self.ring_buffer.get_mut_unchecked(sequence) };
        let poison_guard = PoisonOnPanic::new(&self.sequencer, sequence, sequence);
        update(event);
        std::mem::forget(poison_guard); // disarm: Drop only runs while unwinding

        // Publish the sequence to make it available to consumers
        self.sequencer.publish(sequence);
        Ok(sequence)
    }

    fn batch_publish<'a, F>(&'a mut self, n: usize, update: F) -> Result<i64>
    where
        T: 'a,
        F: FnOnce(BatchIterMut<'a, T>),
    {
        // Validate before claim so zero / over-capacity / unrepresentable sizes
        // never panic or silently drop (aligned with try_batch_publish).
        let capacity = self.sequencer.get_buffer_size();
        if n == 0 || n > capacity {
            return Err(crate::disruptor::DisruptorError::InvalidSequence(
                i64::try_from(n).unwrap_or(i64::MAX),
            ));
        }
        let batch_size = i64::try_from(n)
            .map_err(|_| crate::disruptor::DisruptorError::InvalidSequence(i64::MAX))?;

        // Claim n sequences from the sequencer (blocking). Claim errors are
        // delivered to the caller, never logged-and-dropped (2026-07-18 audit).
        let end_sequence = self.sequencer.next_n(batch_size)?;
        let start_sequence = end_sequence - (batch_size - 1);

        // SAFETY: We have exclusive access to this sequence range from the sequencer
        let iter = unsafe {
            self.ring_buffer
                .batch_iter_mut(start_sequence, end_sequence)
        };
        let poison_guard = PoisonOnPanic::new(&self.sequencer, start_sequence, end_sequence);
        update(iter);
        std::mem::forget(poison_guard); // disarm: Drop only runs while unwinding

        // Publish the entire range to make it available to consumers
        self.sequencer.publish_range(start_sequence, end_sequence);
        Ok(end_sequence)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disruptor::{
        event_factory::DefaultEventFactory,
        sequencer::{MultiProducerSequencer, SingleProducerSequencer},
        wait_strategy::BusySpinWaitStrategy,
    };
    use std::sync::Arc;

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

    fn create_test_producer() -> SimpleProducer<TestEvent, BusySpinWaitStrategy> {
        let factory = DefaultEventFactory::<TestEvent>::new();
        let ring_buffer =
            Arc::new(RingBuffer::new(8, factory).expect("Failed to create ring buffer"));
        let sequencer = SequencerEnum::Single(Arc::new(unsafe {
            SingleProducerSequencer::new(8, Arc::new(BusySpinWaitStrategy))
        }));
        SimpleProducer::new(ring_buffer, sequencer)
    }

    fn create_multi_producer_test_producer() -> SimpleProducer<TestEvent, BusySpinWaitStrategy> {
        let factory = DefaultEventFactory::<TestEvent>::new();
        let ring_buffer =
            Arc::new(RingBuffer::new(16, factory).expect("Failed to create ring buffer"));
        let sequencer = SequencerEnum::Multi(Arc::new(MultiProducerSequencer::new(
            16,
            Arc::new(BusySpinWaitStrategy),
        )));
        SimpleProducer::new(ring_buffer, sequencer)
    }

    #[test]
    fn test_simple_producer_creation() {
        let producer = create_test_producer();

        // Should start with sequence -1 (no events published yet)
        assert_eq!(producer.current_sequence(), -1);
    }

    #[test]
    fn test_try_publish_success() {
        let mut producer = create_test_producer();

        let result = producer.try_publish(|event| {
            event.value = 42;
            event.data = "test".to_string();
        });

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0); // First sequence should be 0
        assert_eq!(producer.current_sequence(), 0);
    }

    #[test]
    fn test_try_publish_multiple() {
        let mut producer = create_test_producer();

        // Publish multiple events
        for i in 0..5 {
            let result = producer.try_publish(|event| {
                event.value = i;
                event.data = format!("event_{i}");
            });

            assert!(result.is_ok());
            assert_eq!(result.unwrap(), i);
        }

        assert_eq!(producer.current_sequence(), 4);
    }

    #[test]
    fn test_publish_blocking() {
        let mut producer = create_test_producer();

        // Publish an event (should not block for first few events)
        let _ = producer.publish(|event| {
            event.value = 100;
            event.data = "blocking_test".to_string();
        });

        assert_eq!(producer.current_sequence(), 0);
    }

    #[test]
    fn test_try_batch_publish_success() {
        let mut producer = create_test_producer();

        let result = producer.try_batch_publish(3, |iter| {
            for (i, event) in iter.enumerate() {
                event.value = i64::try_from(i).expect("index fits in i64");
                event.data = format!("batch_{i}");
            }
        });

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 2); // End sequence should be 2 (0, 1, 2)
        assert_eq!(producer.current_sequence(), 2);
    }

    #[test]
    fn test_try_batch_publish_zero_size() {
        let mut producer = create_test_producer();

        let err = producer
            .try_batch_publish(0, |_iter| panic!("invalid batch closure must not run"))
            .unwrap_err();

        assert_eq!(
            err,
            TryPublishError::InvalidBatchSize {
                requested: 0,
                capacity: 8,
            }
        );
        assert!(!err.is_transient());
        assert!(!err.is_terminal());
        assert_eq!(producer.current_sequence(), -1);
    }

    #[test]
    fn test_batch_publish_blocking() {
        let mut producer = create_test_producer();

        // Publish a batch (should not block for first batch)
        let _ = producer.batch_publish(2, |iter| {
            for (i, event) in iter.enumerate() {
                event.value = i64::try_from(i + 10).expect("index fits in i64");
                event.data = format!("blocking_batch_{i}");
            }
        });

        assert_eq!(producer.current_sequence(), 1); // End sequence for batch of 2
    }

    #[test]
    fn test_producer_with_multi_producer_sequencer() {
        let mut producer = create_multi_producer_test_producer();

        // Should work the same way with multi-producer sequencer
        let result = producer.try_publish(|event| {
            event.value = 999;
            event.data = "multi_producer_test".to_string();
        });

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);
        assert_eq!(producer.current_sequence(), 0);
    }

    #[test]
    fn test_error_types() {
        // Test RingBufferFull error
        let ring_buffer_full = RingBufferFull;
        assert_eq!(ring_buffer_full.to_string(), "Ring Buffer is full");

        // Test MissingFreeSlots error
        let missing_slots = MissingFreeSlots(5);
        assert_eq!(
            missing_slots.to_string(),
            "Missing free slots in Ring Buffer: 5"
        );
    }

    #[test]
    fn test_try_publish_error_classification() {
        // Transient backpressure variants are retryable.
        let full = TryPublishError::Full(RingBufferFull);
        assert!(full.is_transient());
        assert!(!full.is_terminal());
        assert_eq!(full.to_string(), "Ring Buffer is full");

        let contended = TryPublishError::Contended;
        assert!(contended.is_transient());
        assert!(!contended.is_terminal());
        assert_eq!(contended.to_string(), "Another producer won the claim race");

        let missing = TryPublishError::MissingFreeSlots(MissingFreeSlots(3));
        assert!(missing.is_transient());
        assert_eq!(missing.to_string(), "Missing free slots in Ring Buffer: 3");

        // Invalid input is non-retryable without making the pipeline terminal.
        let invalid = TryPublishError::InvalidBatchSize {
            requested: 9,
            capacity: 8,
        };
        assert!(!invalid.is_transient());
        assert!(!invalid.is_terminal());
        assert_eq!(
            invalid.to_string(),
            "Invalid batch size 9; expected a value in 1..=8"
        );

        // Terminal variants must never be retried.
        let poisoned = TryPublishError::Poisoned;
        assert!(poisoned.is_terminal());
        assert!(!poisoned.is_transient());

        let shutdown = TryPublishError::Shutdown;
        assert!(shutdown.is_terminal());
        assert!(!shutdown.is_transient());

        let reentrant = TryPublishError::ReentrantPublish;
        assert!(!reentrant.is_terminal());
        assert!(!reentrant.is_transient());

        // The wrapped transient errors remain accessible as sources.
        assert!(std::error::Error::source(&full).is_some());
        assert!(std::error::Error::source(&contended).is_none());
        assert!(std::error::Error::source(&invalid).is_none());
        assert!(std::error::Error::source(&poisoned).is_none());

        // From conversions keep the legacy error types usable.
        let from_full: TryPublishError = RingBufferFull.into();
        assert!(matches!(from_full, TryPublishError::Full(_)));
        let from_missing: TryPublishError = MissingFreeSlots(2).into();
        assert!(matches!(from_missing, TryPublishError::MissingFreeSlots(_)));
    }

    #[test]
    fn test_producer_trait_methods() {
        let mut producer = create_test_producer();

        // Test all Producer trait methods

        // try_publish - publishes to sequence 0
        let result1 = Producer::try_publish(&mut producer, |event| {
            event.value = 1;
        });
        assert!(result1.is_ok());
        assert_eq!(result1.unwrap(), 0);

        // publish - publishes to sequence 1
        let _ = Producer::publish(&mut producer, |event| {
            event.value = 2;
        });
        assert_eq!(producer.current_sequence(), 1);

        // try_batch_publish - publishes 2 events to sequences 2, 3
        let result2 = Producer::try_batch_publish(&mut producer, 2, |iter| {
            for (i, event) in iter.enumerate() {
                event.value = i64::try_from(i + 10).expect("index fits in i64");
            }
        });
        assert!(result2.is_ok());
        assert_eq!(result2.unwrap(), 3); // End sequence of batch

        // batch_publish - publishes 1 event to sequence 4
        let _ = Producer::batch_publish(&mut producer, 1, |iter| {
            for event in iter {
                event.value = 99;
            }
        });

        // Should have published 5 events total (1 + 1 + 2 + 1) with final sequence 4
        assert_eq!(producer.current_sequence(), 4);
    }

    #[test]
    fn test_concurrent_access_safety() {
        // This test ensures that the producer can be safely used
        // even when the underlying structures are shared
        let factory = DefaultEventFactory::<TestEvent>::new();
        let ring_buffer =
            Arc::new(RingBuffer::new(64, factory).expect("Failed to create ring buffer"));
        let sequencer = SequencerEnum::Multi(Arc::new(MultiProducerSequencer::new(
            64,
            Arc::new(BusySpinWaitStrategy),
        )));

        let mut producer1 = SimpleProducer::new(ring_buffer.clone(), sequencer.clone());
        let mut producer2 = SimpleProducer::new(ring_buffer.clone(), sequencer.clone());

        // Both producers should be able to publish
        let result1 = producer1.try_publish(|event| event.value = 1);
        let result2 = producer2.try_publish(|event| event.value = 2);

        assert!(result1.is_ok());
        assert!(result2.is_ok());

        // Sequences should be different
        assert_ne!(result1.unwrap(), result2.unwrap());
    }

    #[test]
    fn test_large_batch_publish() {
        let factory = DefaultEventFactory::<TestEvent>::new();
        let ring_buffer =
            Arc::new(RingBuffer::new(1024, factory).expect("Failed to create ring buffer"));
        let sequencer = SequencerEnum::Single(Arc::new(unsafe {
            SingleProducerSequencer::new(1024, Arc::new(BusySpinWaitStrategy))
        }));
        let mut producer = SimpleProducer::new(ring_buffer, sequencer);

        // Test large batch
        let batch_size = 100;
        let result = producer.try_batch_publish(batch_size, |iter| {
            for (i, event) in iter.enumerate() {
                event.value = i64::try_from(i).expect("index fits in i64");
                event.data = format!("large_batch_{i}");
            }
        });

        assert!(result.is_ok());
        let expected = i64::try_from(batch_size - 1).expect("batch size fits in i64");
        assert_eq!(result.unwrap(), expected);
        assert_eq!(producer.current_sequence(), expected);
    }

    #[test]
    fn test_producer_sequence_consistency() {
        let mut producer = create_test_producer();
        let mut expected_sequence = 0i64;

        // Mix single and batch publishes
        for i in 0_i64..5 {
            if i % 2 == 0 {
                // Single publish
                let result = producer.try_publish(|event| {
                    event.value = i;
                });
                assert_eq!(result.unwrap(), expected_sequence);
                expected_sequence += 1;
            } else {
                // Batch publish of 2
                let result = producer.try_batch_publish(2, |iter| {
                    for (j, event) in iter.enumerate() {
                        event.value = i * 10 + i64::try_from(j).expect("index fits in i64");
                    }
                });
                expected_sequence += 1; // End sequence of batch
                assert_eq!(result.unwrap(), expected_sequence);
                expected_sequence += 1; // Prepare for next
            }
        }

        assert_eq!(producer.current_sequence(), expected_sequence - 1);
    }
}
