//! `BadBatch` - High-Performance Disruptor Engine
//!
//! A complete Rust implementation of the LMAX Disruptor pattern for ultra-low latency
//! inter-thread messaging with mechanical sympathy.
//!
//! This library provides a faithful implementation of the LMAX Disruptor pattern,
//! following the original design from <https://github.com/LMAX-Exchange/disruptor>
//!
//! ## Features
//!
//! - **Lock-free**: Uses only atomic operations and memory barriers
//! - **Zero-allocation**: Pre-allocates all events during initialization
//! - **Mechanical sympathy**: CPU cache-friendly data structures
//! - **High throughput**: Batch processing and efficient algorithms
//! - **Low latency**: Minimal overhead and predictable performance
//! - **Thread-safe**: Full support for single and multi-producer scenarios
//!
//! ## Quick Start
//!
//! ```rust
//! use badbatch::disruptor::{
//!     Disruptor, ProducerType, BlockingWaitStrategy, DefaultEventFactory,
//!     EventHandler, Result,
//! };
//!
//! // Define your event type
//! #[derive(Debug, Default)]
//! struct MyEvent {
//!     value: i64,
//! }
//!
//! // Implement an event handler
//! struct MyEventHandler;
//!
//! impl EventHandler<MyEvent> for MyEventHandler {
//!     fn on_event(&mut self, event: &mut MyEvent, sequence: i64, end_of_batch: bool) -> Result<()> {
//!         println!("Processing event {} with value {}", sequence, event.value);
//!         Ok(())
//!     }
//! }
//!
//! // Create and configure the Disruptor
//! let factory = DefaultEventFactory::<MyEvent>::new();
//! let mut disruptor = Disruptor::new(
//!     factory,
//!     1024, // Buffer size (must be power of 2)
//!     ProducerType::Single,
//!     BlockingWaitStrategy::new(),
//! ).unwrap()
//! .handle_events_with(MyEventHandler)
//! .build();
//!
//! // Start the Disruptor
//! disruptor.start().unwrap();
//!
//! // Publish events...
//!
//! // Shutdown when done
//! disruptor.shutdown().unwrap();
//! ```
//!
//! ## Architecture
//!
//! The Disruptor pattern consists of several key components:
//!
//! - **`RingBuffer`**: Pre-allocated circular buffer for events
//! - **`Sequence`**: Atomic sequence counters for coordination
//! - **`Sequencer`**: Coordinates access to the ring buffer (single/multi producer)
//! - **`EventHandler`**: Processes events from the ring buffer
//! - **`EventProcessor`**: Manages the event processing loop
//! - **`WaitStrategy`**: Different strategies for waiting for events
//! - **`SequenceBarrier`**: Coordination point for dependencies
//! - **`Disruptor`**: Main DSL for configuring the system
//!
//! ## Performance Characteristics
//!
//! The Disruptor pattern is designed for ultra-low latency scenarios where
//! traditional queuing mechanisms introduce too much overhead. It achieves
//! high performance through:
//!
//! - Pre-allocation of all events to avoid garbage collection
//! - Lock-free algorithms using atomic operations
//! - Cache-friendly data structures with padding to avoid false sharing
//! - Batch processing to amortize coordination costs
//! - Mechanical sympathy with modern CPU architectures
//!
//! ## Event Sizing and Cache-Line Padding
//!
//! BadBatch stores events inline in a contiguous ring buffer (`Box<[UnsafeCell<T>]>`).
//! On modern x86_64/aarch64 the false-sharing granularity is typically **128 bytes**
//! (Sequence values already use `CachePadded` at 128B). Small events packed back-to-back
//! can still share a line: the producer writing slot N and the consumer reading slot N-1
//! may thrash coherency traffic (up to ~4x regression observed on Apple Silicon with
//! 32-byte events).
//!
//! For Builder API users, the easiest opt-in mitigation is 128-byte slot padding:
//!
//! ```rust
//! use badbatch::disruptor::{build_single_producer, BusySpinWaitStrategy};
//!
//! #[derive(Debug, Default)]
//! struct MyEvent {
//!     value: i64,
//!     payload: i64,
//! }
//!
//! let mut disruptor = build_single_producer(1024, MyEvent::default, BusySpinWaitStrategy)
//!     .with_cache_line_padding(true) // → SlotPadding::CacheLine128
//!     .handle_events_with(|_event, _sequence, _end_of_batch| {})
//!     .build();
//! # disruptor.shutdown();
//! ```
//!
//! Or align the event type itself:
//!
//! ```rust
//! #[repr(C, align(128))]
//! #[derive(Debug, Default)]
//! struct MyEvent {
//!     value: i64,
//!     payload: i64,
//! }
//! ```
//!
//! Default remains no slot padding (`SlotPadding::None`); measure before enabling.

#![allow(
    clippy::doc_markdown,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate
)]

pub mod disruptor;

// Re-export the main types for convenience
pub use disruptor::{
    // Utility functions
    is_power_of_two,
    BatchEventProcessor,
    BlockingWaitStrategy,
    BusySpinWaitStrategy,
    // Convenience types
    DefaultEventFactory,

    // Core types
    Disruptor,
    // Error types
    DisruptorError,
    EventFactory,
    // Event handling
    EventHandler,
    // Event processing
    EventProcessor,
    EventTranslator,
    EventTranslatorOneArg,
    EventTranslatorThreeArg,

    EventTranslatorTwoArg,
    // Exception handling
    ExceptionHandler,

    MultiProducerSequencer,
    ProducerType,

    Result,

    RingBuffer,
    Sequence,

    SequenceBarrier,

    // Sequencing
    Sequencer,
    SingleProducerSequencer,
    SleepingWaitStrategy,

    SlotPadding,
    // Wait strategies
    WaitStrategy,
    YieldingWaitStrategy,
    // Constants
    INITIAL_CURSOR_VALUE,
};

/// Version information
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Get the version of the `BadBatch` library
#[must_use]
pub fn version() -> &'static str {
    VERSION
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version() {
        assert!(!version().is_empty());
    }
}
