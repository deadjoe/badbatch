//! Type-state fluent builders (single / multi / dependent stages).
//!
//! Consumer registration is centralized on [`SharedBuilderState`]. Builder
//! wrappers only flip type-state and expose a thin API. Shared method bodies are
//! generated once via macros so SP/MP/dependent surfaces stay in sync.
//!
//! # Pipeline stages
//!
//! Both single- and multi-producer builders support [`and_then`](SingleProducerBuilder::and_then)
//! for dependent stages. Same-stage parallel consumers use WorkerPool (mutable) or
//! fan-out (readonly); mixing access kinds on one stage panics.

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
        }
    }

    fn take_thread_meta(&mut self) -> (Option<String>, Option<usize>) {
        (
            self.current_thread_name.take(),
            self.current_cpu_affinity.take(),
        )
    }

    fn push(&mut self, info: ConsumerInfo<E, W>, access: ConsumerAccess) {
        assert_access_compatible(&self.consumers, self.current_stage_index, access);
        self.consumers.push(info);
    }

    fn add_mut_closure<H>(&mut self, mut handler: H)
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
        self.push(info, ConsumerAccess::Mutable);
    }

    fn add_mut_handler<H>(&mut self, handler: H)
    where
        H: EventHandler<E> + Send + Sync + 'static,
    {
        let (name, cpu) = self.take_thread_meta();
        let stage = self.current_stage_index;
        self.push(
            consumer_info_mutable(handler, name, cpu, stage),
            ConsumerAccess::Mutable,
        );
    }

    fn add_fanout<H>(&mut self, mut handler: H)
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
        self.push(info, ConsumerAccess::Readonly);
    }

    fn advance_stage(&mut self) {
        self.current_stage_index += 1;
    }
}

// --- Shared method macros ------------------------------------------------------------

/// Config methods that only mutate `shared` and return `Self`.
macro_rules! impl_builder_config {
    ($builder:ident, $state:ty, $($wait_bound:tt)*) => {
        impl<E, F, W> $builder<$state, E, F, W>
        where
            E: Send + Sync + 'static,
            F: Fn() -> E + Send + Sync + 'static,
            W: WaitStrategy $($wait_bound)* + 'static,
        {
            /// Pin the next consumer thread to a CPU core (best-effort).
            #[must_use]
            pub fn pin_at_core(mut self, core_id: usize) -> Self {
                self.shared.current_cpu_affinity = Some(core_id);
                self
            }

            /// Set the OS thread name for the next consumer.
            #[must_use]
            pub fn thread_name<S: Into<String>>(mut self, name: S) -> Self {
                self.shared.current_thread_name = Some(name.into());
                self
            }

            /// Configure ring-buffer slot padding strategy.
            #[must_use]
            pub fn with_slot_padding(mut self, slot_padding: SlotPadding) -> Self {
                self.shared.slot_padding = slot_padding;
                self
            }

            /// Enable 128-byte cache-line slot padding when `enabled` is true.
            #[must_use]
            pub fn with_cache_line_padding(self, enabled: bool) -> Self {
                self.with_slot_padding(if enabled {
                    SlotPadding::CacheLine128
                } else {
                    SlotPadding::None
                })
            }
        }
    };
}

/// First consumer methods: `NoConsumers` → `HasConsumers` type-state jump.
macro_rules! impl_first_handlers {
    ($builder:ident, $($wait_bound:tt)*) => {
        impl<E, F, W> $builder<NoConsumers, E, F, W>
        where
            E: Send + Sync + 'static,
            F: Fn() -> E + Send + Sync + 'static,
            W: WaitStrategy $($wait_bound)* + 'static,
        {
            /// Register a mutable closure handler (`&mut E`) on the current stage.
            #[must_use]
            pub fn handle_events_with<H>(
                mut self,
                handler: H,
            ) -> $builder<HasConsumers, E, F, W>
            where
                H: FnMut(&mut E, i64, bool) + Send + Sync + 'static,
            {
                self.shared.add_mut_closure(handler);
                $builder {
                    event_factory: self.event_factory,
                    shared: self.shared,
                    _phantom: PhantomData,
                }
            }

            /// Register a mutable [`EventHandler`] on the current stage.
            #[must_use]
            pub fn handle_events_with_handler<H>(
                mut self,
                handler: H,
            ) -> $builder<HasConsumers, E, F, W>
            where
                H: EventHandler<E> + Send + Sync + 'static,
            {
                self.shared.add_mut_handler(handler);
                $builder {
                    event_factory: self.event_factory,
                    shared: self.shared,
                    _phantom: PhantomData,
                }
            }

            /// Register a read-only fan-out handler (`&E`) that observes every sequence.
            #[must_use]
            pub fn fan_out_events_with<H>(
                mut self,
                handler: H,
            ) -> $builder<HasConsumers, E, F, W>
            where
                H: FnMut(&E, i64, bool) + Send + 'static,
            {
                self.shared.add_fanout(handler);
                $builder {
                    event_factory: self.event_factory,
                    shared: self.shared,
                    _phantom: PhantomData,
                }
            }
        }
    };
}

