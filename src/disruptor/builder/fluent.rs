//! Type-state fluent builders (single / multi / dependent stages).
//!
//! Adding consumers is centralized on [`SharedBuilderState`]; builder wrappers
//! only flip type-state and expose a thin API.

use super::consumer::{
    assert_access_compatible, consumer_info_mutable, consumer_info_readonly, ConsumerAccess,
    ConsumerInfo, HasConsumers, NoConsumers,
};
use super::core::create_disruptor_core;
use super::handle::DisruptorHandle;
use crate::disruptor::{
    event_handler::ClosureEventHandler,
    producer::{Producer, SimpleProducer},
    ring_buffer::SlotPadding,
    sequencer::SequencerEnum,
    EventHandler, RingBuffer, WaitStrategy,
};
use std::marker::PhantomData;
use std::sync::Arc;

/// Shared mutable builder state (stage index, consumers, wait strategy, …).
pub(crate) struct SharedBuilderState<E, W>
where
    E: Send + Sync + 'static,
    W: WaitStrategy + 'static,
{
    pub(crate) size: usize,
    pub(crate) slot_padding: SlotPadding,
    pub(crate) wait_strategy: W,
    pub(crate) consumers: Vec<ConsumerInfo<E, W>>,
    pub(crate) current_thread_name: Option<String>,
    pub(crate) current_cpu_affinity: Option<usize>,
    pub(crate) current_stage_index: usize,
    pub(crate) current_stage_width: usize,
}

impl<E, W> SharedBuilderState<E, W>
where
    E: Send + Sync + 'static,
    W: WaitStrategy + 'static,
{
    fn new(size: usize, wait_strategy: W) -> Self {
        Self {
            size,
            slot_padding: SlotPadding::None,
            wait_strategy,
            consumers: Vec::new(),
            current_thread_name: None,
            current_cpu_affinity: None,
            current_stage_index: 0,
            current_stage_width: 0,
        }
    }

    fn take_thread_meta(&mut self) -> (Option<String>, Option<usize>) {
        (
            self.current_thread_name.take(),
            self.current_cpu_affinity.take(),
        )
    }

    fn push_mutable(&mut self, info: ConsumerInfo<E, W>, first_on_builder: bool) {
        assert_access_compatible(
            &self.consumers,
            self.current_stage_index,
            ConsumerAccess::Mutable,
        );
        self.consumers.push(info);
        if first_on_builder {
            self.current_stage_width = 1;
        } else {
            self.current_stage_width += 1;
        }
    }

    fn push_readonly(&mut self, info: ConsumerInfo<E, W>, first_on_builder: bool) {
        assert_access_compatible(
            &self.consumers,
            self.current_stage_index,
            ConsumerAccess::Readonly,
        );
        self.consumers.push(info);
        if first_on_builder {
            self.current_stage_width = 1;
        } else {
            self.current_stage_width += 1;
        }
    }

    fn add_mut_closure<H>(&mut self, mut handler: H, first: bool)
    where
        H: FnMut(&mut E, i64, bool) + Send + Sync + 'static,
    {
        let (name, cpu) = self.take_thread_meta();
        let stage = self.current_stage_index;
        let info = consumer_info_mutable(
            ClosureEventHandler::new(move |e, s, b| {
                handler(e, s, b);
                Ok(())
            }),
            name,
            cpu,
            stage,
        );
        self.push_mutable(info, first);
    }

    fn add_mut_handler<H>(&mut self, handler: H, first: bool)
    where
        H: EventHandler<E> + Send + Sync + 'static,
    {
        let (name, cpu) = self.take_thread_meta();
        let stage = self.current_stage_index;
        self.push_mutable(consumer_info_mutable(handler, name, cpu, stage), first);
    }

    fn add_fanout<H>(&mut self, mut handler: H, first: bool)
    where
        H: FnMut(&E, i64, bool) + Send + 'static,
    {
        let (name, cpu) = self.take_thread_meta();
        let stage = self.current_stage_index;
        let info = consumer_info_readonly(
            move |e, s, b| {
                handler(e, s, b);
                Ok(())
            },
            name,
            cpu,
            stage,
        );
        self.push_readonly(info, first);
    }

    fn advance_stage(&mut self) {
        self.current_stage_index += 1;
        self.current_stage_width = 0;
    }
}

// --- Single producer -----------------------------------------------------------------

