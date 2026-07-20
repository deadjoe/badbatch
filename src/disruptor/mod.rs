//! `BadBatch` Disruptor Implementation
//!
//! This module provides a complete Rust implementation of the LMAX Disruptor pattern,
//! strictly following the original design from <https://github.com/LMAX-Exchange/disruptor>
//!
//! ## Core Architecture
//!
//! The Disruptor pattern consists of the following key components:
//!
//! 1. **`RingBuffer`** - Pre-allocated circular buffer for events
//! 2. **`Sequence`** - Atomic sequence counters for coordination
//! 3. **`Sequencer`** - Coordinates access to the ring buffer (single/multi producer)
//! 4. **`EventHandler`** - Processes events from the ring buffer
//! 5. **`EventProcessor`** - Manages the event processing loop
//! 6. **`WaitStrategy`** - Different strategies for waiting for events
//! 7. **`SequenceBarrier`** - Coordination point for dependencies
//! 8. **`Disruptor`** - Main DSL for configuring the system
//!
//! ## Design Principles
//!
//! - **Lock-free**: Uses only atomic operations and memory barriers
//! - **Zero-allocation**: Pre-allocates all events during initialization
//! - **Mechanical sympathy**: CPU cache-friendly data structures
//! - **High throughput**: Batch processing and efficient algorithms
//! - **Low latency**: Minimal overhead and predictable performance

// Core sequence management
pub mod sequence;

// Core interfaces
pub mod core_interfaces;

// Ring buffer for event storage
pub mod ring_buffer;

// Event handling interfaces
pub mod event_factory;
pub mod event_handler;
pub mod event_translator;

// Sequencer implementations
pub mod sequencer;

// Wait strategies
pub mod wait_strategy;

// Simplified wait strategies (inspired by disruptor-rs)
pub mod simple_wait_strategy;

// Compatibility macros for the pre-0.2 internal logging surface.
#[doc(hidden)]
pub mod internal_log;

// Structured pipeline failure context
mod failure;

// Event processing
pub mod event_processor;

// Unified sequential + WorkerPool consumer loops
pub mod consumer_engine;

// Sequence barriers for coordination
pub mod sequence_barrier;

// Exception handling
pub mod exception_handler;

// Producer types
pub mod producer_type;

// Producer API (inspired by disruptor-rs)
pub mod producer;

// Builder API (inspired by disruptor-rs)
pub mod builder;

// Thread management and CPU affinity (inspired by disruptor-rs)
pub mod thread_management;

// Elegant consumer handling (inspired by disruptor-rs) — optional extras surface
#[cfg(feature = "extras")]
pub mod elegant_consumer;

// User-driven event polling (inspired by disruptor-rs)
pub mod event_poller;

// Main Disruptor DSL (LMAX-style) — available under `lmax-dsl` feature (default on)
#[cfg(feature = "lmax-dsl")]
#[allow(clippy::module_inception)]
pub mod disruptor;

// Re-export core types for convenience
pub use core_interfaces::DataProvider;
#[cfg(feature = "lmax-dsl")]
pub use disruptor::Disruptor;
pub use event_factory::{DefaultEventFactory, EventFactory};
pub use event_handler::{ClosureEventHandler, EventHandler, NoOpEventHandler};
pub use event_processor::{BatchEventProcessor, EventProcessor};
pub use event_translator::{
    EventTranslator, EventTranslatorOneArg, EventTranslatorThreeArg, EventTranslatorTwoArg,
};
pub use exception_handler::{DefaultExceptionHandler, ErrorDecision, ExceptionHandler};
pub use failure::{FailurePhase, FailureRecord};
pub use producer_type::ProducerType;
pub use ring_buffer::{RingBuffer, SlotPadding};
pub use sequence::Sequence;
pub use sequence_barrier::{ProcessingSequenceBarrier, SequenceBarrier};
#[cfg(feature = "bench-round-diagnostics")]
#[doc(hidden)]
pub use sequencer::{configure_bench_round_diagnostics, BenchProducerBackpressureSnapshot};
pub use sequencer::{MultiProducerSequencer, Sequencer, SequencerEnum, SingleProducerSequencer};
pub use wait_strategy::{
    BlockingWaitStrategy, BusySpinWaitStrategy, SleepingWaitStrategy, WaitStrategy,
    YieldingWaitStrategy,
};

