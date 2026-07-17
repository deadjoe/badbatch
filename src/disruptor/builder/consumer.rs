//! Consumer thread handles and spawn helpers.

use super::core::set_thread_affinity;
use crate::disruptor::{
    consumer_engine::{self, LoopControl},
    sequence_barrier::ProcessingSequenceBarrier,
    EventHandler, RingBuffer, Sequence, WaitStrategy, INITIAL_CURSOR_VALUE,
};
use crossbeam_utils::CachePadded;
use std::sync::{
    atomic::{AtomicBool, AtomicI64},
    Arc,
};
use std::thread::{self, JoinHandle};

/// State marker: No consumers added yet
pub struct NoConsumers;

/// State marker: Has consumers
pub struct HasConsumers;
/// Consumer thread handle
///
/// This structure manages a consumer thread and provides methods to control its lifecycle.
/// It's inspired by the Consumer structure in disruptor-rs.
#[derive(Debug)]
/// A consumer thread handle in the Disruptor system.
///
/// # Drop Behavior Warning
///
/// **Important**: When a `Consumer` is dropped, its `Drop` implementation will
/// automatically call `join()` on the underlying thread, which **may block**
/// until the consumer thread completes.
///
/// **Recommendation**: For better control and to avoid unexpected blocking,
/// explicitly call `DisruptorHandle::shutdown()` before dropping the disruptor
/// system. This ensures all consumer threads are properly terminated in a
/// controlled manner.
///
/// # Example
/// ```rust,no_run
/// # use badbatch::disruptor::{build_single_producer, BusySpinWaitStrategy};
/// # #[derive(Default)]
/// # struct TestEvent { value: i32 }
/// // Build a single-producer disruptor with a simple event handler
/// let mut disruptor = build_single_producer(1024, TestEvent::default, BusySpinWaitStrategy)
///     .handle_events_with(|_event: &mut TestEvent, _seq, _end_of_batch| {})
///     .build();
///
/// // Do some work...
///
/// // Explicitly shutdown before drop to avoid blocking
/// disruptor.shutdown(); // Recommended!
/// // Now disruptor can be safely dropped without blocking
/// ```
pub struct Consumer {
    /// Handle to the consumer thread
    pub(crate) join_handle: Option<JoinHandle<()>>,
    /// Thread name for debugging
    pub(crate) thread_name: String,
}

impl Clone for Consumer {
    /// Creates a clone of this consumer
    ///
    /// Note: The cloned consumer will have its `join_handle` set to `None`
    /// as thread handles cannot be cloned. This means you can only join the
    /// original consumer thread once.
    fn clone(&self) -> Self {
        Self {
            join_handle: None, // JoinHandle cannot be cloned
            thread_name: self.thread_name.clone(),
        }
    }
}

impl Consumer {
    /// Create a new consumer with a thread handle
    pub fn new(join_handle: JoinHandle<()>, thread_name: String) -> Self {
        Self {
            join_handle: Some(join_handle),
            thread_name,
        }
    }

    /// Join the consumer thread, waiting for it to complete
    pub fn join(&mut self) -> std::thread::Result<()> {
        if let Some(handle) = self.join_handle.take() {
            handle.join()
        } else {
            Ok(())
        }
    }

    /// Get the thread name
    pub fn thread_name(&self) -> &str {
        &self.thread_name
    }
}

impl Drop for Consumer {
    /// Drop implementation that automatically joins the consumer thread.
    ///
    /// **Warning**: This may block until the consumer thread completes!
    /// For better control, use `DisruptorHandle::shutdown()` explicitly.
    fn drop(&mut self) {
        if let Some(handle) = self.join_handle.take() {
            // Note: This join() call may block until the consumer thread terminates.
            // Consider calling shutdown() explicitly for better control over timing.
            let _ = handle.join();
        }
    }
}

pub(crate) struct ConsumerThreadConfig {
    pub(crate) thread_name: Option<String>,
    pub(crate) cpu_affinity: Option<usize>,
    pub(crate) shutdown_flag: Arc<AtomicBool>,
    /// Shared work-sequence cursor for LMAX WorkerPool scheme A (parallel stages only).
    /// Workers CAS-claim the next sequence for exclusive ownership.
    pub(crate) work_sequence: Option<Arc<CachePadded<AtomicI64>>>,
}

