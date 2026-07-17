//! Type-state fluent builders (single / multi / dependent stages).

use super::consumer::{
    assert_stage_mode_compatible, consumer_info_from_handler, consumer_info_from_readonly,
    ConsumerInfo, HasConsumers, NoConsumers,
};
use super::core::create_disruptor_core;
use super::handle::DisruptorHandle;
use crate::disruptor::{
    producer::{Producer, SimpleProducer},
    ring_buffer::SlotPadding,
    sequencer::SequencerEnum,
    EventHandler, RingBuffer, WaitStrategy,
};
use std::marker::PhantomData;
use std::sync::Arc;

/// Shared state for building disruptors
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

/// Builder for single producer Disruptors
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

/// Builder for dependent consumers (created by and_then())
pub struct DependentConsumerBuilder<E, F, W>
where
    E: Send + Sync + 'static,
    F: Fn() -> E + Send + Sync + 'static,
    W: WaitStrategy + Clone + 'static,
{
    builder: SingleProducerBuilder<HasConsumers, E, F, W>,
}

impl<State, E, F, W> SingleProducerBuilder<State, E, F, W>
where
    E: Send + Sync + 'static,
    F: Fn() -> E + Send + Sync + 'static,
    W: WaitStrategy + 'static,
{
    /// Configure the physical ring slot padding strategy.
    ///
    /// This changes memory layout only; it does not affect Disruptor semantics.
    #[must_use]
    pub fn with_slot_padding(mut self, slot_padding: SlotPadding) -> Self {
        self.shared.slot_padding = slot_padding;
        self
    }

    /// Enable or disable 128-byte cache-line padding for ring slots.
    ///
    /// Maps to [`SlotPadding::CacheLine128`] when enabled — modern false-sharing
    /// granularity on x86_64/aarch64 (matches `Sequence` / `CachePadded`). Use
    /// [`Self::with_slot_padding`] for explicit `CacheLine64` if needed.
    ///
    /// Useful for small events in high-throughput unicast or pipeline topologies
    /// where adjacent inline slots would otherwise share cache lines.
    #[must_use]
    pub fn with_cache_line_padding(self, enabled: bool) -> Self {
        self.with_slot_padding(if enabled {
            SlotPadding::CacheLine128
        } else {
            SlotPadding::None
        })
    }
}

impl<State, E, F, W> MultiProducerBuilder<State, E, F, W>
where
    E: Send + Sync + 'static,
    F: Fn() -> E + Send + Sync + 'static,
    W: WaitStrategy + Clone + 'static,
{
    /// Configure the physical ring slot padding strategy.
    ///
    /// This changes memory layout only; it does not affect Disruptor semantics.
    #[must_use]
    pub fn with_slot_padding(mut self, slot_padding: SlotPadding) -> Self {
        self.shared.slot_padding = slot_padding;
        self
    }

    /// Enable or disable 128-byte cache-line padding for ring slots.
    ///
    /// Maps to [`SlotPadding::CacheLine128`] when enabled. See single-producer docs.
    #[must_use]
    pub fn with_cache_line_padding(self, enabled: bool) -> Self {
        self.with_slot_padding(if enabled {
            SlotPadding::CacheLine128
        } else {
            SlotPadding::None
        })
    }
}

