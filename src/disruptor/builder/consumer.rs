//! Consumer thread handles, access kind, and unified spawn.

use super::core::set_thread_affinity;
use crate::disruptor::{
    consumer_engine::{self, LoopControl, RunMode, StopFlag},
    sequence_barrier::ProcessingSequenceBarrier,
    EventHandler, RingBuffer, Sequence, WaitStrategy, INITIAL_CURSOR_VALUE,
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
#[derive(Debug)]
pub struct Consumer {
    pub(crate) join_handle: Option<JoinHandle<()>>,
    pub(crate) thread_name: String,
}

impl Clone for Consumer {
    fn clone(&self) -> Self {
        Self {
            join_handle: None,
            thread_name: self.thread_name.clone(),
        }
    }
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

/// Unified spawn: affinity + name + body.
fn spawn_named(
    thread_name: String,
    cpu_affinity: Option<usize>,
    body: impl FnOnce() + Send + 'static,
) -> JoinHandle<()> {
    thread::Builder::new()
        .name(thread_name.clone())
        .spawn(move || {
            if let Some(core_id) = cpu_affinity {
                if let Err(e) = set_thread_affinity(core_id) {
                    #[cfg(debug_assertions)]
                    eprintln!(
                        "Warning: Failed to set CPU affinity for '{thread_name}' to core {core_id}: {e}"
                    );
                    #[cfg(not(debug_assertions))]
                    let _ = e;
                }
            }
            body();
        })
        .expect("Failed to spawn consumer thread")
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
    let consumer_sequence = Arc::new(Sequence::new(INITIAL_CURSOR_VALUE));
    let consumer_sequence_clone = Arc::clone(&consumer_sequence);
    let thread_name = config
        .thread_name
        .unwrap_or_else(|| "disruptor-consumer".to_string());
    let thread_name_clone = thread_name.clone();
    let cpu_affinity = config.cpu_affinity;
    let shutdown_flag = config.shutdown_flag;
    let run_mode = config.run_mode;

    let join_handle = spawn_named(thread_name.clone(), cpu_affinity, move || {
        if let Err(e) = event_handler.on_start() {
            crate::internal_error!("EventHandler on_start failed in '{thread_name}': {e:?}");
        }

        let control = LoopControl {
            stop: StopFlag::External(&shutdown_flag),
        };

        match &run_mode {
            RunMode::WorkPool(work_seq) => {
                consumer_engine::run_work_processor_loop(
                    &ring_buffer,
                    &sequence_barrier,
                    &mut event_handler,
                    &consumer_sequence_clone,
                    work_seq,
                    control,
                    &thread_name,
                );
            }
            RunMode::Sequential => {
                consumer_engine::run_sequential_batch_loop(
                    &ring_buffer,
                    &sequence_barrier,
                    &mut event_handler,
                    &consumer_sequence_clone,
                    control,
                    &thread_name,
                );
            }
        }

        if let Err(e) = event_handler.on_shutdown() {
            crate::internal_error!("EventHandler on_shutdown failed in '{thread_name}': {e:?}");
        }
    });

    (
        consumer_sequence,
        Consumer::new(join_handle, thread_name_clone),
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
    debug_assert!(
        matches!(config.run_mode, RunMode::Sequential),
        "readonly fan-out must use RunMode::Sequential"
    );

    let consumer_sequence = Arc::new(Sequence::new(INITIAL_CURSOR_VALUE));
    let consumer_sequence_clone = Arc::clone(&consumer_sequence);
    let thread_name = config
        .thread_name
        .unwrap_or_else(|| "disruptor-fanout".to_string());
    let thread_name_clone = thread_name.clone();
    let cpu_affinity = config.cpu_affinity;
    let shutdown_flag = config.shutdown_flag;

    let join_handle = spawn_named(thread_name.clone(), cpu_affinity, move || {
        let control = LoopControl {
            stop: StopFlag::External(&shutdown_flag),
        };
        consumer_engine::run_sequential_readonly_loop(
            &ring_buffer,
            &sequence_barrier,
            &mut on_event,
            &consumer_sequence_clone,
            control,
            &thread_name,
        );
    });

    (
        consumer_sequence,
        Consumer::new(join_handle, thread_name_clone),
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
pub(crate) fn stage_run_modes<E, W>(
    consumers: &[ConsumerInfo<E, W>],
    stage_count: usize,
) -> Vec<RunMode>
where
    E: Send + Sync + 'static,
    W: WaitStrategy + 'static,
{
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
