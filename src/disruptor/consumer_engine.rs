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
    DisruptorError, ErrorDecision, EventHandler, ExceptionHandler, FailurePhase, FailureRecord,
    Result, RingBuffer, Sequence, WaitStrategy, INITIAL_CURSOR_VALUE,
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

#[derive(Clone, Copy)]
pub(crate) struct ConsumerContext<'a> {
    thread_name: &'a str,
    stage_index: Option<usize>,
}

impl<'a> ConsumerContext<'a> {
    pub(crate) const fn for_stage(thread_name: &'a str, stage_index: usize) -> Self {
        Self {
            thread_name,
            stage_index: Some(stage_index),
        }
    }

    const fn without_stage(thread_name: &'a str) -> Self {
        Self {
            thread_name,
            stage_index: None,
        }
    }
}

fn consumer_failure(
    phase: FailurePhase,
    message: impl Into<String>,
    context: ConsumerContext<'_>,
    sequence: Option<i64>,
) -> FailureRecord {
    let mut failure = FailureRecord::new(phase, message).with_thread_name(context.thread_name);
    if let Some(stage_index) = context.stage_index {
        failure = failure.with_stage_index(stage_index);
    }
    if let Some(sequence) = sequence {
        failure = failure.with_sequence(sequence);
    }
    failure
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
    run_sequential_batch_loop_impl(
        ring_buffer,
        sequence_barrier,
        event_handler,
        consumer_sequence,
        stop,
        ConsumerContext::without_stage(thread_name),
        None,
    );
}

/// Builder-only variant that preserves the pipeline stage in failure records.
pub(crate) fn run_sequential_batch_loop_for_stage<E, H, W>(
    ring_buffer: &RingBuffer<E>,
    sequence_barrier: &ProcessingSequenceBarrier<W>,
    event_handler: &mut H,
    consumer_sequence: &Sequence,
    stop: StopFlag<'_>,
    context: ConsumerContext<'_>,
) where
    E: Send + Sync,
    H: EventHandler<E>,
    W: WaitStrategy + 'static,
{
    run_sequential_batch_loop_impl(
        ring_buffer,
        sequence_barrier,
        event_handler,
        consumer_sequence,
        stop,
        context,
        None,
    );
}

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
    run_sequential_batch_loop_impl(
        ring_buffer,
        sequence_barrier,
        event_handler,
        consumer_sequence,
        stop,
        ConsumerContext::without_stage(thread_name),
        exception_handler,
    );
}

fn run_sequential_batch_loop_impl<E, H, W>(
    ring_buffer: &RingBuffer<E>,
    sequence_barrier: &ProcessingSequenceBarrier<W>,
    event_handler: &mut H,
    consumer_sequence: &Sequence,
    stop: StopFlag<'_>,
    context: ConsumerContext<'_>,
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
                if let Err(e) = event_handler.on_batch_start(batch_size, queue_depth) {
                    // Fatal: nothing of this batch was processed; freeze the
                    // sequence and poison the producers (2026-07-18 audit —
                    // this Result used to be silently discarded).
                    sequence_barrier.poison_with_failure(&consumer_failure(
                        FailurePhase::BatchStart,
                        e.to_string(),
                        context,
                        Some(next_sequence),
                    ));
                    return;
                }

                while next_sequence <= available_sequence {
                    let end_of_batch = next_sequence == available_sequence;

                    // SAFETY: exclusive ownership of this sequence range by topology.
                    let event = unsafe { &mut *ring_buffer.get_mut_unchecked(next_sequence) };

                    if let Err(e) = event_handler.on_event(event, next_sequence, end_of_batch) {
                        let error_message = e.to_string();
                        let decision = if let Some(eh) = exception_handler {
                            eh.handle_event_exception(e, next_sequence, event)
                        } else {
                            // No exception handler: LMAX-default fatal stop
                            // (used to silently skip; 2026-07-18 audit).
                            ErrorDecision::Stop
                        };

                        if decision == ErrorDecision::Stop {
                            // Freeze at the last fully processed event and fail
                            // the producers fast instead of stalling them.
                            consumer_sequence.set(next_sequence - 1);
                            sequence_barrier.poison_with_failure(&consumer_failure(
                                FailurePhase::EventProcessing,
                                error_message,
                                context,
                                Some(next_sequence),
                            ));
                            return;
                        }
                    }

                    next_sequence += 1;
                }

                consumer_sequence.set(available_sequence);
            }
            // Barrier alert / shutdown is terminal: re-waiting just spins on the
            // same Alert. Also exit when the stop flag flipped mid-wait.
            Err(e)
                if matches!(e, DisruptorError::Alert | DisruptorError::Shutdown)
                    || !stop.should_continue() =>
            {
                break;
            }
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
    run_work_processor_loop_impl(
        ring_buffer,
        sequence_barrier,
        event_handler,
        consumer_sequence,
        work_sequence,
        stop,
        ConsumerContext::without_stage(thread_name),
    );
}