impl<E, F, W> DependentConsumerBuilder<E, F, W>
where
    E: Send + Sync + 'static,
    F: Fn() -> E + Send + Sync + 'static,
    W: WaitStrategy + Clone + 'static,
{
    /// Set CPU core affinity for the next event handler
    #[must_use]
    pub fn pin_at_core(mut self, core_id: usize) -> Self {
        self.builder.shared.current_cpu_affinity = Some(core_id);
        self
    }

    /// Set thread name for the next event handler
    #[must_use]
    pub fn thread_name<S: Into<String>>(mut self, name: S) -> Self {
        self.builder.shared.current_thread_name = Some(name.into());
        self
    }

    /// Configure the physical ring slot padding strategy.
    #[must_use]
    pub fn with_slot_padding(mut self, slot_padding: SlotPadding) -> Self {
        self.builder.shared.slot_padding = slot_padding;
        self
    }

    /// Enable or disable 128-byte cache-line padding for ring slots.
    #[must_use]
    pub fn with_cache_line_padding(self, enabled: bool) -> Self {
        self.with_slot_padding(if enabled {
            SlotPadding::CacheLine128
        } else {
            SlotPadding::None
        })
    }

    /// Add a dependent event handler that waits for previous consumers (closure-based)
    #[must_use]
    pub fn handle_events_with<H>(
        mut self,
        handler: H,
    ) -> SingleProducerBuilder<HasConsumers, E, F, W>
    where
        H: FnMut(&mut E, i64, bool) + Send + Sync + 'static,
    {
        let consumer_info = consumer_info_from_handler(
            ClosureEventHandler::new(handler),
            self.builder.shared.current_thread_name.take(),
            self.builder.shared.current_cpu_affinity.take(),
            self.builder.shared.current_stage_index,
        );
        self.builder.shared.consumers.push(consumer_info);
        self.builder.shared.current_stage_width += 1;
        self.builder
    }

    /// Add a dependent stateful event handler that waits for previous consumers
    #[must_use]
    pub fn handle_events_with_handler<H>(
        mut self,
        handler: H,
    ) -> SingleProducerBuilder<HasConsumers, E, F, W>
    where
        H: EventHandler<E> + Send + Sync + 'static,
    {
        let consumer_info = consumer_info_from_handler(
            handler,
            self.builder.shared.current_thread_name.take(),
            self.builder.shared.current_cpu_affinity.take(),
            self.builder.shared.current_stage_index,
        );
        self.builder.shared.consumers.push(consumer_info);
        self.builder.shared.current_stage_width += 1;
        self.builder
    }

    /// Build the Disruptor and return a DisruptorHandle
    pub fn build(self) -> DisruptorHandle<E, W> {
        self.builder.build()
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
            shared: SharedBuilderState {
                size,
                slot_padding: SlotPadding::None,
                wait_strategy,
                consumers: Vec::new(),
                current_thread_name: None,
                current_cpu_affinity: None,
                current_stage_index: 0,
                current_stage_width: 0,
            },
            _phantom: PhantomData,
        }
    }

    /// Set CPU core affinity for the next event handler
    #[must_use]
    pub fn pin_at_core(mut self, core_id: usize) -> Self {
        self.shared.current_cpu_affinity = Some(core_id);
        self
    }

    /// Set thread name for the next event handler
    #[must_use]
    pub fn thread_name<S: Into<String>>(mut self, name: S) -> Self {
        self.shared.current_thread_name = Some(name.into());
        self
    }

    /// Add an event handler to process events (closure-based)
    ///
    /// This follows the disruptor-rs pattern for adding event handlers.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use badbatch::disruptor::{build_single_producer, BusySpinWaitStrategy};
    ///
    /// #[derive(Default)]
    /// struct MyEvent { value: i32 }
    ///
    /// let mut producer = build_single_producer(8, MyEvent::default, BusySpinWaitStrategy)
    ///     .handle_events_with(|event, sequence, end_of_batch| {
    ///         // Process the event
    ///         event.value += 1;
    ///         println!("Processed event {} at sequence {}", event.value, sequence);
    ///     })
    ///     .build();
    /// # drop(producer); // Clean shutdown
    /// ```
    #[must_use]
    pub fn handle_events_with<H>(
        mut self,
        handler: H,
    ) -> SingleProducerBuilder<HasConsumers, E, F, W>
    where
        H: FnMut(&mut E, i64, bool) + Send + Sync + 'static,
    {
        let consumer_info = consumer_info_from_handler(
            ClosureEventHandler::new(handler),
            self.shared.current_thread_name.take(),
            self.shared.current_cpu_affinity.take(),
            self.shared.current_stage_index,
        );
        self.shared.consumers.push(consumer_info);
        self.shared.current_stage_width = 1;

        SingleProducerBuilder {
            event_factory: self.event_factory,
            shared: self.shared,
            _phantom: PhantomData,
        }
    }

    /// Add a stateful event handler to process events
    ///
    /// This method accepts any type that implements the EventHandler trait,
    /// allowing for stateful event processing.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use badbatch::disruptor::{build_single_producer, BusySpinWaitStrategy, EventHandler, Result};
    ///
    /// #[derive(Default)]
    /// struct MyEvent {
    ///     value: i32,
    ///     processed_count: usize,
    /// }
    ///
    /// struct MyEventHandler {
    ///     counter: usize,
    /// }
    ///
    /// impl EventHandler<MyEvent> for MyEventHandler {
    ///     fn on_event(&mut self, event: &mut MyEvent, sequence: i64, _end_of_batch: bool) -> Result<()> {
    ///         self.counter += 1;
    ///         event.processed_count = self.counter;
    ///         Ok(())
    ///     }
    /// }
    ///
    /// let mut producer = build_single_producer(8, MyEvent::default, BusySpinWaitStrategy)
    ///     .handle_events_with_handler(MyEventHandler { counter: 0 })
    ///     .build();
    /// # drop(producer); // Clean shutdown
    /// ```
    #[must_use]
    pub fn handle_events_with_handler<H>(
        mut self,
        handler: H,
    ) -> SingleProducerBuilder<HasConsumers, E, F, W>
    where
        H: EventHandler<E> + Send + Sync + 'static,
    {
        let consumer_info = consumer_info_from_handler(
            handler,
            self.shared.current_thread_name.take(),
            self.shared.current_cpu_affinity.take(),
            self.shared.current_stage_index,
        );
        self.shared.consumers.push(consumer_info);
        self.shared.current_stage_width = 1;

        SingleProducerBuilder {
            event_factory: self.event_factory,
            shared: self.shared,
            _phantom: PhantomData,
        }
    }

    /// Start a **read-only fan-out** stage (`&E`): every fan-out consumer sees every event.
    ///
    /// Add more fan-out consumers with [`SingleProducerBuilder<HasConsumers, _, _, _>::fan_out_events_with`].
    /// Do not mix with mutable [`Self::handle_events_with`] on the same stage.
    #[must_use]
    pub fn fan_out_events_with<H>(
        mut self,
        mut handler: H,
    ) -> SingleProducerBuilder<HasConsumers, E, F, W>
    where
        H: FnMut(&E, i64, bool) + Send + 'static,
    {
        let consumer_info = consumer_info_from_readonly(
            move |e, seq, eob| {
                handler(e, seq, eob);
                Ok(())
            },
            self.shared.current_thread_name.take(),
            self.shared.current_cpu_affinity.take(),
            self.shared.current_stage_index,
        );
        self.shared.consumers.push(consumer_info);
        self.shared.current_stage_width = 1;

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
    /// Set CPU core affinity for the next event handler
    #[must_use]
    pub fn pin_at_core(mut self, core_id: usize) -> Self {
        self.shared.current_cpu_affinity = Some(core_id);
        self
    }

    /// Set thread name for the next event handler
    #[must_use]
    pub fn thread_name<S: Into<String>>(mut self, name: S) -> Self {
        self.shared.current_thread_name = Some(name.into());
        self
    }

    /// Add another **mutable** event handler on this stage (WorkerPool work-sharing when width > 1).
    #[must_use]
    pub fn handle_events_with<H>(mut self, handler: H) -> Self
    where
        H: FnMut(&mut E, i64, bool) + Send + Sync + 'static,
    {
        assert_stage_mode_compatible(
            &self.shared.consumers,
            self.shared.current_stage_index,
            false,
        );
        let consumer_info = consumer_info_from_handler(
            ClosureEventHandler::new(handler),
            self.shared.current_thread_name.take(),
            self.shared.current_cpu_affinity.take(),
            self.shared.current_stage_index,
        );
        self.shared.consumers.push(consumer_info);
        self.shared.current_stage_width += 1;
        self
    }

    /// Add another stateful mutable handler (WorkerPool when width > 1).
    #[must_use]
    pub fn handle_events_with_handler<H>(mut self, handler: H) -> Self
    where
        H: EventHandler<E> + Send + Sync + 'static,
    {
        assert_stage_mode_compatible(
            &self.shared.consumers,
            self.shared.current_stage_index,
            false,
        );
        let consumer_info = consumer_info_from_handler(
            handler,
            self.shared.current_thread_name.take(),
            self.shared.current_cpu_affinity.take(),
            self.shared.current_stage_index,
        );
        self.shared.consumers.push(consumer_info);
        self.shared.current_stage_width += 1;
        self
    }

    /// Add a **read-only fan-out** consumer on this stage (`&E`).
    ///
    /// Every fan-out consumer observes **every** sequence (broadcast), unlike
    /// [`Self::handle_events_with`] which work-shares via WorkerPool CAS claim.
    /// Cannot be mixed with mutable handlers on the same stage.
    #[must_use]
    pub fn fan_out_events_with<H>(mut self, mut handler: H) -> Self
    where
        H: FnMut(&E, i64, bool) + Send + 'static,
    {
        assert_stage_mode_compatible(
            &self.shared.consumers,
            self.shared.current_stage_index,
            true,
        );
        let consumer_info = consumer_info_from_readonly(
            move |e, seq, eob| {
                handler(e, seq, eob);
                Ok(())
            },
            self.shared.current_thread_name.take(),
            self.shared.current_cpu_affinity.take(),
            self.shared.current_stage_index,
        );
        self.shared.consumers.push(consumer_info);
        self.shared.current_stage_width += 1;
        self
    }

    /// Advance to the next processing stage.
    ///
    /// All consumers added after this call will wait for the entire current stage
    /// to complete before they start processing.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use badbatch::disruptor::{build_single_producer, BusySpinWaitStrategy};
    ///
    /// #[derive(Default)]
    /// struct MyEvent { value: i32 }
    ///
    /// let mut producer = build_single_producer(8, MyEvent::default, BusySpinWaitStrategy)
    ///     .handle_events_with(|event, _sequence, _end_of_batch| {
    ///         // First consumer processes events
    ///         event.value += 1;
    ///     })
    ///     .and_then()
    ///     .handle_events_with(|event, _sequence, _end_of_batch| {
    ///         // Second consumer waits for first consumer to complete
    ///         event.value *= 2;
    ///     })
    ///     .build();
    /// # drop(producer); // Clean shutdown
    /// ```
    pub fn and_then(mut self) -> DependentConsumerBuilder<E, F, W> {
        self.shared.current_stage_index += 1;
        self.shared.current_stage_width = 0;

        DependentConsumerBuilder { builder: self }
    }

    /// Build the Disruptor and return a DisruptorHandle
    ///
    /// This creates the `RingBuffer`, `Sequencer`, and starts all event processors.
    pub fn build(self) -> DisruptorHandle<E, W> {
        // Use the unified factory function
        let core = create_disruptor_core(
            self.shared.size,
            self.event_factory,
            self.shared.wait_strategy,
            self.shared.consumers,
            false, // Single producer
            self.shared.slot_padding,
        );

        DisruptorHandle::new(core)
    }
}

