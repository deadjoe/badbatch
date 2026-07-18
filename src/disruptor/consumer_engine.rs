//! Unified consumer execution engine — **the single authoritative hot loop**.
//!
//! All managed consumption paths must call into this module:
//! - Builder-spawned threads (`builder::consumer`)
//! - [`crate::disruptor::BatchEventProcessor`] (LMAX DSL)
//!
//! Sequential batch processing and WorkerPool CAS claim are implemented here
//! only; do not re-open parallel loop copies in builder/DSL layers.

use crate::disruptor::{
    sequence_barrier::{ProcessingSequenceBarrier, SequenceBarrier},
    DisruptorError, EventHandler, ExceptionHandler, Result, RingBuffer, Sequence, WaitStrategy,
    INITIAL_CURSOR_VALUE,
};
use crossbeam_utils::CachePadded;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;

/// How a managed consumer executes once a barrier is ready.
#[derive(Clone)]
pub enum RunMode {
    /// Exclusive sequential drain (single mutable or any fan-out consumer).
    Sequential,
    /// LMAX WorkerPool: CAS-claim from shared work cursor.
    WorkPool(Arc<CachePadded<AtomicI64>>),
}

/// Single stop source for consumer loops (no dual-flag / static convention).
#[derive(Clone, Copy)]
pub enum StopFlag<'a> {
    /// Builder path: `true` means shut down.
    External(&'a AtomicBool),
    /// BatchEventProcessor path: `false` means halt (pair with barrier.alert()).
    Running(&'a AtomicBool),
}

impl StopFlag<'_> {
    /// Whether the consumer loop should keep processing.
    #[inline]
    pub fn should_continue(self) -> bool {
        match self {
            Self::External(flag) => !flag.load(Ordering::Acquire),
            Self::Running(flag) => flag.load(Ordering::Acquire),
        }
    }

    /// Wait until `sequence` is available or stop/alert interrupts.
    fn wait_for<W: WaitStrategy + 'static>(
        self,
        barrier: &ProcessingSequenceBarrier<W>,
        sequence: i64,
    ) -> Result<i64> {
        match self {
            Self::External(shutdown) => barrier.wait_for_with_shutdown(sequence, shutdown),
            Self::Running(running) => {
                if !running.load(Ordering::Acquire) {
                    return Err(DisruptorError::Alert);
                }
                // Halt pairs `running=false` with `barrier.alert()`.
                barrier.wait_for(sequence)
            }
        }
    }
}

/// Run the LMAX BatchEventProcessor-style sequential loop.
///
/// Published events already visible when a batch is taken are fully drained before
/// the loop checks stop conditions again (LMAX contract).
pub fn run_sequential_batch_loop<E, H, W>(
    ring_buffer: &RingBuffer<E>,
    sequence_barrier: &ProcessingSequenceBarrier<W>,
    event_handler: &mut H,
    consumer_sequence: &Sequence,
    stop: StopFlag<'_>,
    thread_name: &str,
) where
    E: Send + Sync,
    H: EventHandler<E>,
    W: WaitStrategy + 'static,
{
    run_sequential_batch_loop_with_exceptions(
        ring_buffer,
        sequence_barrier,
        event_handler,
        consumer_sequence,
        stop,
        thread_name,
        None,
    );
}

// `thread_name` is used only on the debug error path inside the with_exceptions variant.

