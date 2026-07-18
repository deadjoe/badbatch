//! Event poller for user-owned consumer threads.
//!
//! Unlike managed [`crate::disruptor::builder`] consumers, an [`EventPoller`] does **not**
//! spawn a thread. The application drives polling (busy loop, dedicated runtime, etc.).
//! Inspired by disruptor-rs `EventPoller`.

use crate::disruptor::{
    sequence_barrier::{ProcessingSequenceBarrier, SequenceBarrier},
    DisruptorError, Result, RingBuffer, Sequence, WaitStrategy, INITIAL_CURSOR_VALUE,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Outcome of a non-blocking poll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Polling {
    /// No published events are available yet.
    Idle,
    /// Disruptor / poller has been shut down.
    Shutdown,
}

/// User-driven consumer that polls events from a ring buffer.
///
/// Register [`EventPoller::sequence`] with the sequencer as a gating sequence so
/// producers cannot overwrite unconsumed slots.
#[derive(Debug)]
pub struct EventPoller<T, W>
where
    T: Send + Sync,
    W: WaitStrategy + 'static,
{
    ring_buffer: Arc<RingBuffer<T>>,
    barrier: Arc<ProcessingSequenceBarrier<W>>,
    /// Consumer sequence published for producer backpressure (gating).
    sequence: Arc<Sequence>,
    next_sequence: i64,
    shutdown: Arc<AtomicBool>,
}

impl<T, W> EventPoller<T, W>
where
    T: Send + Sync,
    W: WaitStrategy + 'static,
{
    /// Create a poller bound to a ring buffer and monomorphized barrier.
    ///
    /// `shutdown` is observed on each poll; set it to stop the consumer loop.
    ///
    /// # Safety
    /// The poller takes `&mut` access to slots the barrier reports available.
    /// The caller must guarantee the topology invariant: the barrier's sequence
    /// progression only exposes slots that no other consumer accesses mutably
    /// and that the producer cannot rewrite before this poller's gating sequence
    /// (registered with the sequencer) advances past them. Prefer
    /// [`open_single_producer_poller`], which wires this correctly.
    #[must_use]
    pub unsafe fn new(
        ring_buffer: Arc<RingBuffer<T>>,
        barrier: Arc<ProcessingSequenceBarrier<W>>,
        shutdown: Arc<AtomicBool>,
    ) -> Self {
        Self {
            ring_buffer,
            barrier,
            sequence: Arc::new(Sequence::new(INITIAL_CURSOR_VALUE)),
            next_sequence: 0,
            shutdown,
        }
    }

    /// Sequence used for gating / backpressure. Must be registered with the sequencer.
    #[must_use]
    pub fn sequence(&self) -> Arc<Sequence> {
        Arc::clone(&self.sequence)
    }

    /// Current next sequence this poller will attempt to consume.
    #[must_use]
    pub fn next_sequence(&self) -> i64 {
        self.next_sequence
    }

    /// Signal this poller to stop (also wake barrier waiters via alert).
    pub fn halt(&self) {
        self.shutdown.store(true, Ordering::Release);
        self.barrier.alert();
    }

    /// Non-blocking attempt to claim a batch of available events.
    ///
    /// On success returns an [`EventBatch`] RAII guard. When the guard is dropped,
    /// the poller's gating sequence advances to the last available sequence in the batch.
    ///
    /// # Errors
    /// - [`Polling::Idle`] when nothing is published yet
    /// - [`Polling::Shutdown`] when halt was requested
    pub fn poll(&mut self) -> std::result::Result<EventBatch<'_, T, W>, Polling> {
        if self.shutdown.load(Ordering::Acquire) {
            return Err(Polling::Shutdown);
        }

        // Non-blocking: use a zero-timeout wait path by checking cursor/deps via barrier.
        // ProcessingSequenceBarrier::wait_for may block for BlockingWaitStrategy; for
        // BusySpin/Yielding it spins. Prefer try path via available check first.
        let available = match self
            .barrier
            .wait_for_with_timeout(self.next_sequence, std::time::Duration::ZERO)
        {
            Ok(seq) => seq,
            Err(DisruptorError::Alert | DisruptorError::Shutdown) => {
                return Err(Polling::Shutdown);
            }
            Err(DisruptorError::Timeout | _) => return Err(Polling::Idle),
        };

        if available < self.next_sequence {
            return Err(Polling::Idle);
        }

        let from = self.next_sequence;
        let to = available;
        Ok(EventBatch {
            poller: self,
            from,
            current: from,
            to,
            committed: false,
        })
    }

    /// Poll with a maximum batch size (inclusive range length cap).
    pub fn take(
        &mut self,
        max_events: usize,
    ) -> std::result::Result<EventBatch<'_, T, W>, Polling> {
        let mut batch = self.poll()?;
        if max_events == 0 {
            batch.to = batch.from - 1;
            return Ok(batch);
        }
        let max_hi = batch
            .from
            .saturating_add(i64::try_from(max_events).unwrap_or(i64::MAX) - 1);
        if batch.to > max_hi {
            batch.to = max_hi;
        }
        Ok(batch)
    }
}