/// Additional same-stage consumers (already `HasConsumers`).
macro_rules! impl_more_handlers {
    ($builder:ident, $($wait_bound:tt)*) => {
        impl<E, F, W> $builder<HasConsumers, E, F, W>
        where
            E: Send + Sync + 'static,
            F: Fn() -> E + Send + Sync + 'static,
            W: WaitStrategy $($wait_bound)* + 'static,
        {
            /// Register a mutable closure handler (`&mut E`) on the current stage.
            #[must_use]
            pub fn handle_events_with<H>(mut self, handler: H) -> Self
            where
                H: FnMut(&mut E, i64, bool) + Send + Sync + 'static,
            {
                self.shared.add_mut_closure(handler);
                self
            }

            /// Register a mutable [`EventHandler`] on the current stage.
            #[must_use]
            pub fn handle_events_with_handler<H>(mut self, handler: H) -> Self
            where
                H: EventHandler<E> + Send + Sync + 'static,
            {
                self.shared.add_mut_handler(handler);
                self
            }

            /// Register a read-only fan-out handler (`&E`) that observes every sequence.
            #[must_use]
            pub fn fan_out_events_with<H>(mut self, handler: H) -> Self
            where
                H: FnMut(&E, i64, bool) + Send + 'static,
            {
                self.shared.add_fanout(handler);
                self
            }

            /// Start a dependent pipeline stage that waits for the previous stage.
            #[must_use]
            pub fn and_then(mut self) -> DependentBuilder<Self> {
                self.shared.advance_stage();
                DependentBuilder { inner: self }
            }
        }
    };
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
}

impl_builder_config!(SingleProducerBuilder, NoConsumers,);
impl_builder_config!(SingleProducerBuilder, HasConsumers, + Clone);
impl_first_handlers!(SingleProducerBuilder,);
impl_more_handlers!(SingleProducerBuilder, + Clone);

impl<E, F, W> SingleProducerBuilder<HasConsumers, E, F, W>
where
    E: Send + Sync + 'static,
    F: Fn() -> E + Send + Sync + 'static,
    W: WaitStrategy + Clone + 'static,
{
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
/// [`Self::build`]. Supports the same `and_then` pipeline stages as the single-producer
/// builder. Producers are coordinated through a shared multi-producer sequencer.
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
}

impl_builder_config!(MultiProducerBuilder, NoConsumers, + Clone);
impl_builder_config!(MultiProducerBuilder, HasConsumers, + Clone);
impl_first_handlers!(MultiProducerBuilder, + Clone);
impl_more_handlers!(MultiProducerBuilder, + Clone);

