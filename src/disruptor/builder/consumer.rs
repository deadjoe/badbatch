//! Consumer thread handles, access kind, and unified spawn.

use super::core::set_thread_affinity;
use crate::disruptor::failure::panic_payload_message;
use crate::disruptor::{
    consumer_engine::{self, ConsumerContext, RunMode, StopFlag},
    sequence_barrier::ProcessingSequenceBarrier,
    EventHandler, FailurePhase, FailureRecord, RingBuffer, Sequence, WaitStrategy,
    INITIAL_CURSOR_VALUE,
};
use std::sync::{atomic::AtomicBool, Arc};
use std::thread::{self, JoinHandle};

/// Type-state: no consumers registered yet.
pub struct NoConsumers;

/// Type-state: at least one consumer registered.
pub struct HasConsumers;

/// How a consumer accesses ring slots on its stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConsumerAccess {
    /// `&mut E` — exclusive sequential or WorkerPool claim.
    Mutable,
    /// `&E` — broadcast fan-out (every consumer sees every sequence).
    Readonly,
}

/// Consumer thread handle for a managed Disruptor consumer.
///
/// # Drop behavior
///
/// Dropping a `Consumer` joins the underlying thread and **may block**. Prefer
/// [`crate::disruptor::builder::DisruptorHandle::shutdown`] for controlled teardown.
///
/// Not [`Clone`]: a join handle is unique ownership. Cloning would drop the handle
/// and produce a hollow shell — that API is intentionally absent.
#[derive(Debug)]
pub struct Consumer {
    pub(crate) join_handle: Option<JoinHandle<()>>,
    pub(crate) thread_name: String,
}

impl Consumer {
    /// Create a consumer handle from a spawned join handle and thread name.
    pub fn new(join_handle: JoinHandle<()>, thread_name: String) -> Self {
        Self {
            join_handle: Some(join_handle),
            thread_name,
        }
    }

    /// Join the consumer thread if still running.
    pub fn join(&mut self) -> std::thread::Result<()> {
        if let Some(handle) = self.join_handle.take() {
            handle.join()
        } else {
            Ok(())
        }
    }

    /// Thread name used when the consumer was spawned.
    pub fn thread_name(&self) -> &str {
        &self.thread_name
    }
}

impl Drop for Consumer {
    fn drop(&mut self) {
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.join();
        }
    }
}

/// Thread configuration for a single consumer spawn.
pub(crate) struct ConsumerThreadConfig {
    pub(crate) thread_name: Option<String>,
    pub(crate) cpu_affinity: Option<usize>,
    pub(crate) stage_index: usize,
    pub(crate) shutdown_flag: Arc<AtomicBool>,
    /// How this consumer runs (sequential / work-pool). Never optional magic.
    pub(crate) run_mode: RunMode,
}

pub(crate) type ConsumerStarter<E, W> = Box<
    dyn FnOnce(
            Arc<RingBuffer<E>>,
            Arc<ProcessingSequenceBarrier<W>>,
            ConsumerThreadConfig,
        ) -> (Arc<Sequence>, Consumer)
        + Send,
>;

/// One registered consumer before threads start.
pub struct ConsumerInfo<E, W>
where
    E: Send + Sync + 'static,
    W: WaitStrategy + 'static,
{
    pub(crate) starter: ConsumerStarter<E, W>,
    pub(crate) thread_name: Option<String>,
    pub(crate) cpu_affinity: Option<usize>,
    pub(crate) stage_index: usize,
    pub(crate) access: ConsumerAccess,
}

fn lifecycle_failure(
    phase: FailurePhase,
    message: impl Into<String>,
    thread_name: &str,
    stage_index: usize,
) -> FailureRecord {
    FailureRecord::new(phase, message)
        .with_thread_name(thread_name)
        .with_stage_index(stage_index)
}