/// Type-state builder for a single-producer Disruptor.
///
/// Start with [`crate::disruptor::build_single_producer`], add consumers with
/// `handle_events_with` / `fan_out_events_with`, then [`Self::build`].
pub struct SingleProducerBuilder<State, E, F, W>
where
    E: Send + Sync + 'static,
    F: Fn() -> E + Send + Sync + 'static,
    W: WaitStrategy + 'static,
{
    event_factory: F,
    pub(crate) shared: SharedBuilderState<E, W>,
    _phantom: PhantomData<(State, E)>,
}

/// Builder stage created by [`SingleProducerBuilder::and_then`].
///
/// Consumers registered here form a new pipeline stage that waits for the
/// previous stage's sequences before processing.
pub struct DependentConsumerBuilder<E, F, W>
where
    E: Send + Sync + 'static,
    F: Fn() -> E + Send + Sync + 'static,
    W: WaitStrategy + Clone + 'static,
{
    builder: SingleProducerBuilder<HasConsumers, E, F, W>,
}

impl<E, F, W> DependentConsumerBuilder<E, F, W>
where
    E: Send + Sync + 'static,
    F: Fn() -> E + Send + Sync + 'static,
    W: WaitStrategy + Clone + 'static,
{
    #[must_use]
    /// Pin the next consumer thread to a CPU core (best-effort).
    pub fn pin_at_core(mut self, core_id: usize) -> Self {
        self.builder.shared.current_cpu_affinity = Some(core_id);
        self
    }

    #[must_use]
    /// Set the OS thread name for the next consumer.
    pub fn thread_name<S: Into<String>>(mut self, name: S) -> Self {
        self.builder.shared.current_thread_name = Some(name.into());
        self
    }

    #[must_use]
    /// Configure ring-buffer slot padding strategy.
    pub fn with_slot_padding(mut self, slot_padding: SlotPadding) -> Self {
        self.builder.shared.slot_padding = slot_padding;
        self
    }

    #[must_use]
    /// Enable 128-byte cache-line slot padding when `enabled` is true.
    pub fn with_cache_line_padding(self, enabled: bool) -> Self {
        self.with_slot_padding(if enabled {
            SlotPadding::CacheLine128
        } else {
            SlotPadding::None
        })
    }

    #[must_use]
    /// Register a mutable closure handler (`&mut E`) on the current stage.
    pub fn handle_events_with<H>(
        mut self,
        handler: H,
    ) -> SingleProducerBuilder<HasConsumers, E, F, W>
    where
        H: FnMut(&mut E, i64, bool) + Send + Sync + 'static,
    {
        self.builder.shared.add_mut_closure(handler, true);
        self.builder
    }

    #[must_use]
    /// Register a mutable [`EventHandler`] on the current stage.
    pub fn handle_events_with_handler<H>(
        mut self,
        handler: H,
    ) -> SingleProducerBuilder<HasConsumers, E, F, W>
    where
        H: EventHandler<E> + Send + Sync + 'static,
    {
        self.builder.shared.add_mut_handler(handler, true);
        self.builder
    }

    #[must_use]
    /// Register a read-only fan-out handler (`&E`) that observes every sequence.
    pub fn fan_out_events_with<H>(
        mut self,
        handler: H,
    ) -> SingleProducerBuilder<HasConsumers, E, F, W>
    where
        H: FnMut(&E, i64, bool) + Send + 'static,
    {
        self.builder.shared.add_fanout(handler, true);
        self.builder
    }
}