// Preferred public path: monomorphized Builder + Producer + EventPoller
pub use builder::{build_multi_producer, build_single_producer, CloneableProducer};
#[cfg(feature = "extras")]
pub use elegant_consumer::ElegantConsumer;
pub use event_poller::{open_single_producer_poller, EventBatch, EventPoller, Polling};
pub use producer::{MissingFreeSlots, Producer, RingBufferFull, SimpleProducer, TryPublishError};

/// The initial cursor value for sequences (matches LMAX Disruptor)
pub const INITIAL_CURSOR_VALUE: i64 = -1;

/// Errors that can occur in the Disruptor
#[derive(Debug, thiserror::Error)]
pub enum DisruptorError {
    /// Returned when a producer attempts to publish but the ring buffer is full.
    #[error("Ring buffer is full")]
    BufferFull,

    /// Returned when a sequence number outside the valid range is used.
    #[error("Invalid sequence: {0}")]
    InvalidSequence(i64),

    /// Returned when an invalid (non power-of-two) buffer size is provided.
    #[error("Buffer size must be a power of 2, got: {0}")]
    InvalidBufferSize(usize),

    /// Returned when a blocking call times out waiting for a sequence.
    #[error("Timeout waiting for sequence")]
    Timeout,

    /// Returned when an operation is attempted after the disruptor shuts down.
    #[error("Disruptor has been shut down")]
    Shutdown,

    /// Returned when the requested capacity exceeds available slots.
    #[error("Insufficient capacity")]
    InsufficientCapacity,

    /// Returned when the disruptor is alerted to stop processing.
    #[error("Alert exception")]
    Alert,

    /// Returned when start() or run() is called on an already-running component.
    #[error("Already running")]
    AlreadyRunning,

    /// Returned when shutdown encounters an unrecoverable error.
    #[error("Shutdown error: {0}")]
    ShutdownError(String),

    /// Returned when a fatal consumer/producer failure poisons the pipeline.
    /// Publishing fails instead of spinning forever on a gating sequence that
    /// will never advance (or exposing a never-written slot). Built-in runtime
    /// handles expose the causal [`FailureRecord`] through `first_failure()`.
    #[error("Pipeline poisoned by a fatal producer or consumer failure")]
    Poisoned,

    /// A second thread attempted to drive claim methods on a
    /// [`SingleProducerSequencer`] while another claim was in progress.
    ///
    /// Single-producer sequencers store claim state in non-atomic cells; the
    /// runtime claim lock rejects concurrent drivers instead of data-racing
    /// (residual closure after the 2026-07-18 soundness audit).
    #[error("Concurrent claim on a single-producer sequencer")]
    ConcurrentClaimDriver,

    /// A single-producer DSL publish re-entered `publish_event` /
    /// `try_publish_event` from inside a translator callback while the claim
    /// lock was already held. Nested publishes would disorder the cursor and
    /// previously deadlocked on a non-reentrant mutex.
    #[error("Reentrant publish on a single-producer DSL disruptor")]
    ReentrantPublish,
}

/// Result type for Disruptor operations
pub type Result<T> = std::result::Result<T, DisruptorError>;

/// Utility function to check if a number is a power of 2
#[must_use]
pub fn is_power_of_two(n: usize) -> bool {
    n.is_power_of_two()
}

#[cfg(test)]
pub mod property_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_power_of_two() {
        assert!(is_power_of_two(1));
        assert!(is_power_of_two(2));
        assert!(is_power_of_two(4));
        assert!(is_power_of_two(8));
        assert!(is_power_of_two(1024));

        assert!(!is_power_of_two(0));
        assert!(!is_power_of_two(3));
        assert!(!is_power_of_two(5));
        assert!(!is_power_of_two(1023));
    }
}
