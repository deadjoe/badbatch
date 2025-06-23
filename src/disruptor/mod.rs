//! BadBatch Disruptor Implementation
//!
//! This module provides a complete Rust implementation of the LMAX Disruptor pattern,
//! strictly following the original design from <https://github.com/LMAX-Exchange/disruptor>
//!
//! ## Core Architecture
//!
//! The Disruptor pattern consists of the following key components:
//!
//! 1. **RingBuffer** - Pre-allocated circular buffer for events
//! 2. **Sequence** - Atomic sequence counters for coordination
//! 3. **Sequencer** - Coordinates access to the ring buffer (single/multi producer)
//! 4. **EventHandler** - Processes events from the ring buffer
//! 5. **EventProcessor** - Manages the event processing loop
//! 6. **WaitStrategy** - Different strategies for waiting for events
//! 7. **SequenceBarrier** - Coordination point for dependencies
//! 8. **Disruptor** - Main DSL for configuring the system
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

// Event processing
pub mod event_processor;

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

// Elegant consumer handling (inspired by disruptor-rs)
pub mod elegant_consumer;

// Main Disruptor DSL
#[allow(clippy::module_inception)]
pub mod disruptor;

// Re-export core types for convenience
pub use sequence::Sequence;
// pub use core_interfaces::{Cursored, Sequenced, DataProvider, EventSink}; // Temporarily disabled
pub use disruptor::Disruptor;
pub use event_factory::{DefaultEventFactory, EventFactory};
pub use event_handler::{EventHandler, NoOpEventHandler};
pub use event_processor::{BatchEventProcessor, EventProcessor};
pub use event_translator::{
    EventTranslator, EventTranslatorOneArg, EventTranslatorThreeArg, EventTranslatorTwoArg,
};
pub use exception_handler::{DefaultExceptionHandler, ExceptionHandler};
pub use producer_type::ProducerType;
pub use ring_buffer::RingBuffer;
pub use sequence_barrier::{ProcessingSequenceBarrier, SequenceBarrier};
pub use sequencer::{MultiProducerSequencer, Sequencer, SingleProducerSequencer};
pub use wait_strategy::{
    BlockingWaitStrategy, BusySpinWaitStrategy, SleepingWaitStrategy, WaitStrategy,
    YieldingWaitStrategy,
};

// Re-export builder functions and producer types (inspired by disruptor-rs)
pub use builder::{build_multi_producer, build_single_producer, CloneableProducer};
pub use elegant_consumer::ElegantConsumer;
pub use producer::{MissingFreeSlots, Producer, RingBufferFull, SimpleProducer};

/// The initial cursor value for sequences (matches LMAX Disruptor)
pub const INITIAL_CURSOR_VALUE: i64 = -1;

/// Errors that can occur in the Disruptor
#[derive(Debug, thiserror::Error)]
pub enum DisruptorError {
    #[error("Ring buffer is full")]
    BufferFull,

    #[error("Invalid sequence: {0}")]
    InvalidSequence(i64),

    #[error("Buffer size must be a power of 2, got: {0}")]
    InvalidBufferSize(usize),

    #[error("Timeout waiting for sequence")]
    Timeout,

    #[error("Disruptor has been shut down")]
    Shutdown,

    #[error("Insufficient capacity")]
    InsufficientCapacity,

    #[error("Alert exception")]
    Alert,

    #[error("Shutdown error: {0}")]
    ShutdownError(String),
}

/// Result type for Disruptor operations
pub type Result<T> = std::result::Result<T, DisruptorError>;

/// Utility function to check if a number is a power of 2
pub fn is_power_of_two(n: usize) -> bool {
    n != 0 && (n & (n - 1)) == 0
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
