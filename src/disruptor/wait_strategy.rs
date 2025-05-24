//! Wait Strategy Implementation
//!
//! This module provides different wait strategies for the Disruptor pattern.
//! Wait strategies determine how consumers wait for new events to become available.
//! This follows the exact design from the original LMAX Disruptor WaitStrategy interface.

use crate::disruptor::{Result, DisruptorError, Sequence};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use std::sync::{Condvar, Mutex};

/// Strategy for waiting for events to become available
///
/// This trait defines how consumers wait for new events in the ring buffer.
/// Different strategies provide different trade-offs between CPU usage,
/// latency, and throughput.
pub trait WaitStrategy: Send + Sync + std::fmt::Debug {
    /// Wait for the given sequence to become available
    ///
    /// This method blocks until the specified sequence is available or
    /// until an alert/timeout occurs.
    ///
    /// # Arguments
    /// * `sequence` - The sequence to wait for
    /// * `cursor` - The current cursor position
    /// * `dependent_sequences` - Sequences that this consumer depends on
    ///
    /// # Returns
    /// The actual available sequence (may be higher than requested)
    ///
    /// # Errors
    /// Returns an error if waiting is interrupted or times out
    fn wait_for(
        &self,
        sequence: i64,
        cursor: Arc<Sequence>,
        dependent_sequences: &[Arc<Sequence>],
    ) -> Result<i64>;

    /// Wait for the given sequence with timeout
    ///
    /// This method blocks until the specified sequence is available or
    /// until the timeout expires.
    ///
    /// # Arguments
    /// * `sequence` - The sequence to wait for
    /// * `cursor` - The current cursor position
    /// * `dependent_sequences` - Sequences that this consumer depends on
    /// * `timeout` - Maximum time to wait
    ///
    /// # Returns
    /// The actual available sequence (may be higher than requested)
    ///
    /// # Errors
    /// Returns an error if waiting is interrupted or times out
    fn wait_for_with_timeout(
        &self,
        sequence: i64,
        cursor: Arc<Sequence>,
        dependent_sequences: &[Arc<Sequence>],
        _timeout: Duration,
    ) -> Result<i64> {
        // Default implementation delegates to wait_for
        // Specific strategies can override this for better timeout handling
        self.wait_for(sequence, cursor, dependent_sequences)
    }

    /// Signal all waiting threads to wake up
    ///
    /// This is used when shutting down the Disruptor to wake up
    /// any threads that are waiting for events.
    fn signal_all_when_blocking(&self);
}

/// Blocking wait strategy using parking/unparking
///
/// This strategy uses thread parking to wait for events. It provides
/// good CPU efficiency but may have higher latency than busy-wait strategies.
/// This is equivalent to the BlockingWaitStrategy in the original LMAX Disruptor.
#[derive(Debug, Default)]
pub struct BlockingWaitStrategy {
    // Use a condvar for proper blocking/signaling
    condvar: Condvar,
    mutex: Mutex<BlockingState>,
}

/// Internal state for blocking wait strategy
#[derive(Debug, Default)]
struct BlockingState {
    /// Whether the strategy has been alerted (for shutdown)
    alerted: bool,
    /// Number of waiting threads
    waiting_threads: usize,
}

impl BlockingWaitStrategy {
    /// Create a new blocking wait strategy
    pub fn new() -> Self {
        Self {
            condvar: Condvar::new(),
            mutex: Mutex::new(BlockingState::default()),
        }
    }

    /// Alert the wait strategy to wake up all waiting threads
    /// This is used during shutdown to interrupt waiting threads
    pub fn alert(&self) {
        if let Ok(mut state) = self.mutex.lock() {
            state.alerted = true;
            self.condvar.notify_all();
        }
    }

    /// Clear the alert state
    pub fn clear_alert(&self) {
        if let Ok(mut state) = self.mutex.lock() {
            state.alerted = false;
        }
    }

    /// Check if the strategy is currently alerted
    pub fn is_alerted(&self) -> bool {
        self.mutex.lock().map(|state| state.alerted).unwrap_or(false)
    }
}