/// Allocate sequence and spawn named thread; body receives sequence + resolved name.
fn start_consumer_thread<W, F>(
    config: ConsumerThreadConfig,
    default_name: &str,
    failure_barrier: Arc<ProcessingSequenceBarrier<W>>,
    body: F,
) -> (Arc<Sequence>, Consumer)
where
    W: WaitStrategy + 'static,
    F: FnOnce(Arc<Sequence>, String, usize, RunMode, Arc<AtomicBool>) + Send + 'static,
{
    let consumer_sequence = Arc::new(Sequence::new(INITIAL_CURSOR_VALUE));
    let sequence_for_thread = Arc::clone(&consumer_sequence);
    let thread_name = config
        .thread_name
        .unwrap_or_else(|| default_name.to_string());
    let thread_name_for_handle = thread_name.clone();
    let cpu_affinity = config.cpu_affinity;
    let stage_index = config.stage_index;
    let shutdown_flag = config.shutdown_flag;
    let run_mode = config.run_mode;

    let join_handle = thread::Builder::new()
        .name(thread_name.clone())
        .spawn(move || {
            if let Some(core_id) = cpu_affinity {
                if let Err(error) = set_thread_affinity(core_id) {
                    failure_barrier.poison_with_failure(&lifecycle_failure(
                        FailurePhase::ThreadAffinity,
                        format!("failed to pin to CPU core {core_id}: {error}"),
                        &thread_name,
                        stage_index,
                    ));
                    return;
                }
                log::debug!(
                    target: "badbatch::lifecycle",
                    "phase=thread_affinity thread={thread_name:?} stage={stage_index} core={core_id} decision=pinned"
                );
            }
            body(
                sequence_for_thread,
                thread_name,
                stage_index,
                run_mode,
                shutdown_flag,
            );
        })
        .expect("Failed to spawn consumer thread");

    (
        consumer_sequence,
        Consumer::new(join_handle, thread_name_for_handle),
    )
}

/// Mutable consumer (sequential or WorkerPool from `run_mode`).
pub(crate) fn start_mutable_consumer<E, H, W>(
    ring_buffer: Arc<RingBuffer<E>>,
    sequence_barrier: Arc<ProcessingSequenceBarrier<W>>,
    mut event_handler: H,
    config: ConsumerThreadConfig,
) -> (Arc<Sequence>, Consumer)
where
    E: Send + Sync + 'static,
    H: EventHandler<E> + Send + 'static,
    W: WaitStrategy + 'static,
{
    let failure_barrier = Arc::clone(&sequence_barrier);
    start_consumer_thread(
        config,
        "disruptor-consumer",
        failure_barrier,
        move |seq, thread_name, stage_index, run_mode, shutdown| {
            // A panicking handler kills this consumer thread; poison the
            // producers so blocking publishes fail fast instead of spinning
            // forever on a gating sequence that never advances (2026-07-18 audit).
            let barrier_for_poison = Arc::clone(&sequence_barrier);
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if let Err(e) = event_handler.on_start() {
                    sequence_barrier.poison_with_failure(&lifecycle_failure(
                        FailurePhase::HandlerStart,
                        e.to_string(),
                        &thread_name,
                        stage_index,
                    ));
                    return;
                }

                let stop = StopFlag::External(&shutdown);
                match &run_mode {
                    RunMode::WorkPool(work_seq) => {
                        consumer_engine::run_work_processor_loop_for_stage(
                            &ring_buffer,
                            &sequence_barrier,
                            &mut event_handler,
                            &seq,
                            work_seq,
                            stop,
                            ConsumerContext::for_stage(&thread_name, stage_index),
                        );
                    }
                    RunMode::Sequential => {
                        consumer_engine::run_sequential_batch_loop_for_stage(
                            &ring_buffer,
                            &sequence_barrier,
                            &mut event_handler,
                            &seq,
                            stop,
                            ConsumerContext::for_stage(&thread_name, stage_index),
                        );
                    }
                }

                if let Err(e) = event_handler.on_shutdown() {
                    sequence_barrier.record_failure(&lifecycle_failure(
                        FailurePhase::HandlerShutdown,
                        e.to_string(),
                        &thread_name,
                        stage_index,
                    ));
                }
            }));
            if let Err(payload) = result {
                barrier_for_poison.poison_with_failure(&lifecycle_failure(
                    FailurePhase::ConsumerPanic,
                    panic_payload_message(payload.as_ref()),
                    &thread_name,
                    stage_index,
                ));
                std::panic::resume_unwind(payload);
            }
        },
    )
}