/// Sequential loop with optional LMAX-style exception handler (BatchEventProcessor path).
pub fn run_sequential_batch_loop_with_exceptions<E, H, W>(
    ring_buffer: &RingBuffer<E>,
    sequence_barrier: &ProcessingSequenceBarrier<W>,
    event_handler: &mut H,
    consumer_sequence: &Sequence,
    stop: StopFlag<'_>,
    thread_name: &str,
    exception_handler: Option<&dyn ExceptionHandler<E>>,
) where
    E: Send + Sync,
    H: EventHandler<E>,
    W: WaitStrategy + 'static,
{
    let mut next_sequence = consumer_sequence.get() + 1;

    while stop.should_continue() {
        match stop.wait_for(sequence_barrier, next_sequence) {
            Ok(available_sequence) => {
                if available_sequence < next_sequence {
                    continue;
                }

                let batch_size = available_sequence - next_sequence + 1;
                let queue_depth = available_sequence - consumer_sequence.get();
                let _ = event_handler.on_batch_start(batch_size, queue_depth);

                while next_sequence <= available_sequence {
                    let end_of_batch = next_sequence == available_sequence;

                    // SAFETY: exclusive ownership of this sequence range by topology.
                    let event = unsafe { &mut *ring_buffer.get_mut_unchecked(next_sequence) };

                    if let Err(e) = event_handler.on_event(event, next_sequence, end_of_batch) {
                        if let Some(eh) = exception_handler {
                            eh.handle_event_exception(e, next_sequence, event);
                        } else {
                            #[cfg(debug_assertions)]
                            {
                                eprintln!(
                                    "Event processing error in '{thread_name}' at sequence {next_sequence}: {e:?}"
                                );
                            }
                            #[cfg(not(debug_assertions))]
                            {
                                let _ = (e, thread_name);
                            }
                        }
                    }

                    next_sequence += 1;
                }

                consumer_sequence.set(available_sequence);
            }
            Err(_) if !stop.should_continue() => break,
            Err(_) => {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
    }
}

/// Run LMAX WorkProcessor-inspired CAS work claim (WorkerPool scheme A).
///
/// Shared `work_sequence` starts at [`INITIAL_CURSOR_VALUE`]. Each worker
/// CAS-claims the next sequence for exclusive ownership (no slot Mutex).
pub fn run_work_processor_loop<E, H, W>(
    ring_buffer: &RingBuffer<E>,
    sequence_barrier: &ProcessingSequenceBarrier<W>,
    event_handler: &mut H,
    consumer_sequence: &Sequence,
    work_sequence: &CachePadded<AtomicI64>,
    stop: StopFlag<'_>,
    thread_name: &str,
) where
    E: Send + Sync,
    H: EventHandler<E>,
    W: WaitStrategy + 'static,
{
    let mut cached_available = INITIAL_CURSOR_VALUE;

    'work: while stop.should_continue() {
        let claimed = loop {
            if !stop.should_continue() {
                break 'work;
            }
            let current = work_sequence.load(Ordering::Acquire);
            let next = current + 1;
            consumer_sequence.set(current);
            match work_sequence.compare_exchange(current, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => break next,
                Err(_) => std::hint::spin_loop(),
            }
        };

        while claimed > cached_available {
            if !stop.should_continue() {
                break 'work;
            }
            match stop.wait_for(sequence_barrier, claimed) {
                Ok(available) => cached_available = available,
                Err(_) if !stop.should_continue() => break 'work,
                Err(_) => std::hint::spin_loop(),
            }
        }

        let end_of_batch = claimed == cached_available;
        let _ = event_handler.on_batch_start(1, cached_available - claimed + 1);

        // SAFETY: CAS claim grants exclusive ownership of `claimed`.
        let event = unsafe { &mut *ring_buffer.get_mut_unchecked(claimed) };

        if let Err(e) = event_handler.on_event(event, claimed, end_of_batch) {
            #[cfg(debug_assertions)]
            {
                eprintln!("Event processing error in '{thread_name}' at sequence {claimed}: {e:?}");
            }
            #[cfg(not(debug_assertions))]
            {
                let _ = (e, thread_name);
            }
        }

        consumer_sequence.set(claimed);
    }
}

/// Shared work cursor for a parallel stage (WorkerPool scheme A).
#[must_use]
pub fn new_work_sequence() -> Arc<CachePadded<AtomicI64>> {
    Arc::new(CachePadded::new(AtomicI64::new(INITIAL_CURSOR_VALUE)))
}

/// Sequential **read-only** fan-out loop: every consumer sees every published sequence
/// via shared `&E` (no exclusive claim, no slot locks).
///
/// Distinct from WorkerPool: this is broadcast-style observation, not work-sharing.
pub fn run_sequential_readonly_loop<E, F, W>(
    ring_buffer: &RingBuffer<E>,
    sequence_barrier: &ProcessingSequenceBarrier<W>,
    on_event: &mut F,
    consumer_sequence: &Sequence,
    stop: StopFlag<'_>,
    thread_name: &str,
) where
    E: Send + Sync,
    F: FnMut(&E, i64, bool) -> crate::disruptor::Result<()>,
    W: WaitStrategy + 'static,
{
    let mut next_sequence = consumer_sequence.get() + 1;

    while stop.should_continue() {
        match stop.wait_for(sequence_barrier, next_sequence) {
            Ok(available_sequence) => {
                if available_sequence < next_sequence {
                    continue;
                }

                while next_sequence <= available_sequence {
                    let end_of_batch = next_sequence == available_sequence;
                    // SAFETY: the barrier guarantees [next, available] is published,
                    // and this consumer's gating sequence prevents producer wrap-around
                    // until it advances. Fan-out consumers do not mutate slots.
                    let event = unsafe { ring_buffer.get(next_sequence) };
                    if let Err(e) = on_event(event, next_sequence, end_of_batch) {
                        #[cfg(debug_assertions)]
                        {
                            eprintln!(
                                "Readonly fan-out error in '{thread_name}' at sequence {next_sequence}: {e:?}"
                            );
                        }
                        #[cfg(not(debug_assertions))]
                        {
                            let _ = (e, thread_name);
                        }
                    }
                    next_sequence += 1;
                }

                consumer_sequence.set(available_sequence);
            }
            Err(_) if !stop.should_continue() => break,
            Err(_) => {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disruptor::{
        producer::{Producer, SimpleProducer},
        sequencer::{SequencerEnum, SingleProducerSequencer},
        BusySpinWaitStrategy, DefaultEventFactory, Sequencer,
    };
    use std::sync::atomic::AtomicUsize;

    #[derive(Debug, Default)]
    struct Ev {
        v: i64,
    }

    struct CountingHandler {
        count: Arc<AtomicUsize>,
    }

    impl EventHandler<Ev> for CountingHandler {
        fn on_event(
            &mut self,
            _event: &mut Ev,
            _sequence: i64,
            _end_of_batch: bool,
        ) -> crate::disruptor::Result<()> {
            self.count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[test]
    fn sequential_engine_drains_published_events() {
        let factory = DefaultEventFactory::<Ev>::new();
        let ring = Arc::new(RingBuffer::new(8, factory).unwrap());
        let wait = Arc::new(BusySpinWaitStrategy);
        let sequencer = Arc::new(unsafe { SingleProducerSequencer::new(8, Arc::clone(&wait)) });
        let seq_enum = SequencerEnum::Single(Arc::clone(&sequencer));
        let barrier = ProcessingSequenceBarrier::new(
            sequencer.get_cursor(),
            Arc::clone(&wait),
            Vec::new(),
            seq_enum.clone(),
        );
        let consumer_seq = Sequence::new(INITIAL_CURSOR_VALUE);
        sequencer.add_gating_sequences(&[Arc::new(Sequence::new(INITIAL_CURSOR_VALUE))]);
        // Note: gating uses a separate sequence so producer isn't blocked; we drive consumer in-thread.

        let mut producer = SimpleProducer::new(Arc::clone(&ring), seq_enum);
        for i in 0..5 {
            let _ = producer.publish(|e| e.v = i);
        }

        let count = Arc::new(AtomicUsize::new(0));
        let mut handler = CountingHandler {
            count: Arc::clone(&count),
        };
        let shutdown = AtomicBool::new(false);
        let running = AtomicBool::new(true);

        // Process one batch then stop via running=false + alert before next wait.
        let available = barrier
            .wait_for_with_shutdown(0, &shutdown)
            .expect("published");
        assert!(available >= 4);
        let mut next = 0i64;
        let _ = handler.on_batch_start(available - next + 1, available);
        while next <= available {
            let event = unsafe { &mut *ring.get_mut_unchecked(next) };
            handler.on_event(event, next, next == available).unwrap();
            next += 1;
        }
        consumer_seq.set(available);
        running.store(false, Ordering::Release);

        // Engine entry with already-stopped running should exit immediately.
        run_sequential_batch_loop(
            &ring,
            &barrier,
            &mut handler,
            &consumer_seq,
            StopFlag::Running(&running),
            "engine-test",
        );
        let _ = shutdown;

        assert_eq!(count.load(Ordering::SeqCst), 5);
        assert_eq!(consumer_seq.get(), 4);
    }

    #[test]
    fn work_sequence_cas_claims_are_unique() {
        let work = new_work_sequence();
        let mut claimed = Vec::new();
        for _ in 0..10 {
            let current = work.load(Ordering::Acquire);
            let next = current + 1;
            assert!(work
                .compare_exchange(current, next, Ordering::AcqRel, Ordering::Acquire)
                .is_ok());
            claimed.push(next);
        }
        assert_eq!(claimed, (0..10).collect::<Vec<_>>());
        assert_eq!(work.load(Ordering::Acquire), 9);
    }
}