impl WaitStrategy for BlockingWaitStrategy {
    fn wait_for(
        &self,
        sequence: i64,
        cursor: Arc<Sequence>,
        dependent_sequences: &[Arc<Sequence>],
    ) -> Result<i64> {
        let mut available_sequence = cursor.get();

        if available_sequence < sequence {
            // Check dependent sequences
            let minimum_sequence = Sequence::get_minimum_sequence(dependent_sequences);
            if minimum_sequence < sequence {
                return Err(DisruptorError::InsufficientCapacity);
            }

            // Real blocking implementation using condvar with proper exception handling
            let mut guard = self.mutex.lock().map_err(|_| DisruptorError::InvalidSequence(-1))?;

            // Increment waiting thread count
            guard.waiting_threads += 1;

            while {
                // Check for alert state first
                if guard.alerted {
                    guard.waiting_threads -= 1;
                    return Err(DisruptorError::InvalidSequence(-1)); // Interrupted
                }

                available_sequence = cursor.get();
                available_sequence < sequence
            } {
                // Wait for signal from producers with timeout for robustness
                let result = self.condvar.wait_timeout(guard, Duration::from_millis(10))
                    .map_err(|_| DisruptorError::InvalidSequence(-1))?;
                guard = result.0; // Get the guard back from wait_timeout

                // Check for timeout and alert again
                if guard.alerted {
                    guard.waiting_threads -= 1;
                    return Err(DisruptorError::InvalidSequence(-1)); // Interrupted
                }
            }

            // Decrement waiting thread count before returning
            guard.waiting_threads -= 1;
        }

        Ok(available_sequence)
    }

    fn wait_for_with_timeout(
        &self,
        sequence: i64,
        cursor: Arc<Sequence>,
        dependent_sequences: &[Arc<Sequence>],
        timeout: Duration,
    ) -> Result<i64> {
        let start_time = std::time::Instant::now();
        let mut available_sequence = cursor.get();

        if available_sequence < sequence {
            // Check dependent sequences
            let minimum_sequence = Sequence::get_minimum_sequence(dependent_sequences);
            if minimum_sequence < sequence {
                return Err(DisruptorError::InsufficientCapacity);
            }

            // Real blocking implementation with explicit timeout handling
            let mut guard = self.mutex.lock().map_err(|_| DisruptorError::InvalidSequence(-1))?;

            // Increment waiting thread count
            guard.waiting_threads += 1;

            while {
                // Check for timeout first
                if start_time.elapsed() >= timeout {
                    guard.waiting_threads -= 1;
                    return Err(DisruptorError::InsufficientCapacity); // Timeout
                }

                // Check for alert state
                if guard.alerted {
                    guard.waiting_threads -= 1;
                    return Err(DisruptorError::InvalidSequence(-1)); // Interrupted
                }

                available_sequence = cursor.get();
                available_sequence < sequence
            } {
                // Calculate remaining timeout
                let elapsed = start_time.elapsed();
                if elapsed >= timeout {
                    guard.waiting_threads -= 1;
                    return Err(DisruptorError::InsufficientCapacity); // Timeout
                }

                let remaining_timeout = timeout - elapsed;
                let wait_timeout = std::cmp::min(remaining_timeout, Duration::from_millis(10));

                // Wait for signal from producers with remaining timeout
                let result = self.condvar.wait_timeout(guard, wait_timeout)
                    .map_err(|_| DisruptorError::InvalidSequence(-1))?;
                guard = result.0; // Get the guard back from wait_timeout

                // Check for alert again after wait
                if guard.alerted {
                    guard.waiting_threads -= 1;
                    return Err(DisruptorError::InvalidSequence(-1)); // Interrupted
                }
            }

            // Decrement waiting thread count before returning
            guard.waiting_threads -= 1;
        }

        Ok(available_sequence)
    }

    fn signal_all_when_blocking(&self) {
        // Signal all waiting threads using condvar
        self.condvar.notify_all();
    }
}

/// Yielding wait strategy
///
/// This strategy yields the CPU to other threads while waiting.
/// It provides better CPU efficiency than busy-spin but may have
/// higher latency than blocking strategies.
#[derive(Debug, Default)]
pub struct YieldingWaitStrategy;

impl YieldingWaitStrategy {
    /// Create a new yielding wait strategy
    pub fn new() -> Self {
        Self
    }
}

impl WaitStrategy for YieldingWaitStrategy {
    fn wait_for(
        &self,
        sequence: i64,
        cursor: Arc<Sequence>,
        dependent_sequences: &[Arc<Sequence>],
    ) -> Result<i64> {
        let mut available_sequence = cursor.get();

        if available_sequence < sequence {
            // Check dependent sequences
            let minimum_sequence = Sequence::get_minimum_sequence(dependent_sequences);
            if minimum_sequence < sequence {
                return Err(DisruptorError::InsufficientCapacity);
            }

            while {
                available_sequence = cursor.get();
                available_sequence < sequence
            } {
                thread::yield_now();
            }
        }

        Ok(available_sequence)
    }