impl<E, F, W> MultiProducerBuilder<HasConsumers, E, F, W>
where
    E: Send + Sync + 'static,
    F: Fn() -> E + Send + Sync + 'static,
    W: WaitStrategy + Clone + 'static,
{
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

// --- Dependent stage (shared for SP and MP) ------------------------------------------

/// Builder stage created by `and_then()` on a single- or multi-producer builder.
///
/// Consumers registered here form a new pipeline stage that waits for the previous
/// stage's sequences before processing.
pub struct DependentBuilder<Inner> {
    inner: Inner,
}

/// Dependent stage after a single-producer builder.
pub type DependentConsumerBuilder<E, F, W> =
    DependentBuilder<SingleProducerBuilder<HasConsumers, E, F, W>>;

/// Dependent stage after a multi-producer builder.
pub type DependentMultiConsumerBuilder<E, F, W> =
    DependentBuilder<MultiProducerBuilder<HasConsumers, E, F, W>>;

/// Dependent-stage methods for an inner `*ProducerBuilder<HasConsumers, …>`.
macro_rules! impl_dependent {
    ($builder:ident, $($wait_bound:tt)*) => {
        impl<E, F, W> DependentBuilder<$builder<HasConsumers, E, F, W>>
        where
            E: Send + Sync + 'static,
            F: Fn() -> E + Send + Sync + 'static,
            W: WaitStrategy $($wait_bound)* + 'static,
        {
            /// Pin the next consumer thread to a CPU core (best-effort).
            #[must_use]
            pub fn pin_at_core(mut self, core_id: usize) -> Self {
                self.inner.shared.current_cpu_affinity = Some(core_id);
                self
            }

            /// Set the OS thread name for the next consumer.
            #[must_use]
            pub fn thread_name<S: Into<String>>(mut self, name: S) -> Self {
                self.inner.shared.current_thread_name = Some(name.into());
                self
            }

            /// Configure ring-buffer slot padding strategy.
            #[must_use]
            pub fn with_slot_padding(mut self, slot_padding: SlotPadding) -> Self {
                self.inner.shared.slot_padding = slot_padding;
                self
            }

            /// Enable 128-byte cache-line slot padding when `enabled` is true.
            #[must_use]
            pub fn with_cache_line_padding(self, enabled: bool) -> Self {
                self.with_slot_padding(if enabled {
                    SlotPadding::CacheLine128
                } else {
                    SlotPadding::None
                })
            }

            /// Register a mutable closure handler (`&mut E`) on this dependent stage.
            #[must_use]
            pub fn handle_events_with<H>(
                mut self,
                handler: H,
            ) -> $builder<HasConsumers, E, F, W>
            where
                H: FnMut(&mut E, i64, bool) + Send + Sync + 'static,
            {
                self.inner.shared.add_mut_closure(handler);
                self.inner
            }

            /// Register a mutable [`EventHandler`] on this dependent stage.
            #[must_use]
            pub fn handle_events_with_handler<H>(
                mut self,
                handler: H,
            ) -> $builder<HasConsumers, E, F, W>
            where
                H: EventHandler<E> + Send + Sync + 'static,
            {
                self.inner.shared.add_mut_handler(handler);
                self.inner
            }

            /// Register a read-only fan-out handler (`&E`) on this dependent stage.
            #[must_use]
            pub fn fan_out_events_with<H>(
                mut self,
                handler: H,
            ) -> $builder<HasConsumers, E, F, W>
            where
                H: FnMut(&E, i64, bool) + Send + 'static,
            {
                self.inner.shared.add_fanout(handler);
                self.inner
            }
        }
    };
}

impl_dependent!(SingleProducerBuilder, + Clone);
impl_dependent!(MultiProducerBuilder, + Clone);

// --- Cloneable multi-producer convenience --------------------------------------------

/// Cloneable multi-producer handle sharing one ring buffer and sequencer.
///
/// Each clone is an independent [`SimpleProducer`] handle (Arc clones only) for
/// concurrent publishing on the multi-producer sequencer.
#[derive(Clone)]
pub struct CloneableProducer<E, W>
where
    E: Send + Sync,
    W: WaitStrategy + 'static,
{
    producer: SimpleProducer<E, W>,
}

impl<E, W> CloneableProducer<E, W>
where
    E: Send + Sync,
    W: WaitStrategy + 'static,
{
    /// Create a cloneable producer from a shared ring buffer and sequencer.
    pub fn new(ring_buffer: Arc<RingBuffer<E>>, sequencer: SequencerEnum<W>) -> Self {
        Self {
            producer: SimpleProducer::new(ring_buffer, sequencer),
        }
    }

    /// Create a [`SimpleProducer`] handle (Arc-clones the ring buffer and sequencer).
    pub fn create_producer(&self) -> SimpleProducer<E, W> {
        self.producer.clone()
    }

    /// Try to publish one event; returns [`crate::disruptor::RingBufferFull`] if no slot is free.
    pub fn try_publish<F>(
        &self,
        update: F,
    ) -> std::result::Result<i64, crate::disruptor::producer::RingBufferFull>
    where
        F: FnOnce(&mut E),
    {
        self.producer.clone().try_publish(update)
    }

    /// Publish one event, spinning until a slot is available.
    pub fn publish<F>(&self, update: F)
    where
        F: FnOnce(&mut E),
    {
        self.producer.clone().publish(update);
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
        self.producer.clone().try_batch_publish(n, update)
    }

    /// Publish a batch of `n` events, spinning until space is available.
    pub fn batch_publish<F>(&self, n: usize, update: F)
    where
        F: for<'a> FnOnce(crate::disruptor::ring_buffer::BatchIterMut<'a, E>),
    {
        self.producer.clone().batch_publish(n, update);
    }
}