impl<E, F, W> SingleProducerBuilder<NoConsumers, E, F, W>
where
    E: Send + Sync + 'static,
    F: Fn() -> E + Send + Sync + 'static,
    W: WaitStrategy + 'static,
{
    pub(crate) fn new(size: usize, event_factory: F, wait_strategy: W) -> Self {
        Self {
            event_factory,
            shared: SharedBuilderState::new(size, wait_strategy),
            _phantom: PhantomData,
        }
    }

    #[must_use]
    /// Configure ring-buffer slot padding strategy.
    pub fn with_slot_padding(mut self, slot_padding: SlotPadding) -> Self {
        self.shared.slot_padding = slot_padding;
        self
    }

    #[must_use]
    /// Enable 128-byte cache-line slot padding when `enabled` is true.
    pub fn with_cache_line_padding(self, enabled: bool) -> Self {
        self.with_slot_padding(if enabled {
            SlotPadding::CacheLine128
        } else {
            SlotPadding::None
        })
    }

    #[must_use]
    /// Pin the next consumer thread to a CPU core (best-effort).
    pub fn pin_at_core(mut self, core_id: usize) -> Self {
        self.shared.current_cpu_affinity = Some(core_id);
        self
    }

    #[must_use]
    /// Set the OS thread name for the next consumer.
    pub fn thread_name<S: Into<String>>(mut self, name: S) -> Self {
        self.shared.current_thread_name = Some(name.into());
        self
    }

    #[must_use]
    /// Register a mutable closure handler (`&mut E`) on the current stage.
    pub fn handle_events_with<H>(
        mut self,
        handler: H,
    ) -> SingleProducerBuilder<HasConsumers, E, F, W>
    where
        H: FnMut(&mut E, i64, bool) + Send + Sync + 'static,
    {
        self.shared.add_mut_closure(handler, true);
        SingleProducerBuilder {
            event_factory: self.event_factory,
            shared: self.shared,
            _phantom: PhantomData,
        }
    }

    #[must_use]
    /// Register a mutable [`EventHandler`] on the current stage.
    pub fn handle_events_with_handler<H>(
        mut self,
        handler: H,
    ) -> SingleProducerBuilder<HasConsumers, E, F, W>
    where
        H: EventHandler<E> + Send + Sync + 'static,
    {
        self.shared.add_mut_handler(handler, true);
        SingleProducerBuilder {
            event_factory: self.event_factory,
            shared: self.shared,
            _phantom: PhantomData,
        }
    }

    #[must_use]
    /// Register a read-only fan-out handler (`&E`) that observes every sequence.
    pub fn fan_out_events_with<H>(
        mut self,
        handler: H,
    ) -> SingleProducerBuilder<HasConsumers, E, F, W>
    where
        H: FnMut(&E, i64, bool) + Send + 'static,
    {
        self.shared.add_fanout(handler, true);
        SingleProducerBuilder {
            event_factory: self.event_factory,
            shared: self.shared,
            _phantom: PhantomData,
        }
    }
}

impl<E, F, W> SingleProducerBuilder<HasConsumers, E, F, W>
where
    E: Send + Sync + 'static,
    F: Fn() -> E + Send + Sync + 'static,
    W: WaitStrategy + Clone + 'static,
{
    #[must_use]
    /// Pin the next consumer thread to a CPU core (best-effort).
    pub fn pin_at_core(mut self, core_id: usize) -> Self {
        self.shared.current_cpu_affinity = Some(core_id);
        self
    }

    #[must_use]
    /// Set the OS thread name for the next consumer.
    pub fn thread_name<S: Into<String>>(mut self, name: S) -> Self {
        self.shared.current_thread_name = Some(name.into());
        self
    }

    #[must_use]
    /// Configure ring-buffer slot padding strategy.
    pub fn with_slot_padding(mut self, slot_padding: SlotPadding) -> Self {
        self.shared.slot_padding = slot_padding;
        self
    }

    #[must_use]
    /// Enable 128-byte cache-line slot padding when `enabled` is true.
    pub fn with_cache_line_padding(self, enabled: bool) -> Self {
        self.with_slot_padding(if enabled {
            SlotPadding::CacheLine128
        } else {
            SlotPadding::None
        })
    }

    #[must_use]
    /// Register a mutable closure handler (`&mut E`) on the current stage.
    pub fn handle_events_with<H>(mut self, handler: H) -> Self
    where
        H: FnMut(&mut E, i64, bool) + Send + Sync + 'static,
    {
        self.shared.add_mut_closure(handler, false);
        self
    }

    #[must_use]
    /// Register a mutable [`EventHandler`] on the current stage.
    pub fn handle_events_with_handler<H>(mut self, handler: H) -> Self
    where
        H: EventHandler<E> + Send + Sync + 'static,
    {
        self.shared.add_mut_handler(handler, false);
        self
    }

    #[must_use]
    /// Register a read-only fan-out handler (`&E`) that observes every sequence.
    pub fn fan_out_events_with<H>(mut self, handler: H) -> Self
    where
        H: FnMut(&E, i64, bool) + Send + 'static,
    {
        self.shared.add_fanout(handler, false);
        self
    }

    #[must_use]
    /// Start a dependent pipeline stage that waits for the previous stage.
    pub fn and_then(mut self) -> DependentConsumerBuilder<E, F, W> {
        self.shared.advance_stage();
        DependentConsumerBuilder { builder: self }
    }

    /// Assemble the disruptor, start consumers, and return a handle.
    pub fn build(self) -> DisruptorHandle<E, W> {
        let core = create_disruptor_core(
            self.shared.size,
            self.event_factory,
            self.shared.wait_strategy,
            self.shared.consumers,
            false,
            self.shared.slot_padding,
        );
        DisruptorHandle::new(core)
    }
}