/// RAII batch of events available to a poller.
///
/// Iterate with [`EventBatch::next_mut`] (mutable) or as shared references via
/// [`EventBatch::get`]. Dropping the batch commits progress up to `to`.
pub struct EventBatch<'p, T, W>
where
    T: Send + Sync,
    W: WaitStrategy + 'static,
{
    poller: &'p mut EventPoller<T, W>,
    from: i64,
    current: i64,
    to: i64,
    committed: bool,
}

impl<T, W> EventBatch<'_, T, W>
where
    T: Send + Sync,
    W: WaitStrategy + 'static,
{
    /// Inclusive start sequence of this batch.
    #[must_use]
    pub fn from_sequence(&self) -> i64 {
        self.from
    }

    /// Inclusive end sequence of this batch.
    #[must_use]
    pub fn to_sequence(&self) -> i64 {
        self.to
    }

    /// Number of events in the batch.
    #[must_use]
    pub fn len(&self) -> usize {
        if self.to < self.from {
            0
        } else {
            usize::try_from(self.to - self.from + 1).unwrap_or(0)
        }
    }

    /// Whether the batch is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Immutable access to the event at `sequence` (must be within `[from, to]`).
    #[must_use]
    pub fn get(&self, sequence: i64) -> Option<&T> {
        if sequence < self.from || sequence > self.to {
            return None;
        }
        // SAFETY: [from, to] is published (poll observed it) and the poller's
        // gating sequence is not advanced until this batch commits, so the
        // producer cannot wrap into the range while this borrow is alive.
        Some(unsafe { self.poller.ring_buffer.get(sequence) })
    }

    /// Take the next event mutably. Returns `None` when the batch is exhausted.
    pub fn next_mut(&mut self) -> Option<(i64, &mut T)> {
        if self.current > self.to {
            return None;
        }
        let seq = self.current;
        self.current += 1;
        // SAFETY: poller has exclusive ownership of sequences [from, to] until drop
        // (single poller / registered gating). Same contract as BatchEventProcessor.
        let event = unsafe { &mut *self.poller.ring_buffer.get_mut_unchecked(seq) };
        Some((seq, event))
    }

    /// Commit progress early without dropping. Subsequent drop is a no-op for gating.
    pub fn commit(mut self) {
        self.commit_inner();
    }

    fn commit_inner(&mut self) {
        if self.committed {
            return;
        }
        self.committed = true;
        if self.to >= self.from {
            self.poller.sequence.set(self.to);
            self.poller.next_sequence = self.to + 1;
        }
    }
}

impl<T, W> Drop for EventBatch<'_, T, W>
where
    T: Send + Sync,
    W: WaitStrategy + 'static,
{
    fn drop(&mut self) {
        self.commit_inner();
    }
}