/// Builder-only WorkerPool variant that preserves the pipeline stage.
pub(crate) fn run_work_processor_loop_for_stage<E, H, W>(
    ring_buffer: &RingBuffer<E>,
    sequence_barrier: &ProcessingSequenceBarrier<W>,
    event_handler: &mut H,
    consumer_sequence: &Sequence,
    work_sequence: &CachePadded<AtomicI64>,
    stop: StopFlag<'_>,
    context: ConsumerContext<'_>,
) where
    E: Send + Sync,
    H: EventHandler<E>,
    W: WaitStrategy + 'static,
{
    run_work_processor_loop_impl(
        ring_buffer,
        sequence_barrier,
        event_handler,
        consumer_sequence,
        work_sequence,
        stop,
        context,
    );
}

fn run_work_processor_loop_impl<E, H, W>(
    ring_buffer: &RingBuffer<E>,
    sequence_barrier: &ProcessingSequenceBarrier<W>,
    event_handler: &mut H,
    consumer_sequence: &Sequence,
    work_sequence: &CachePadded<AtomicI64>,
    stop: StopFlag<'_>,
    context: ConsumerContext<'_>,
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
                Err(e)
                    if matches!(e, DisruptorError::Alert | DisruptorError::Shutdown)
                        || !stop.should_continue() =>
                {
                    break 'work;
                }
                Err(_) => std::hint::spin_loop(),
            }
        }

        let end_of_batch = claimed == cached_available;
        if let Err(e) = event_handler.on_batch_start(1, cached_available - claimed + 1) {
            // Fatal (2026-07-18 audit): this Result used to be discarded.
            sequence_barrier.poison_with_failure(&consumer_failure(
                FailurePhase::BatchStart,
                e.to_string(),
                context,
                Some(claimed),
            ));
            break 'work;
        }

        // SAFETY: CAS claim grants exclusive ownership of `claimed`.
        let event = unsafe { &mut *ring_buffer.get_mut_unchecked(claimed) };

        if let Err(e) = event_handler.on_event(event, claimed, end_of_batch) {
            // WorkerPool has no per-worker exception handler; apply the
            // LMAX-default fatal policy (used to silently skip the event).
            // Note: `claimed` was already CAS-taken from the shared work
            // cursor, so this worker's sequence stays at its pre-claim value.
            sequence_barrier.poison_with_failure(&consumer_failure(
                FailurePhase::EventProcessing,
                e.to_string(),
                context,
                Some(claimed),
            ));
            break 'work;
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
    run_sequential_readonly_loop_impl(
        ring_buffer,
        sequence_barrier,
        on_event,
        consumer_sequence,
        stop,
        ConsumerContext::without_stage(thread_name),
    );
}

/// Builder-only readonly variant that preserves the pipeline stage.
pub(crate) fn run_sequential_readonly_loop_for_stage<E, F, W>(
    ring_buffer: &RingBuffer<E>,
    sequence_barrier: &ProcessingSequenceBarrier<W>,
    on_event: &mut F,
    consumer_sequence: &Sequence,
    stop: StopFlag<'_>,
    context: ConsumerContext<'_>,
) where
    E: Send + Sync,
    F: FnMut(&E, i64, bool) -> crate::disruptor::Result<()>,
    W: WaitStrategy + 'static,
{
    run_sequential_readonly_loop_impl(
        ring_buffer,
        sequence_barrier,
        on_event,
        consumer_sequence,
        stop,
        context,
    );
}

fn run_sequential_readonly_loop_impl<E, F, W>(
    ring_buffer: &RingBuffer<E>,
    sequence_barrier: &ProcessingSequenceBarrier<W>,
    on_event: &mut F,
    consumer_sequence: &Sequence,
    stop: StopFlag<'_>,
    context: ConsumerContext<'_>,
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
                        // LMAX-default fatal policy (used to silently skip).
                        consumer_sequence.set(next_sequence - 1);
                        sequence_barrier.poison_with_failure(&consumer_failure(
                            FailurePhase::EventProcessing,
                            e.to_string(),
                            context,
                            Some(next_sequence),
                        ));
                        return;
                    }
                    next_sequence += 1;
                }

                consumer_sequence.set(available_sequence);
            }
            Err(e)
                if matches!(e, DisruptorError::Alert | DisruptorError::Shutdown)
                    || !stop.should_continue() =>
            {
                break;
            }
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