// --- Multi producer ------------------------------------------------------------------

/// Type-state builder for a multi-producer Disruptor.
///
/// Start with [`crate::disruptor::build_multi_producer`], add consumers, then
/// [`Self::build`]. Producers are coordinated through a shared multi-producer sequencer.
pub struct MultiProducerBuilder<State, E, F, W>
where
    E: Send + Sync + 'static,
    F: Fn() -> E + Send + Sync + 'static,
    W: WaitStrategy + Clone + 'static,
{
    event_factory: F,
    pub(crate) shared: SharedBuilderState<E, W>,
    _phantom: PhantomData<(State, E)>,
}

impl<E, F, W> MultiProducerBuilder<NoConsumers, E, F, W>
where
    E: Send + Sync + 'static,
    F: Fn() -> E + Send + Sync + 'static,
    W: WaitStrategy + Clone + 'static,
{
    pub(crate) fn new(size: usize, event_factory: F, wait_strategy: W) -> Self {
        Self {
            event_factory,
            shared: SharedBuilderState::new(size, wait_strategy),
            _phantom: PhantomData,
        }
    }

    #[must_use]
    /// Pin the next consumer thread to a CPU core (best-effort).
    pub fn pin_at_core(mut self, core_id: usize) -> Self {
        self.shared.current_cpu_affinity = Some(core_id);
        self
    }

    #[must_use]
    /// Set the OS thread name for the next consumer.
    pub fn thread_name<S: Into<String>>(mut self, name: S) -> Self {
        self.shared.current_thread_name = Some(name.into());
        self
    }

    #[must_use]
    /// Configure ring-buffer slot padding strategy.
    pub fn with_slot_padding(mut self, slot_padding: SlotPadding) -> Self {
        self.shared.slot_padding = slot_padding;
        self
    }

    #[must_use]
    /// Enable 128-byte cache-line slot padding when `enabled` is true.
    pub fn with_cache_line_padding(self, enabled: bool) -> Self {
        self.with_slot_padding(if enabled {
            SlotPadding::CacheLine128
        } else {
            SlotPadding::None
        })
    }

    #[must_use]
    /// Register a mutable closure handler (`&mut E`) on the current stage.
    pub fn handle_events_with<H>(
        mut self,
        handler: H,
    ) -> MultiProducerBuilder<HasConsumers, E, F, W>
    where
        H: FnMut(&mut E, i64, bool) + Send + Sync + 'static,
    {
        self.shared.add_mut_closure(handler, true);
        MultiProducerBuilder {
            event_factory: self.event_factory,
            shared: self.shared,
            _phantom: PhantomData,
        }
    }

    #[must_use]
    /// Register a mutable [`EventHandler`] on the current stage.
    pub fn handle_events_with_handler<H>(
        mut self,
        handler: H,
    ) -> MultiProducerBuilder<HasConsumers, E, F, W>
    where
        H: EventHandler<E> + Send + Sync + 'static,
    {
        self.shared.add_mut_handler(handler, true);
        MultiProducerBuilder {
            event_factory: self.event_factory,
            shared: self.shared,
            _phantom: PhantomData,
        }
    }

    #[must_use]
    /// Register a read-only fan-out handler (`&E`) that observes every sequence.
    pub fn fan_out_events_with<H>(
        mut self,
        handler: H,
    ) -> MultiProducerBuilder<HasConsumers, E, F, W>
    where
        H: FnMut(&E, i64, bool) + Send + 'static,
    {
        self.shared.add_fanout(handler, true);
        MultiProducerBuilder {
            event_factory: self.event_factory,
            shared: self.shared,
            _phantom: PhantomData,
        }
    }
}

