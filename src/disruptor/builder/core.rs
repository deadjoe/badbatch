//! DisruptorCore and topology assembly (`create_disruptor_core`).
//!
//! Consumer threads are started here; hot loops live in [`crate::disruptor::consumer_engine`].

use super::consumer::{Consumer, ConsumerInfo, ConsumerThreadConfig};
use crate::disruptor::{
    consumer_engine::RunMode,
    event_factory::ClosureEventFactory,
    producer::SimpleProducer,
    ring_buffer::SlotPadding,
    sequence_barrier::ProcessingSequenceBarrier,
    sequencer::{MultiProducerSequencer, SequencerEnum, SingleProducerSequencer},
    RingBuffer, Sequence, Sequencer, WaitStrategy,
};
use std::sync::{atomic::AtomicBool, atomic::Ordering, Arc};

/// Core Disruptor components shared between DSL and Builder APIs.
///
/// Holds the ring buffer, sequencer, consumer handles, and gating sequences so
/// both the classic DSL and the type-state Builder share one assembly path.
#[derive(Debug, Clone)]
pub struct DisruptorCore<E, W>
where
    W: WaitStrategy + 'static,
{
    /// Shared ring buffer storing events.
    pub ring_buffer: Arc<RingBuffer<E>>,
    /// Sequencer coordinating producer access (enum dispatch, no vtable).
    pub sequencer: SequencerEnum<W>,
    /// Managed consumer threads.
    pub consumers: Vec<Consumer>,
    /// Cooperative shutdown flag observed by builder-spawned consumers.
    pub shutdown_flag: Arc<AtomicBool>,
    gating_sequences: Vec<Arc<Sequence>>,
}

impl<E, W> DisruptorCore<E, W>
where
    E: Send + Sync + 'static,
    W: WaitStrategy + 'static,
{
    /// Create a core from already-assembled components.
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

    /// Ring buffer capacity (power of two).
    pub fn buffer_size(&self) -> usize {
        self.ring_buffer.buffer_size()
    }

    /// Number of started consumer threads.
    pub fn consumer_count(&self) -> usize {
        self.consumers.len()
    }

    /// Signal shutdown and join all consumer threads.
    pub fn shutdown(&mut self) {
        if self.shutdown_flag.swap(true, Ordering::AcqRel) {
            return;
        }

        for mut consumer in self.consumers.drain(..) {
            if let Some(handle) = consumer.join_handle.take() {
                if let Err(e) = handle.join() {
                    #[cfg(debug_assertions)]
                    eprintln!(
                        "Error joining consumer thread '{}': {e:?}",
                        consumer.thread_name
                    );
                    #[cfg(not(debug_assertions))]
                    let _ = e;
                }
            }
        }

        for sequence in self.gating_sequences.drain(..) {
            self.sequencer.remove_gating_sequence(sequence);
        }
    }

    /// Create a producer bound to this core's ring buffer and sequencer.
    pub fn create_producer(&self) -> SimpleProducer<E, W> {
        SimpleProducer::new(self.ring_buffer.clone(), self.sequencer.clone())
    }
}

/// Assemble ring, sequencer, barriers, and start consumers.
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
    let closure_factory = ClosureEventFactory::new(event_factory);
    let ring_buffer = Arc::new(
        RingBuffer::new_with_padding(size, closure_factory, slot_padding)
            .expect("Failed to create ring buffer"),
    );

    let wait_strategy_arc = Arc::new(wait_strategy);
    let sequencer: SequencerEnum<W> = if is_multi_producer {
        SequencerEnum::Multi(Arc::new(MultiProducerSequencer::new(
            size,
            Arc::clone(&wait_strategy_arc),
        )))
    } else {
        SequencerEnum::Single(Arc::new(SingleProducerSequencer::new(
            size,
            Arc::clone(&wait_strategy_arc),
        )))
    };

    let shutdown_flag = Arc::new(AtomicBool::new(false));

    let stage_count = consumers
        .iter()
        .map(|c| c.stage_index)
        .max()
        .map_or(0, |m| m + 1);

    let stage_modes = super::consumer::stage_run_modes(&consumers, stage_count.max(1));

    let mut consumer_threads: Vec<Consumer> = Vec::new();
    let mut consumer_sequences: Vec<Arc<Sequence>> = Vec::new();
    let mut stage_sequences = (0..stage_count.max(1))
        .map(|_| Vec::<Arc<Sequence>>::new())
        .collect::<Vec<_>>();

    for consumer_info in consumers {
        let ConsumerInfo {
            starter,
            thread_name,
            cpu_affinity,
            stage_index,
            access: _,
        } = consumer_info;

        let dependent_sequences = if stage_index == 0 {
            Vec::new()
        } else {
            let previous = stage_sequences
                .get(stage_index - 1)
                .expect("consumer stages must be contiguous");
            assert!(
                !previous.is_empty(),
                "dependent consumer stage must have upstream consumers"
            );
            previous.clone()
        };

        let sequence_barrier = Arc::new(ProcessingSequenceBarrier::new(
            sequencer.get_cursor(),
            Arc::clone(&wait_strategy_arc),
            dependent_sequences,
            sequencer.clone(),
        ));

        let run_mode = stage_modes
            .get(stage_index)
            .cloned()
            .unwrap_or(RunMode::Sequential);

        let (consumer_sequence, consumer) = starter(
            Arc::clone(&ring_buffer),
            sequence_barrier,
            ConsumerThreadConfig {
                thread_name,
                cpu_affinity,
                shutdown_flag: Arc::clone(&shutdown_flag),
                run_mode,
            },
        );

        consumer_sequences.push(Arc::clone(&consumer_sequence));
        stage_sequences
            .get_mut(stage_index)
            .expect("consumer stage must exist")
            .push(consumer_sequence);
        consumer_threads.push(consumer);
    }

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

pub(crate) fn set_thread_affinity(core_id: usize) -> Result<(), String> {
    let core_ids = core_affinity::get_core_ids()
        .ok_or_else(|| "Failed to get available CPU cores".to_string())?;

    if core_id >= core_ids.len() {
        return Err(format!(
            "Invalid core ID: {core_id}. Available cores: 0-{}",
            core_ids.len() - 1
        ));
    }

    let target_core = core_ids[core_id];
    if core_affinity::set_for_current(target_core) {
        Ok(())
    } else {
        Err(format!("Failed to set CPU affinity to core {core_id}"))
    }
}
