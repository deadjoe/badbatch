//! DisruptorCore and topology assembly (create_disruptor_core).
//!
//! Consumer threads are started here; the hot loops live in [`crate::disruptor::consumer_engine`].

use super::consumer::{Consumer, ConsumerInfo, ConsumerThreadConfig};
use crate::disruptor::{
    consumer_engine,
    event_factory::ClosureEventFactory,
    producer::SimpleProducer,
    ring_buffer::SlotPadding,
    sequence_barrier::ProcessingSequenceBarrier,
    sequencer::{MultiProducerSequencer, SequencerEnum, SingleProducerSequencer},
    RingBuffer, Sequence, Sequencer, WaitStrategy,
};
use crossbeam_utils::CachePadded;
use std::sync::{
    atomic::{AtomicBool, AtomicI64, Ordering},
    Arc,
};

/// Core Disruptor components shared between DSL and Builder APIs
///
/// This struct contains the common implementation used by both the DSL-style
/// Disruptor class and the Builder-style API to avoid code duplication.
#[derive(Debug, Clone)]
pub struct DisruptorCore<E, W>
where
    W: WaitStrategy + 'static,
{
    /// Shared ring buffer storing all events managed by the disruptor.
    pub ring_buffer: Arc<RingBuffer<E>>,
    /// Sequencer coordinating producer access to the ring buffer (enum dispatch, no vtable).
    pub sequencer: SequencerEnum<W>,
    /// Consumer handles that process published events.
    pub consumers: Vec<Consumer>,
    /// Shared flag signaling graceful shutdown to consumer threads.
    pub shutdown_flag: Arc<AtomicBool>,
    gating_sequences: Vec<Arc<Sequence>>,
}

impl<E, W> DisruptorCore<E, W>
where
    E: Send + Sync + 'static,
    W: WaitStrategy + 'static,
{
    /// Create a new DisruptorCore with the given components
    pub fn new(
        ring_buffer: Arc<RingBuffer<E>>,
        sequencer: SequencerEnum<W>,
        consumers: Vec<Consumer>,
        shutdown_flag: Arc<AtomicBool>,
        gating_sequences: Vec<Arc<Sequence>>,
    ) -> Self {
        Self {
            ring_buffer,
            sequencer,
            consumers,
            shutdown_flag,
            gating_sequences,
        }
    }

    /// Get the buffer size
    pub fn buffer_size(&self) -> usize {
        self.ring_buffer.buffer_size()
    }

    /// Get the number of consumers
    pub fn consumer_count(&self) -> usize {
        self.consumers.len()
    }

    /// Shutdown all consumers
    pub fn shutdown(&mut self) {
        if self.shutdown_flag.swap(true, Ordering::AcqRel) {
            return;
        }

        // Wait for all consumer threads to finish
        for mut consumer in self.consumers.drain(..) {
            if let Some(handle) = consumer.join_handle.take() {
                if let Err(e) = handle.join() {
                    #[cfg(debug_assertions)]
                    eprintln!(
                        "Error joining consumer thread '{thread_name}': {e:?}",
                        thread_name = consumer.thread_name
                    );
                    #[cfg(not(debug_assertions))]
                    let _ = e;
                }
            }
        }

        // Remove gating sequences to restore full producer capacity
        for sequence in self.gating_sequences.drain(..) {
            self.sequencer.remove_gating_sequence(sequence);
        }
    }

    /// Create a producer for this disruptor core
    pub fn create_producer(&self) -> SimpleProducer<E, W> {
        SimpleProducer::new(self.ring_buffer.clone(), self.sequencer.clone())
    }
}