/// Bundle returned by [`open_single_producer_poller`].
pub type SingleProducerPollerBundle<E, W> = (
    crate::disruptor::producer::SimpleProducer<E, W>,
    EventPoller<E, W>,
    Arc<AtomicBool>,
);

/// Convenience: single-producer ring + poller for user-owned threads.
///
/// Returns `(producer, poller, shutdown_flag)`. Register is automatic (poller sequence
/// is added as a gating sequence). Set `shutdown_flag` or call [`EventPoller::halt`]
/// to stop polling.
pub fn open_single_producer_poller<E, F, W>(
    buffer_size: usize,
    event_factory: F,
    wait_strategy: W,
) -> Result<SingleProducerPollerBundle<E, W>>
where
    E: Send + Sync + 'static,
    F: crate::disruptor::EventFactory<E>,
    W: WaitStrategy + 'static,
{
    use crate::disruptor::{
        producer::SimpleProducer,
        sequencer::{SequencerEnum, SingleProducerSequencer},
        Sequencer,
    };

    let ring_buffer = Arc::new(RingBuffer::new(buffer_size, event_factory)?);
    let wait = Arc::new(wait_strategy);
    // SAFETY: this bundle hands out exactly one producer handle (not Clone),
    // so the sequencer's claim methods have a single driving thread.
    let sequencer =
        Arc::new(unsafe { SingleProducerSequencer::new(buffer_size, Arc::clone(&wait)) });
    let seq_enum = SequencerEnum::Single(Arc::clone(&sequencer));
    let shutdown = Arc::new(AtomicBool::new(false));

    let barrier = Arc::new(ProcessingSequenceBarrier::new(
        sequencer.get_cursor(),
        Arc::clone(&wait),
        Vec::new(),
        seq_enum.clone(),
    ));

    // SAFETY: fresh ring + fresh single sequencer; this poller is the only
    // consumer, and its sequence is registered as the sole gating sequence
    // below, so the producer cannot wrap uncommitted slots.
    let poller =
        unsafe { EventPoller::new(Arc::clone(&ring_buffer), barrier, Arc::clone(&shutdown)) };
    sequencer.add_gating_sequences(&[poller.sequence()]);

    let producer = SimpleProducer::new(ring_buffer, seq_enum);
    Ok((producer, poller, shutdown))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disruptor::{producer::Producer, BusySpinWaitStrategy, DefaultEventFactory};

    #[derive(Debug, Default, Clone, Copy)]
    struct Ev {
        v: i64,
    }

    #[test]
    fn poller_reads_published_events_and_advances_gating() {
        let factory = DefaultEventFactory::<Ev>::new();
        let (mut producer, mut poller, shutdown) =
            open_single_producer_poller(8, factory, BusySpinWaitStrategy).unwrap();

        assert_eq!(poller.poll().err(), Some(Polling::Idle));

        producer.publish(|e| e.v = 7);

        {
            let mut batch = poller.poll().expect("events");
            assert_eq!(batch.len(), 1);
            let (s, e) = batch.next_mut().unwrap();
            assert_eq!(s, 0);
            assert_eq!(e.v, 7);
        }

        assert_eq!(poller.sequence().get(), 0);
        assert_eq!(poller.next_sequence(), 1);

        shutdown.store(true, Ordering::Release);
        poller.halt();
        assert_eq!(poller.poll().err(), Some(Polling::Shutdown));
    }

    #[test]
    fn simple_busy_spin_works_with_builder() {
        use crate::disruptor::{build_single_producer, simple_wait_strategy::BusySpin};

        #[derive(Default)]
        struct E {
            x: i64,
        }

        let mut d = build_single_producer(8, E::default, BusySpin)
            .handle_events_with(|e: &mut E, _, _| {
                e.x += 1;
            })
            .build();
        d.publish(|e| e.x = 1);
        d.shutdown();
    }
}
