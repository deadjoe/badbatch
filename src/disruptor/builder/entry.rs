//! Public entry points: build_single_producer / build_multi_producer.

use super::consumer::NoConsumers;
use super::fluent::{MultiProducerBuilder, SingleProducerBuilder};
use crate::disruptor::WaitStrategy;

/// Build a single producer Disruptor
///
/// This follows the disruptor-rs pattern for creating single producer Disruptors.
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
///     .handle_events_with(|_event, _sequence, _end_of_batch| {
///         // Process event
///     })
///     .build();
/// # drop(producer); // Clean shutdown
/// ```
pub fn build_single_producer<E, F, W>(
    size: usize,
    event_factory: F,
    wait_strategy: W,
) -> SingleProducerBuilder<NoConsumers, E, F, W>
where
    E: Send + Sync + 'static,
    F: Fn() -> E + Send + Sync + 'static,
    W: WaitStrategy + 'static,
{
    SingleProducerBuilder::new(size, event_factory, wait_strategy)
}

/// Build a multi producer Disruptor
///
/// This follows the disruptor-rs pattern for creating multi producer Disruptors.
///
/// # Examples
///
/// ```rust
/// use badbatch::disruptor::{build_multi_producer, BusySpinWaitStrategy};
///
/// #[derive(Default, Clone)]
/// struct MyEvent { value: i32 }
///
/// let mut producer1 = build_multi_producer(64, MyEvent::default, BusySpinWaitStrategy)
///     .handle_events_with(|_event, _sequence, _end_of_batch| {
///         // Process event in processor1
///     })
///     .handle_events_with(|_event, _sequence, _end_of_batch| {
///         // Process event in processor2
///     })
///     .build();
/// # drop(producer1); // Clean shutdown
/// // let mut producer2 = producer1.clone(); // After drop, cannot clone
/// ```
pub fn build_multi_producer<E, F, W>(
    size: usize,
    event_factory: F,
    wait_strategy: W,
) -> MultiProducerBuilder<NoConsumers, E, F, W>
where
    E: Send + Sync + 'static,
    F: Fn() -> E + Send + Sync + 'static,
    W: WaitStrategy + Clone + 'static,
{
    MultiProducerBuilder::new(size, event_factory, wait_strategy)
}
