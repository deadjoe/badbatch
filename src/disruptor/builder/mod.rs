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
mod tests;

pub use consumer::{Consumer, HasConsumers, NoConsumers};
// DisruptorCore / create_disruptor_core are no longer re-exported (soundness
// audit 2026-07-18): a public core allowed unlimited create_producer() over a
// single-producer sequencer. In-crate users path them via builder::core.
pub use entry::{build_multi_producer, build_single_producer};
pub use fluent::{CloneableProducer, MultiProducerBuilder, SingleProducerBuilder};
pub use handle::{DisruptorHandle, MultiProducerMode, SingleProducerMode};

pub use fluent::{DependentConsumerBuilder, DependentMultiConsumerBuilder};