/// Factory function to create a DisruptorCore from components
///
/// This function provides a unified way to create DisruptorCore instances
/// that can be used by both DSL-style and Builder-style APIs.
#[allow(clippy::too_many_lines)]
pub fn create_disruptor_core<E, F, W>(
    size: usize,
    event_factory: F,
    wait_strategy: W,
    consumers: Vec<ConsumerInfo<E, W>>,
    is_multi_producer: bool,
    slot_padding: SlotPadding,
) -> DisruptorCore<E, W>
where
    E: Send + Sync + 'static,
    F: Fn() -> E + Send + Sync + 'static,
    W: WaitStrategy + 'static,
{
    // Create the ring buffer
    let closure_factory = ClosureEventFactory::new(event_factory);
    let ring_buffer = Arc::new(
        RingBuffer::new_with_padding(size, closure_factory, slot_padding)
            .expect("Failed to create ring buffer"),
    );

    // Monomorphized wait strategy — stored as Arc<W>, no dyn vtable.
    let wait_strategy_arc = Arc::new(wait_strategy);

    // Create the appropriate sequencer
    let sequencer: SequencerEnum<W> = if is_multi_producer {
        SequencerEnum::Multi(Arc::new(MultiProducerSequencer::new(
            size,
            wait_strategy_arc.clone(),
        )))
    } else {
        SequencerEnum::Single(Arc::new(SingleProducerSequencer::new(
            size,
            wait_strategy_arc.clone(),
        )))
    };

    // Create shutdown flag
    let shutdown_flag = Arc::new(AtomicBool::new(false));

    let max_stage_index = consumers.iter().map(|consumer| consumer.stage_index).max();
    let stage_widths = max_stage_index.map_or_else(Vec::new, |max_stage| {
        let mut widths = vec![0_usize; max_stage + 1];
        for consumer in &consumers {
            widths[consumer.stage_index] += 1;
        }
        widths
    });

    // LMAX WorkerPool scheme A: one shared work-sequence cursor per parallel stage.
    let stage_work_sequences: Vec<Option<Arc<CachePadded<AtomicI64>>>> = stage_widths
        .iter()
        .map(|width| {
            if *width > 1 {
                Some(consumer_engine::new_work_sequence())
            } else {
                None
            }
        })
        .collect();

    // Start consumer threads
    let mut consumer_threads: Vec<Consumer> = Vec::new();
    let mut consumer_sequences: Vec<Arc<Sequence>> = Vec::new();
    let mut stage_sequences = stage_widths
        .iter()
        .map(|_| Vec::<Arc<Sequence>>::new())
        .collect::<Vec<_>>();

    for consumer_info in consumers {
        let ConsumerInfo {
            starter,
            thread_name,
            cpu_affinity,
            stage_index,
        } = consumer_info;

        let dependent_sequences = if stage_index == 0 {
            Vec::new()
        } else {
            let previous_stage_sequences = stage_sequences
                .get(stage_index - 1)
                .expect("consumer stages must be contiguous");
            assert!(
                !previous_stage_sequences.is_empty(),
                "dependent consumer stage must have upstream consumers"
            );
            previous_stage_sequences.clone()
        };

        // Monomorphized barrier so wait_for has no dyn vtable on the hot path.
        let sequence_barrier = Arc::new(ProcessingSequenceBarrier::new(
            sequencer.get_cursor(),
            wait_strategy_arc.clone(),
            dependent_sequences,
            sequencer.clone(),
        ));

        let work_sequence = stage_work_sequences
            .get(stage_index)
            .and_then(Option::as_ref)
            .cloned();

        // Start the consumer thread
        let (consumer_sequence, consumer) = starter(
            ring_buffer.clone(),
            sequence_barrier,
            ConsumerThreadConfig {
                thread_name,
                cpu_affinity,
                shutdown_flag: shutdown_flag.clone(),
                work_sequence,
            },
        );

        let stage_sequence = consumer_sequence.clone();
        consumer_sequences.push(consumer_sequence);
        stage_sequences
            .get_mut(stage_index)
            .expect("consumer stage must exist")
            .push(stage_sequence);
        consumer_threads.push(consumer);
    }

    // Register consumer sequences with the sequencer for backpressure.
    // For parallel stages, each worker's sequence is a gating sequence; the
    // minimum across workers provides correct backpressure (LMAX WorkerPool).
    if !consumer_sequences.is_empty() {
        sequencer.add_gating_sequences(&consumer_sequences);
    }

    DisruptorCore::new(
        ring_buffer,
        sequencer,
        consumer_threads,
        shutdown_flag,
        consumer_sequences,
    )
}

/// Set CPU affinity for the current thread
///
/// This function attempts to pin the current thread to a specific CPU core.
/// It uses the core_affinity crate for cross-platform support.
///
/// # Arguments
/// * `core_id` - The CPU core ID to pin the thread to
///
/// # Returns
/// * `Ok(())` if the affinity was set successfully
/// * `Err(String)` if setting affinity failed
pub(crate) fn set_thread_affinity(core_id: usize) -> Result<(), String> {
    // Get available CPU cores
    let core_ids = core_affinity::get_core_ids()
        .ok_or_else(|| "Failed to get available CPU cores".to_string())?;

    // Check if the requested core ID is valid
    if core_id >= core_ids.len() {
        return Err(format!(
            "Invalid core ID: {core_id}. Available cores: 0-{}",
            core_ids.len() - 1
        ));
    }

    // Set the affinity to the specified core
    let target_core = core_ids[core_id];
    if core_affinity::set_for_current(target_core) {
        Ok(())
    } else {
        Err(format!("Failed to set CPU affinity to core {core_id}"))
    }
}
