//! Builder module for creating Disruptors with a fluent API.
//!
//! Hot-path event loops live in [`crate::disruptor::consumer_engine`]; this module
//! only assembles topology, starts threads, and exposes the type-state Builder API.

mod consumer;
mod core;
mod entry;
mod fluent;
mod handle;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

pub use consumer::{Consumer, HasConsumers, NoConsumers};
pub use core::{create_disruptor_core, DisruptorCore};
pub use entry::{build_multi_producer, build_single_producer};
pub use fluent::{CloneableProducer, MultiProducerBuilder, SingleProducerBuilder};
pub use handle::DisruptorHandle;

// Re-export DependentConsumerBuilder if public
pub use fluent::DependentConsumerBuilder;