/// Read-only fan-out consumer (always sequential broadcast).
pub(crate) fn start_readonly_consumer<E, F, W>(
    ring_buffer: Arc<RingBuffer<E>>,
    sequence_barrier: Arc<ProcessingSequenceBarrier<W>>,
    mut on_event: F,
    config: ConsumerThreadConfig,
) -> (Arc<Sequence>, Consumer)
where
    E: Send + Sync + 'static,
    F: FnMut(&E, i64, bool) -> crate::disruptor::Result<()> + Send + 'static,
    W: WaitStrategy + 'static,
{
    assert!(
        matches!(config.run_mode, RunMode::Sequential),
        "readonly fan-out must use RunMode::Sequential (got WorkPool — assembly bug)"
    );

    let failure_barrier = Arc::clone(&sequence_barrier);
    start_consumer_thread(
        config,
        "disruptor-fanout",
        failure_barrier,
        move |seq, thread_name, stage_index, _run_mode, shutdown| {
            // Same panic policy as mutable consumers: poison the producers so
            // they fail fast instead of spinning on a dead gating sequence.
            let barrier_for_poison = Arc::clone(&sequence_barrier);
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let stop = StopFlag::External(&shutdown);
                consumer_engine::run_sequential_readonly_loop_for_stage(
                    &ring_buffer,
                    &sequence_barrier,
                    &mut on_event,
                    &seq,
                    stop,
                    ConsumerContext::for_stage(&thread_name, stage_index),
                );
            }));
            if let Err(payload) = result {
                barrier_for_poison.poison_with_failure(&lifecycle_failure(
                    FailurePhase::ConsumerPanic,
                    panic_payload_message(payload.as_ref()),
                    &thread_name,
                    stage_index,
                ));
                std::panic::resume_unwind(payload);
            }
        },
    )
}

pub(crate) fn consumer_info_mutable<E, H, W>(
    handler: H,
    thread_name: Option<String>,
    cpu_affinity: Option<usize>,
    stage_index: usize,
) -> ConsumerInfo<E, W>
where
    E: Send + Sync + 'static,
    H: EventHandler<E> + Send + 'static,
    W: WaitStrategy + 'static,
{
    let starter: ConsumerStarter<E, W> = Box::new(move |ring_buffer, sequence_barrier, config| {
        start_mutable_consumer(ring_buffer, sequence_barrier, handler, config)
    });
    ConsumerInfo {
        starter,
        thread_name,
        cpu_affinity,
        stage_index,
        access: ConsumerAccess::Mutable,
    }
}

pub(crate) fn consumer_info_readonly<E, F, W>(
    on_event: F,
    thread_name: Option<String>,
    cpu_affinity: Option<usize>,
    stage_index: usize,
) -> ConsumerInfo<E, W>
where
    E: Send + Sync + 'static,
    F: FnMut(&E, i64, bool) -> crate::disruptor::Result<()> + Send + 'static,
    W: WaitStrategy + 'static,
{
    let starter: ConsumerStarter<E, W> = Box::new(move |ring_buffer, sequence_barrier, config| {
        start_readonly_consumer(ring_buffer, sequence_barrier, on_event, config)
    });
    ConsumerInfo {
        starter,
        thread_name,
        cpu_affinity,
        stage_index,
        access: ConsumerAccess::Readonly,
    }
}

/// Same-stage consumers must share one access kind.
pub(crate) fn assert_access_compatible<E, W>(
    consumers: &[ConsumerInfo<E, W>],
    stage_index: usize,
    access: ConsumerAccess,
) where
    E: Send + Sync + 'static,
    W: WaitStrategy + 'static,
{
    for c in consumers {
        assert!(
            !(c.stage_index == stage_index && c.access != access),
            "cannot mix mutable handlers (WorkerPool / &mut E) with read-only fan-out (&E) on the same stage (stage {stage_index})"
        );
    }
}

/// Build per-stage run modes: WorkerPool only for multi-mutable stages.
///
/// `stage_count` must equal `max(stage_index)+1` for the consumer list (or 0 if empty).
pub(crate) fn stage_run_modes<E, W>(
    consumers: &[ConsumerInfo<E, W>],
    stage_count: usize,
) -> Vec<RunMode>
where
    E: Send + Sync + 'static,
    W: WaitStrategy + 'static,
{
    if stage_count == 0 {
        return Vec::new();
    }
    let mut widths = vec![0_usize; stage_count];
    let mut access = vec![ConsumerAccess::Mutable; stage_count];
    for c in consumers {
        widths[c.stage_index] += 1;
        access[c.stage_index] = c.access;
    }
    widths
        .into_iter()
        .zip(access)
        .map(|(width, acc)| {
            if width > 1 && acc == ConsumerAccess::Mutable {
                RunMode::WorkPool(consumer_engine::new_work_sequence())
            } else {
                RunMode::Sequential
            }
        })
        .collect()
}