    fn signal_all_when_blocking(&self) {
        // Yielding strategy doesn't block, so no signaling needed
    }
}

/// Busy-spin wait strategy
///
/// This strategy continuously polls for events without yielding the CPU.
/// It provides the lowest latency but uses 100% CPU while waiting.
/// Use this only when you can dedicate CPU cores to the Disruptor.
#[derive(Debug, Default)]
pub struct BusySpinWaitStrategy;

impl BusySpinWaitStrategy {
    /// Create a new busy-spin wait strategy
    pub fn new() -> Self {
        Self
    }
}

impl WaitStrategy for BusySpinWaitStrategy {
    fn wait_for(
        &self,
        sequence: i64,
        cursor: Arc<Sequence>,
        dependent_sequences: &[Arc<Sequence>],
    ) -> Result<i64> {
        let mut available_sequence = cursor.get();

        if available_sequence < sequence {
            // Check dependent sequences
            let minimum_sequence = Sequence::get_minimum_sequence(dependent_sequences);
            if minimum_sequence < sequence {
                return Err(DisruptorError::InsufficientCapacity);
            }

            // Busy spin until sequence is available
            while {
                available_sequence = cursor.get();
                available_sequence < sequence
            } {
                // Busy spin - no yielding or parking
                std::hint::spin_loop();
            }
        }

        Ok(available_sequence)
    }

    fn signal_all_when_blocking(&self) {
        // Busy spin strategy doesn't block, so no signaling needed
    }
}

/// Sleeping wait strategy
///
/// This strategy sleeps for a short duration while waiting.
/// It provides good CPU efficiency but may have variable latency
/// depending on the sleep duration.
#[derive(Debug)]
pub struct SleepingWaitStrategy {
    sleep_duration: Duration,
}

impl SleepingWaitStrategy {
    /// Create a new sleeping wait strategy with default sleep duration
    pub fn new() -> Self {
        Self {
            sleep_duration: Duration::from_millis(1),
        }
    }

    /// Create a new sleeping wait strategy with custom sleep duration
    ///
    /// # Arguments
    /// * `sleep_duration` - How long to sleep between checks
    pub fn new_with_duration(sleep_duration: Duration) -> Self {
        Self { sleep_duration }
    }
}

impl Default for SleepingWaitStrategy {
    fn default() -> Self {
        Self::new()
    }
}

impl WaitStrategy for SleepingWaitStrategy {
    fn wait_for(
        &self,
        sequence: i64,
        cursor: Arc<Sequence>,
        dependent_sequences: &[Arc<Sequence>],
    ) -> Result<i64> {
        let mut available_sequence = cursor.get();

        if available_sequence < sequence {
            // Check dependent sequences
            let minimum_sequence = Sequence::get_minimum_sequence(dependent_sequences);
            if minimum_sequence < sequence {
                return Err(DisruptorError::InsufficientCapacity);
            }

            while {
                available_sequence = cursor.get();
                available_sequence < sequence
            } {
                thread::sleep(self.sleep_duration);
            }
        }

        Ok(available_sequence)
    }

    fn signal_all_when_blocking(&self) {
        // Sleeping strategy doesn't use complex blocking, so no signaling needed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blocking_wait_strategy() {
        let strategy = BlockingWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(10));
        let dependent_sequences = vec![];

        // Should return immediately if sequence is already available
        let result = strategy.wait_for(5, cursor, &dependent_sequences);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 10);
    }

    #[test]
    fn test_yielding_wait_strategy() {
        let strategy = YieldingWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(10));
        let dependent_sequences = vec![];

        let result = strategy.wait_for(5, cursor, &dependent_sequences);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 10);
    }

    #[test]
    fn test_busy_spin_wait_strategy() {
        let strategy = BusySpinWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(10));
        let dependent_sequences = vec![];

        let result = strategy.wait_for(5, cursor, &dependent_sequences);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 10);
    }

    #[test]
    fn test_sleeping_wait_strategy() {
        let strategy = SleepingWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(10));
        let dependent_sequences = vec![];

        let result = strategy.wait_for(5, cursor.clone(), &dependent_sequences);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 10);

        // Test custom duration
        let custom_strategy = SleepingWaitStrategy::new_with_duration(Duration::from_micros(100));
        let result = custom_strategy.wait_for(5, cursor, &dependent_sequences);
        assert!(result.is_ok());
    }
}