/// Builder for multi producer Disruptors
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
            shared: SharedBuilderState {
                size,
                slot_padding: SlotPadding::None,
                wait_strategy,
                consumers: Vec::new(),
                current_thread_name: None,
                current_cpu_affinity: None,
                current_stage_index: 0,
                current_stage_width: 0,
            },
            _phantom: PhantomData,
        }
    }

    /// Set CPU core affinity for the next event handler
    #[must_use]
    pub fn pin_at_core(mut self, core_id: usize) -> Self {
        self.shared.current_cpu_affinity = Some(core_id);
        self
    }

    /// Set thread name for the next event handler
    #[must_use]
    pub fn thread_name<S: Into<String>>(mut self, name: S) -> Self {
        self.shared.current_thread_name = Some(name.into());
        self
    }

    /// Add an event handler to process events
    #[must_use]
    pub fn handle_events_with<H>(
        mut self,
        handler: H,
    ) -> MultiProducerBuilder<HasConsumers, E, F, W>
    where
        H: FnMut(&mut E, i64, bool) + Send + Sync + 'static,
    {
        let consumer_info = consumer_info_from_handler(
            ClosureEventHandler::new(handler),
            self.shared.current_thread_name.take(),
            self.shared.current_cpu_affinity.take(),
            self.shared.current_stage_index,
        );
        self.shared.consumers.push(consumer_info);
        self.shared.current_stage_width = 1;

        MultiProducerBuilder {
            event_factory: self.event_factory,
            shared: self.shared,
            _phantom: PhantomData,
        }
    }

    /// Add a stateful event handler to process events
    #[must_use]
    pub fn handle_events_with_handler<H>(
        mut self,
        handler: H,
    ) -> MultiProducerBuilder<HasConsumers, E, F, W>
    where
        H: EventHandler<E> + Send + Sync + 'static,
    {
        let consumer_info = consumer_info_from_handler(
            handler,
            self.shared.current_thread_name.take(),
            self.shared.current_cpu_affinity.take(),
            self.shared.current_stage_index,
        );
        self.shared.consumers.push(consumer_info);
        self.shared.current_stage_width = 1;

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
    /// Set CPU core affinity for the next event handler
    #[must_use]
    pub fn pin_at_core(mut self, core_id: usize) -> Self {
        self.shared.current_cpu_affinity = Some(core_id);
        self
    }

    /// Set thread name for the next event handler
    #[must_use]
    pub fn thread_name<S: Into<String>>(mut self, name: S) -> Self {
        self.shared.current_thread_name = Some(name.into());
        self
    }

    /// Add another mutable handler (WorkerPool work-sharing when width > 1).
    #[must_use]
    pub fn handle_events_with<H>(mut self, handler: H) -> Self
    where
        H: FnMut(&mut E, i64, bool) + Send + Sync + 'static,
    {
        assert_stage_mode_compatible(
            &self.shared.consumers,
            self.shared.current_stage_index,
            false,
        );
        let consumer_info = consumer_info_from_handler(
            ClosureEventHandler::new(handler),
            self.shared.current_thread_name.take(),
            self.shared.current_cpu_affinity.take(),
            self.shared.current_stage_index,
        );
        self.shared.consumers.push(consumer_info);
        self.shared.current_stage_width += 1;
        self
    }

    /// Add another stateful mutable handler (WorkerPool when width > 1).
    #[must_use]
    pub fn handle_events_with_handler<H>(mut self, handler: H) -> Self
    where
        H: EventHandler<E> + Send + Sync + 'static,
    {
        assert_stage_mode_compatible(
            &self.shared.consumers,
            self.shared.current_stage_index,
            false,
        );
        let consumer_info = consumer_info_from_handler(
            handler,
            self.shared.current_thread_name.take(),
            self.shared.current_cpu_affinity.take(),
            self.shared.current_stage_index,
        );
        self.shared.consumers.push(consumer_info);
        self.shared.current_stage_width += 1;
        self
    }

    /// Read-only fan-out (`&E`): every consumer on this stage sees every sequence.
    #[must_use]
    pub fn fan_out_events_with<H>(mut self, mut handler: H) -> Self
    where
        H: FnMut(&E, i64, bool) + Send + 'static,
    {
        assert_stage_mode_compatible(
            &self.shared.consumers,
            self.shared.current_stage_index,
            true,
        );
        let consumer_info = consumer_info_from_readonly(
            move |e, seq, eob| {
                handler(e, seq, eob);
                Ok(())
            },
            self.shared.current_thread_name.take(),
            self.shared.current_cpu_affinity.take(),
            self.shared.current_stage_index,
        );
        self.shared.consumers.push(consumer_info);
        self.shared.current_stage_width += 1;
        self
    }

    /// Build the Disruptor and return a DisruptorHandle
    ///
    /// This creates the `RingBuffer`, `MultiProducerSequencer`, and starts all event processors.
    pub fn build(self) -> DisruptorHandle<E, W> {
        // Use the unified factory function to create the core components and start consumer threads
        let core = create_disruptor_core(
            self.shared.size,
            self.event_factory,
            self.shared.wait_strategy,
            self.shared.consumers,
            true, // Multi producer
            self.shared.slot_padding,
        );

        DisruptorHandle::new(core)
    }
}

