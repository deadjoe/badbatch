//! BadBatch Disruptor Implementation
//!
//! This module provides a complete Rust implementation of the LMAX Disruptor pattern,
//! designed for ultra-low latency inter-thread messaging with mechanical sympathy.

pub mod sequence;
pub mod ring_buffer;
pub mod sequencer;
pub mod wait_strategy;
pub mod event_processor;
pub mod sequence_barrier;

pub use sequence::Sequence;
pub use ring_buffer::{RingBuffer, SharedRingBuffer};
pub use sequencer::{Sequencer, SingleProducerSequencer, MultiProducerSequencer};
pub use wait_strategy::{WaitStrategy, BlockingWaitStrategy, YieldingWaitStrategy, BusySpinWaitStrategy, SleepingWaitStrategy};
pub use event_processor::{EventProcessor, BatchEventProcessor};
pub use sequence_barrier::SequenceBarrier;

/// The initial cursor value for sequences
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
}

pub type Result<T> = std::result::Result<T, DisruptorError>;

/// Trait for events that can be processed by the Disruptor
pub trait Event: Send + Sync + 'static {
    /// Clear the event data (for memory management)
    fn clear(&mut self);
}

/// Factory for creating events
pub trait EventFactory<T: Event>: Send + Sync {
    /// Create a new event instance
    fn new_instance(&self) -> T;
}

/// Handler for processing events
pub trait EventHandler<T: Event>: Send + Sync {
    /// Process an event
    fn on_event(&mut self, event: &mut T, sequence: i64, end_of_batch: bool) -> Result<()>;
}

/// Utility function to check if a number is a power of 2
pub fn is_power_of_two(n: usize) -> bool {
    n != 0 && (n & (n - 1)) == 0
}

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