impl<E, F, W> MultiProducerBuilder<HasConsumers, E, F, W>
where
    E: Send + Sync + 'static,
    F: Fn() -> E + Send + Sync + 'static,
    W: WaitStrategy + Clone + 'static,
{
    #[must_use]
    /// Pin the next consumer thread to a CPU core (best-effort).
    pub fn pin_at_core(mut self, core_id: usize) -> Self {
        self.shared.current_cpu_affinity = Some(core_id);
        self
    }

    #[must_use]
    /// Set the OS thread name for the next consumer.
    pub fn thread_name<S: Into<String>>(mut self, name: S) -> Self {
        self.shared.current_thread_name = Some(name.into());
        self
    }

    #[must_use]
    /// Register a mutable closure handler (`&mut E`) on the current stage.
    pub fn handle_events_with<H>(mut self, handler: H) -> Self
    where
        H: FnMut(&mut E, i64, bool) + Send + Sync + 'static,
    {
        self.shared.add_mut_closure(handler, false);
        self
    }

    #[must_use]
    /// Register a mutable [`EventHandler`] on the current stage.
    pub fn handle_events_with_handler<H>(mut self, handler: H) -> Self
    where
        H: EventHandler<E> + Send + Sync + 'static,
    {
        self.shared.add_mut_handler(handler, false);
        self
    }

    #[must_use]
    /// Register a read-only fan-out handler (`&E`) that observes every sequence.
    pub fn fan_out_events_with<H>(mut self, handler: H) -> Self
    where
        H: FnMut(&E, i64, bool) + Send + 'static,
    {
        self.shared.add_fanout(handler, false);
        self
    }

    /// Assemble the disruptor, start consumers, and return a handle.
    pub fn build(self) -> DisruptorHandle<E, W> {
        let core = create_disruptor_core(
            self.shared.size,
            self.event_factory,
            self.shared.wait_strategy,
            self.shared.consumers,
            true,
            self.shared.slot_padding,
        );
        DisruptorHandle::new(core)
    }
}

// --- Cloneable multi-producer convenience --------------------------------------------

/// Cloneable multi-producer handle sharing one ring buffer and sequencer.
///
/// Each clone can publish concurrently; sequencing is coordinated by the
/// multi-producer sequencer (bitmap / cursor CAS).
#[derive(Clone)]
pub struct CloneableProducer<E, W>
where
    E: Send + Sync,
    W: WaitStrategy + 'static,
{
    ring_buffer: Arc<RingBuffer<E>>,
    sequencer: SequencerEnum<W>,
}

impl<E, W> CloneableProducer<E, W>
where
    E: Send + Sync,
    W: WaitStrategy + 'static,
{
    /// Create a cloneable producer from a shared ring buffer and sequencer.
    pub fn new(ring_buffer: Arc<RingBuffer<E>>, sequencer: SequencerEnum<W>) -> Self {
        Self {
            ring_buffer,
            sequencer,
        }
    }

    /// Create a [`SimpleProducer`] bound to the shared ring buffer.
    pub fn create_producer(&self) -> SimpleProducer<E, W> {
        SimpleProducer::new(self.ring_buffer.clone(), self.sequencer.clone())
    }

    /// Try to publish one event; returns [`crate::disruptor::RingBufferFull`] if no slot is free.
    pub fn try_publish<F>(
        &self,
        update: F,
    ) -> std::result::Result<i64, crate::disruptor::producer::RingBufferFull>
    where
        F: FnOnce(&mut E),
    {
        self.create_producer().try_publish(update)
    }

    /// Publish one event, spinning until a slot is available.
    pub fn publish<F>(&self, update: F)
    where
        F: FnOnce(&mut E),
    {
        self.create_producer().publish(update);
    }

    /// Try to publish a batch of `n` events.
    pub fn try_batch_publish<F>(
        &self,
        n: usize,
        update: F,
    ) -> std::result::Result<i64, crate::disruptor::producer::MissingFreeSlots>
    where
        F: for<'a> FnOnce(crate::disruptor::ring_buffer::BatchIterMut<'a, E>),
    {
        self.create_producer().try_batch_publish(n, update)
    }

    /// Publish a batch of `n` events, spinning until space is available.
    pub fn batch_publish<F>(&self, n: usize, update: F)
    where
        F: for<'a> FnOnce(crate::disruptor::ring_buffer::BatchIterMut<'a, E>),
    {
        self.create_producer().batch_publish(n, update);
    }
}