/// Wrapper for SimpleProducer that can be cloned for multi-producer scenarios
///
/// This implementation now properly coordinates multiple producers using a shared sequencer.
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
    /// Creates a new CloneableProducer with the given ring buffer and sequencer
    ///
    /// This constructor allows creating custom producer instances from existing components
    pub fn new(ring_buffer: Arc<RingBuffer<E>>, sequencer: SequencerEnum<W>) -> Self {
        Self {
            ring_buffer,
            sequencer,
        }
    }

    /// Create a new SimpleProducer for this thread
    ///
    /// Each thread should call this to get its own producer instance.
    /// This avoids the lifetime issues with shared mutable access.
    pub fn create_producer(&self) -> SimpleProducer<E, W> {
        SimpleProducer::new(self.ring_buffer.clone(), self.sequencer.clone())
    }
}

impl<E, W> CloneableProducer<E, W>
where
    E: Send + Sync,
    W: WaitStrategy + 'static,
{
    /// Publish an event using a closure (convenience API).
    ///
    /// **Performance note:** This creates a temporary `SimpleProducer` on each call,
    /// incurring 2 Arc refcount increments + decrements. For hot paths, call
    /// [`create_producer()`](Self::create_producer) once and reuse the returned
    /// `SimpleProducer` directly.
    pub fn try_publish<F>(
        &self,
        update: F,
    ) -> std::result::Result<i64, crate::disruptor::producer::RingBufferFull>
    where
        F: FnOnce(&mut E),
    {
        let mut producer = self.create_producer();
        producer.try_publish(update)
    }

    /// Publish an event, spinning until space is available (convenience API).
    ///
    /// **Performance note:** This creates a temporary `SimpleProducer` on each call.
    /// For hot paths, call [`create_producer()`](Self::create_producer) once and
    /// reuse the returned `SimpleProducer` directly.
    pub fn publish<F>(&self, update: F)
    where
        F: FnOnce(&mut E),
    {
        let mut producer = self.create_producer();
        producer.publish(update);
    }

    /// Try to publish a batch of events (convenience API).
    ///
    /// **Performance note:** This creates a temporary `SimpleProducer` on each call.
    /// For hot paths, call [`create_producer()`](Self::create_producer) once and
    /// reuse the returned `SimpleProducer` directly.
    pub fn try_batch_publish<F>(
        &self,
        n: usize,
        update: F,
    ) -> std::result::Result<i64, crate::disruptor::producer::MissingFreeSlots>
    where
        F: for<'a> FnOnce(crate::disruptor::ring_buffer::BatchIterMut<'a, E>),
    {
        let mut producer = self.create_producer();
        producer.try_batch_publish(n, update)
    }

    /// Publish a batch of events, spinning until space is available (convenience API).
    ///
    /// **Performance note:** This creates a temporary `SimpleProducer` on each call.
    /// For hot paths, call [`create_producer()`](Self::create_producer) once and
    /// reuse the returned `SimpleProducer` directly.
    pub fn batch_publish<F>(&self, n: usize, update: F)
    where
        F: for<'a> FnOnce(crate::disruptor::ring_buffer::BatchIterMut<'a, E>),
    {
        let mut producer = self.create_producer();
        producer.batch_publish(n, update);
    }
}

/// Wrapper for closures to implement EventHandler trait
pub(crate) struct ClosureEventHandler<E, F>
where
    E: Send + Sync,
    F: FnMut(&mut E, i64, bool) + Send + Sync,
{
    handler: F,
    _phantom: PhantomData<E>,
}

impl<E, F> ClosureEventHandler<E, F>
where
    E: Send + Sync,
    F: FnMut(&mut E, i64, bool) + Send + Sync,
{
    pub(crate) fn new(handler: F) -> Self {
        Self {
            handler,
            _phantom: PhantomData,
        }
    }
}

impl<E, F> EventHandler<E> for ClosureEventHandler<E, F>
where
    E: Send + Sync,
    F: FnMut(&mut E, i64, bool) + Send + Sync,
{
    fn on_event(
        &mut self,
        event: &mut E,
        sequence: i64,
        end_of_batch: bool,
    ) -> crate::disruptor::Result<()> {
        (self.handler)(event, sequence, end_of_batch);
        Ok(())
    }
}