/// Start a consumer thread for processing events.
///
/// Single-consumer stages use the unified sequential batch engine.
/// Parallel same-stage consumers (width > 1) use the unified WorkerPool engine.
pub(crate) fn start_consumer_thread<E, H, W>(
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
    let consumer_sequence_clone = consumer_sequence.clone();

    let ConsumerThreadConfig {
        thread_name,
        cpu_affinity,
        shutdown_flag,
        work_sequence,
    } = config;

    let thread_name = thread_name.unwrap_or_else(|| "disruptor-consumer".to_string());
    let thread_name_clone = thread_name.clone();
    let is_work_processor = work_sequence.is_some();

    let join_handle = thread::Builder::new()
        .name(thread_name.clone())
        .spawn(move || {
            if let Some(core_id) = cpu_affinity {
                if let Err(e) = set_thread_affinity(core_id) {
                    #[cfg(debug_assertions)]
                    eprintln!(
                        "Warning: Failed to set CPU affinity for thread '{thread_name}' to core {core_id}: {e}"
                    );
                    #[cfg(not(debug_assertions))]
                    let _ = e;
                }
            }

            if let Err(e) = event_handler.on_start() {
                crate::internal_error!(
                    "EventHandler on_start failed in thread '{thread_name}': {e:?}"
                );
            }

            let control = LoopControl {
                shutdown: &shutdown_flag,
                running: None,
            };

            if is_work_processor {
                let work_seq = work_sequence.expect("work processor requires work_sequence");
                consumer_engine::run_work_processor_loop(
                    &ring_buffer,
                    &sequence_barrier,
                    &mut event_handler,
                    &consumer_sequence_clone,
                    &work_seq,
                    control,
                    &thread_name,
                );
            } else {
                consumer_engine::run_sequential_batch_loop(
                    &ring_buffer,
                    &sequence_barrier,
                    &mut event_handler,
                    &consumer_sequence_clone,
                    control,
                    &thread_name,
                );
            }

            if let Err(e) = event_handler.on_shutdown() {
                crate::internal_error!(
                    "EventHandler on_shutdown failed in thread '{thread_name}': {e:?}"
                );
            }
        })
        .expect("Failed to spawn consumer thread");

    let consumer = Consumer::new(join_handle, thread_name_clone);
    (consumer_sequence, consumer)
}

pub(crate) type ConsumerStarter<E, W> = Box<
    dyn FnOnce(
            Arc<RingBuffer<E>>,
            Arc<ProcessingSequenceBarrier<W>>,
            ConsumerThreadConfig,
        ) -> (Arc<Sequence>, Consumer)
        + Send,
>;

/// Consumer information for the builder
pub struct ConsumerInfo<E, W>
where
    E: Send + Sync + 'static,
    W: WaitStrategy + 'static,
{
    pub(crate) starter: ConsumerStarter<E, W>,
    pub(crate) thread_name: Option<String>,
    pub(crate) cpu_affinity: Option<usize>,
    /// Logical stage index within the processing graph.
    pub(crate) stage_index: usize,
    /// `true` = read-only fan-out (`&E`); `false` = mutable handler / WorkerPool.
    pub(crate) readonly: bool,
}

pub(crate) fn consumer_info_from_handler<E, H, W>(
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
        start_consumer_thread(ring_buffer, sequence_barrier, handler, config)
    });

    ConsumerInfo {
        starter,
        thread_name,
        cpu_affinity,
        stage_index,
        readonly: false,
    }
}

/// Start a read-only fan-out consumer (each sees every sequence via `&E`).
pub(crate) fn start_readonly_consumer_thread<E, F, W>(
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
    let consumer_sequence = Arc::new(Sequence::new(INITIAL_CURSOR_VALUE));
    let consumer_sequence_clone = consumer_sequence.clone();

    let ConsumerThreadConfig {
        thread_name,
        cpu_affinity,
        shutdown_flag,
        work_sequence,
    } = config;

    debug_assert!(
        work_sequence.is_none(),
        "readonly fan-out must not use WorkerPool work sequence"
    );

    let thread_name = thread_name.unwrap_or_else(|| "disruptor-fanout".to_string());
    let thread_name_clone = thread_name.clone();

    let join_handle = thread::Builder::new()
        .name(thread_name.clone())
        .spawn(move || {
            if let Some(core_id) = cpu_affinity {
                if let Err(e) = set_thread_affinity(core_id) {
                    #[cfg(debug_assertions)]
                    eprintln!(
                        "Warning: Failed to set CPU affinity for fan-out '{thread_name}' to core {core_id}: {e}"
                    );
                    #[cfg(not(debug_assertions))]
                    let _ = e;
                }
            }

            let control = LoopControl {
                shutdown: &shutdown_flag,
                running: None,
            };

            consumer_engine::run_sequential_readonly_loop(
                &ring_buffer,
                &sequence_barrier,
                &mut on_event,
                &consumer_sequence_clone,
                control,
                &thread_name,
            );
        })
        .expect("Failed to spawn fan-out consumer thread");

    (
        consumer_sequence,
        Consumer::new(join_handle, thread_name_clone),
    )
}

pub(crate) fn consumer_info_from_readonly<E, F, W>(
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
        start_readonly_consumer_thread(ring_buffer, sequence_barrier, on_event, config)
    });

    ConsumerInfo {
        starter,
        thread_name,
        cpu_affinity,
        stage_index,
        readonly: true,
    }
}

/// Reject mixing mutable WorkerPool handlers with read-only fan-out on one stage.
pub(crate) fn assert_stage_mode_compatible<E, W>(
    consumers: &[ConsumerInfo<E, W>],
    stage_index: usize,
    readonly: bool,
) where
    E: Send + Sync + 'static,
    W: WaitStrategy + 'static,
{
    for c in consumers {
        assert!(
            !(c.stage_index == stage_index && c.readonly != readonly),
            "cannot mix mutable handlers (WorkerPool / &mut E) with read-only fan-out (&E) on the same stage (stage {stage_index})"
        );
    }
}
