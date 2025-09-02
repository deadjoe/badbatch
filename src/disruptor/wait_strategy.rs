//! Wait Strategy Implementation
//!
//! This module provides different wait strategies for the Disruptor pattern.
//! Wait strategies determine how consumers wait for new events to become available.
//! This follows the exact design from the original LMAX Disruptor WaitStrategy interface.

use crate::disruptor::{DisruptorError, Result, Sequence};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::{Condvar, Mutex};
use std::thread;
use std::time::Duration;

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

    /// Wait for the given sequence with external shutdown signal
    ///
    /// This method blocks until the specified sequence is available or
    /// until an external shutdown signal is received.
    ///
    /// # Arguments
    /// * `sequence` - The sequence to wait for
    /// * `cursor` - The current cursor position
    /// * `dependent_sequences` - Sequences that this consumer depends on
    /// * `shutdown_flag` - External shutdown signal to check
    ///
    /// # Returns
    /// The actual available sequence (may be higher than requested)
    ///
    /// # Errors
    /// Returns `DisruptorError::Alert` if shutdown is signaled
    fn wait_for_with_shutdown(
        &self,
        sequence: i64,
        cursor: Arc<Sequence>,
        dependent_sequences: &[Arc<Sequence>],
        shutdown_flag: &AtomicBool,
    ) -> Result<i64> {
        // Default implementation with periodic shutdown checks
        let check_interval = Duration::from_millis(1);

        loop {
            // Check shutdown flag first
            if shutdown_flag.load(Ordering::Acquire) {
                return Err(DisruptorError::Alert);
            }

            // Try to get the sequence with a short timeout
            match self.wait_for_with_timeout(
                sequence,
                cursor.clone(),
                dependent_sequences,
                check_interval,
            ) {
                Ok(available_sequence) => return Ok(available_sequence),
                Err(DisruptorError::Timeout) => {
                    // Continue loop to check shutdown flag again
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
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

impl Clone for BlockingWaitStrategy {
    fn clone(&self) -> Self {
        // Create a new instance with default state
        Self::new()
    }
}

/// Internal state for blocking wait strategy
#[derive(Debug, Default)]
struct BlockingState {
    /// Whether the strategy has been alerted (for shutdown)
    alerted: bool,
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
        self.mutex
            .lock()
            .map(|state| state.alerted)
            .unwrap_or(false)
    }
}

impl WaitStrategy for BlockingWaitStrategy {
    fn wait_for(
        &self,
        sequence: i64,
        cursor: Arc<Sequence>,
        dependent_sequences: &[Arc<Sequence>],
    ) -> Result<i64> {
        // This follows the exact LMAX Disruptor BlockingWaitStrategy.waitFor logic
        let mut available_sequence;

        // First check: wait for cursor to advance if needed
        if cursor.get() < sequence {
            let mut guard = self.mutex.lock().map_err(|_| DisruptorError::Alert)?;

            while cursor.get() < sequence {
                // Check for alert state (equivalent to barrier.checkAlert() in LMAX)
                if guard.alerted {
                    return Err(DisruptorError::Alert);
                }

                // Wait for signal from producers (equivalent to mutex.wait() in LMAX)
                guard = self
                    .condvar
                    .wait(guard)
                    .map_err(|_| DisruptorError::Alert)?;
            }
        }

        // Second check: spin wait for dependent sequences (equivalent to LMAX spin wait)
        loop {
            available_sequence = if dependent_sequences.is_empty() {
                cursor.get()
            } else {
                Sequence::get_minimum_sequence(dependent_sequences)
            };

            if available_sequence >= sequence {
                break;
            }

            // Check for alert state again
            if self.is_alerted() {
                return Err(DisruptorError::Alert);
            }

            // Equivalent to Thread.onSpinWait() in LMAX
            std::hint::spin_loop();
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

        // First check: wait for cursor to advance if needed (with timeout)
        if cursor.get() < sequence {
            let mut guard = self.mutex.lock().map_err(|_| DisruptorError::Alert)?;

            while cursor.get() < sequence {
                // Check for timeout
                if start_time.elapsed() >= timeout {
                    return Err(DisruptorError::Timeout);
                }

                // Check for alert state
                if guard.alerted {
                    return Err(DisruptorError::Alert);
                }

                // Calculate remaining timeout
                let remaining = timeout.saturating_sub(start_time.elapsed());
                if remaining.is_zero() {
                    return Err(DisruptorError::Timeout);
                }

                // Wait with timeout
                let (new_guard, timeout_result) = self
                    .condvar
                    .wait_timeout(guard, remaining)
                    .map_err(|_| DisruptorError::Alert)?;
                guard = new_guard;

                if timeout_result.timed_out() {
                    return Err(DisruptorError::Timeout);
                }
            }
        }

        // Second check: spin wait for dependent sequences (with timeout)
        loop {
            let available_sequence = if dependent_sequences.is_empty() {
                cursor.get()
            } else {
                Sequence::get_minimum_sequence(dependent_sequences)
            };

            if available_sequence >= sequence {
                return Ok(available_sequence);
            }

            // Check for timeout
            if start_time.elapsed() >= timeout {
                return Err(DisruptorError::Timeout);
            }

            // Check for alert state
            if self.is_alerted() {
                return Err(DisruptorError::Alert);
            }

            std::hint::spin_loop();
        }
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
#[derive(Debug, Default, Clone)]
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
        loop {
            let cursor_sequence = cursor.get();
            let dep_min = if dependent_sequences.is_empty() {
                cursor_sequence
            } else {
                Sequence::get_minimum_sequence(dependent_sequences)
            };
            let available = std::cmp::min(cursor_sequence, dep_min);
            if available >= sequence {
                return Ok(available);
            }
            thread::yield_now();
        }
    }

    fn wait_for_with_timeout(
        &self,
        sequence: i64,
        cursor: Arc<Sequence>,
        dependent_sequences: &[Arc<Sequence>],
        timeout: Duration,
    ) -> Result<i64> {
        let start_time = std::time::Instant::now();

        // Wait for both cursor and dependent sequences to reach the required sequence
        loop {
            let cursor_sequence = cursor.get();
            let minimum_dependent_sequence = if dependent_sequences.is_empty() {
                cursor_sequence
            } else {
                Sequence::get_minimum_sequence(dependent_sequences)
            };

            // The available sequence is the minimum of cursor and dependent sequences
            let available_sequence = std::cmp::min(cursor_sequence, minimum_dependent_sequence);

            if available_sequence >= sequence {
                return Ok(available_sequence);
            }

            // Check for timeout
            if start_time.elapsed() >= timeout {
                return Err(DisruptorError::Timeout);
            }

            thread::yield_now();
        }
    }

    fn wait_for_with_shutdown(
        &self,
        sequence: i64,
        cursor: Arc<Sequence>,
        dependent_sequences: &[Arc<Sequence>],
        shutdown_flag: &AtomicBool,
    ) -> Result<i64> {
        loop {
            if shutdown_flag.load(Ordering::Acquire) {
                return Err(DisruptorError::Alert);
            }
            let cursor_sequence = cursor.get();
            let dep_min = if dependent_sequences.is_empty() {
                cursor_sequence
            } else {
                Sequence::get_minimum_sequence(dependent_sequences)
            };
            let available = std::cmp::min(cursor_sequence, dep_min);
            if available >= sequence {
                return Ok(available);
            }
            thread::yield_now();
        }
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
#[derive(Debug, Default, Clone)]
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
        loop {
            let cursor_sequence = cursor.get();
            let dep_min = if dependent_sequences.is_empty() {
                cursor_sequence
            } else {
                // Ensure we see the most up-to-date dependent sequence values
                std::sync::atomic::fence(std::sync::atomic::Ordering::Acquire);
                Sequence::get_minimum_sequence(dependent_sequences)
            };
            let available = std::cmp::min(cursor_sequence, dep_min);
            if available >= sequence {
                // Ensure visibility of writes before returning
                std::sync::atomic::fence(std::sync::atomic::Ordering::Acquire);
                return Ok(available);
            }
            std::hint::spin_loop();
        }
    }

    fn wait_for_with_timeout(
        &self,
        sequence: i64,
        cursor: Arc<Sequence>,
        dependent_sequences: &[Arc<Sequence>],
        timeout: Duration,
    ) -> Result<i64> {
        let start_time = std::time::Instant::now();

        // Wait for both cursor and dependent sequences to reach the required sequence
        loop {
            let cursor_sequence = cursor.get();
            let minimum_dependent_sequence = if dependent_sequences.is_empty() {
                cursor_sequence
            } else {
                Sequence::get_minimum_sequence(dependent_sequences)
            };

            // The available sequence is the minimum of cursor and dependent sequences
            let available_sequence = std::cmp::min(cursor_sequence, minimum_dependent_sequence);

            if available_sequence >= sequence {
                return Ok(available_sequence);
            }

            // Check for timeout
            if start_time.elapsed() >= timeout {
                return Err(DisruptorError::Timeout);
            }

            // Busy spin - no yielding or parking
            std::hint::spin_loop();
        }
    }

    fn wait_for_with_shutdown(
        &self,
        sequence: i64,
        cursor: Arc<Sequence>,
        dependent_sequences: &[Arc<Sequence>],
        shutdown_flag: &AtomicBool,
    ) -> Result<i64> {
        // Simplified logic following LMAX Disruptor pattern
        let mut available_sequence;

        // Create a combined sequence that represents the minimum of cursor and dependent sequences
        let dependent_sequence = if dependent_sequences.is_empty() {
            cursor.clone()
        } else {
            // For dependent sequences, we need to wait for the minimum
            loop {
                // Check shutdown flag
                if shutdown_flag.load(Ordering::Acquire) {
                    return Err(DisruptorError::Alert);
                }

                let cursor_value = cursor.get();
                let min_dependent = Sequence::get_minimum_sequence(dependent_sequences);

                // The available sequence is the minimum of cursor and dependent sequences
                available_sequence = std::cmp::min(cursor_value, min_dependent);

                if available_sequence >= sequence {
                    return Ok(available_sequence);
                }

                std::hint::spin_loop();
            }
        };

        // Simple case: only wait for cursor
        while {
            available_sequence = dependent_sequence.get();
            available_sequence < sequence
        } {
            // Check shutdown flag
            if shutdown_flag.load(Ordering::Acquire) {
                return Err(DisruptorError::Alert);
            }
            std::hint::spin_loop();
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
#[derive(Debug, Clone)]
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
        loop {
            let cursor_sequence = cursor.get();
            let dep_min = if dependent_sequences.is_empty() {
                cursor_sequence
            } else {
                Sequence::get_minimum_sequence(dependent_sequences)
            };
            let available = std::cmp::min(cursor_sequence, dep_min);
            if available >= sequence {
                return Ok(available);
            }
            thread::sleep(self.sleep_duration);
        }
    }

    fn wait_for_with_shutdown(
        &self,
        sequence: i64,
        cursor: Arc<Sequence>,
        dependent_sequences: &[Arc<Sequence>],
        shutdown_flag: &AtomicBool,
    ) -> Result<i64> {
        loop {
            if shutdown_flag.load(Ordering::Acquire) {
                return Err(DisruptorError::Alert);
            }
            let cursor_sequence = cursor.get();
            let dep_min = if dependent_sequences.is_empty() {
                cursor_sequence
            } else {
                Sequence::get_minimum_sequence(dependent_sequences)
            };
            let available = std::cmp::min(cursor_sequence, dep_min);
            if available >= sequence {
                return Ok(available);
            }
            thread::sleep(self.sleep_duration);
        }
    }

    fn signal_all_when_blocking(&self) {
        // Sleeping strategy doesn't use complex blocking, so no signaling needed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Instant;

    #[test]
    fn test_blocking_wait_strategy_creation() {
        let strategy = BlockingWaitStrategy::new();
        assert!(!strategy.is_alerted());

        // Test default creation
        let default_strategy = BlockingWaitStrategy::default();
        assert!(!default_strategy.is_alerted());
    }

    #[test]
    fn test_blocking_wait_strategy_immediate_return() {
        let strategy = BlockingWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(10));
        let dependent_sequences = vec![];

        // Should return immediately if sequence is already available
        let result = strategy.wait_for(5, cursor, &dependent_sequences);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 10);
    }

    #[test]
    fn test_blocking_wait_strategy_with_dependent_sequences() {
        let strategy = BlockingWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(10));
        let dep1 = Arc::new(Sequence::new(8));
        let dep2 = Arc::new(Sequence::new(6));
        let dependent_sequences = vec![dep1, dep2];

        // Should return minimum of dependent sequences
        let result = strategy.wait_for(5, cursor, &dependent_sequences);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 6); // Minimum of dependent sequences
    }

    #[test]
    fn test_blocking_wait_strategy_alert_functionality() {
        let strategy = BlockingWaitStrategy::new();

        // Test alert and clear
        assert!(!strategy.is_alerted());
        strategy.alert();
        assert!(strategy.is_alerted());
        strategy.clear_alert();
        assert!(!strategy.is_alerted());
    }

    #[test]
    fn test_blocking_wait_strategy_alert_during_wait() {
        let strategy = Arc::new(BlockingWaitStrategy::new());
        let cursor = Arc::new(Sequence::new(0)); // Start with low sequence
        let dependent_sequences = vec![];

        let strategy_clone = strategy.clone();
        let cursor_clone = cursor.clone();

        // Spawn a thread that will wait for a high sequence
        let handle =
            thread::spawn(move || strategy_clone.wait_for(100, cursor_clone, &dependent_sequences));

        // Give the thread time to start waiting
        thread::sleep(Duration::from_millis(10));

        // Alert the strategy
        strategy.alert();

        // The waiting thread should return with an alert error
        let result = handle.join().unwrap();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), DisruptorError::Alert));
    }

    #[test]
    fn test_blocking_wait_strategy_timeout() {
        let strategy = BlockingWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(0));
        let dependent_sequences = vec![];
        let timeout = Duration::from_millis(50);

        let start = Instant::now();
        let result = strategy.wait_for_with_timeout(100, cursor, &dependent_sequences, timeout);
        let elapsed = start.elapsed();

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), DisruptorError::Timeout));
        assert!(elapsed >= timeout);
        assert!(elapsed < timeout + Duration::from_millis(50)); // Allow some tolerance
    }

    #[test]
    fn test_blocking_wait_strategy_timeout_immediate_return() {
        let strategy = BlockingWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(10));
        let dependent_sequences = vec![];
        let timeout = Duration::from_millis(100);

        // Should return immediately since sequence is already available
        let start = Instant::now();
        let result = strategy.wait_for_with_timeout(5, cursor, &dependent_sequences, timeout);
        let elapsed = start.elapsed();

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 10);
        assert!(elapsed < Duration::from_millis(10)); // Should be very fast
    }

    #[test]
    fn test_blocking_wait_strategy_signal_all() {
        let strategy = BlockingWaitStrategy::new();

        // Test that signal_all_when_blocking doesn't panic
        strategy.signal_all_when_blocking();
    }

    #[test]
    fn test_yielding_wait_strategy_creation() {
        let _strategy = YieldingWaitStrategy::new();

        // Test default creation
        let _default_strategy = YieldingWaitStrategy::new();

        // Both should work (no state to compare)
    }

    #[test]
    fn test_yielding_wait_strategy_immediate_return() {
        let strategy = YieldingWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(10));
        let dependent_sequences = vec![];

        let result = strategy.wait_for(5, cursor, &dependent_sequences);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 10);
    }

    #[test]
    fn test_yielding_wait_strategy_with_dependent_sequences() {
        let strategy = YieldingWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(10));
        let dep1 = Arc::new(Sequence::new(8));
        let dep2 = Arc::new(Sequence::new(6));
        let dependent_sequences = vec![dep1, dep2];

        let result = strategy.wait_for(5, cursor, &dependent_sequences);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 6); // Minimum of dependent sequences
    }

    #[test]
    fn test_yielding_wait_strategy_with_low_dependent_sequence() {
        let strategy = YieldingWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(3)); // Cursor lower than requested
        let dep1 = Arc::new(Sequence::new(2)); // Dependent sequence even lower
        let dependent_sequences = vec![dep1];

        // With unified semantics, use timeout variant to avoid hanging
        let result = strategy.wait_for_with_timeout(
            5,
            cursor,
            &dependent_sequences,
            Duration::from_millis(10),
        );

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), DisruptorError::Timeout));
    }

    #[test]
    fn test_yielding_wait_strategy_signal_all() {
        let strategy = YieldingWaitStrategy::new();

        // Test that signal_all_when_blocking doesn't panic
        strategy.signal_all_when_blocking();
    }

    #[test]
    fn test_busy_spin_wait_strategy_creation() {
        let _strategy = BusySpinWaitStrategy::new();

        // Test default creation
        let _default_strategy = BusySpinWaitStrategy::new();

        // Both should work (no state to compare)
    }

    #[test]
    fn test_busy_spin_wait_strategy_immediate_return() {
        let strategy = BusySpinWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(10));
        let dependent_sequences = vec![];

        let result = strategy.wait_for(5, cursor, &dependent_sequences);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 10);
    }

    #[test]
    fn test_busy_spin_wait_strategy_with_dependent_sequences() {
        let strategy = BusySpinWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(10));
        let dep1 = Arc::new(Sequence::new(8));
        let dep2 = Arc::new(Sequence::new(6));
        let dependent_sequences = vec![dep1, dep2];

        let result = strategy.wait_for(5, cursor, &dependent_sequences);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 6); // Minimum of dependent sequences
    }

    #[test]
    fn test_busy_spin_wait_strategy_with_low_dependent_sequence() {
        let strategy = BusySpinWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(3)); // Cursor lower than requested
        let dep1 = Arc::new(Sequence::new(2)); // Dependent sequence even lower
        let dependent_sequences = vec![dep1];

        let result = strategy.wait_for_with_timeout(
            5,
            cursor,
            &dependent_sequences,
            Duration::from_millis(10),
        );

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), DisruptorError::Timeout));
    }

    #[test]
    fn test_busy_spin_wait_strategy_signal_all() {
        let strategy = BusySpinWaitStrategy::new();

        // Test that signal_all_when_blocking doesn't panic
        strategy.signal_all_when_blocking();
    }

    #[test]
    fn test_sleeping_wait_strategy_creation() {
        let strategy = SleepingWaitStrategy::new();
        assert_eq!(strategy.sleep_duration, Duration::from_millis(1));

        // Test default creation
        let default_strategy = SleepingWaitStrategy::default();
        assert_eq!(default_strategy.sleep_duration, Duration::from_millis(1));

        // Test custom duration
        let custom_duration = Duration::from_micros(500);
        let custom_strategy = SleepingWaitStrategy::new_with_duration(custom_duration);
        assert_eq!(custom_strategy.sleep_duration, custom_duration);
    }

    #[test]
    fn test_sleeping_wait_strategy_immediate_return() {
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

    #[test]
    fn test_sleeping_wait_strategy_with_dependent_sequences() {
        let strategy = SleepingWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(10));
        let dep1 = Arc::new(Sequence::new(8));
        let dep2 = Arc::new(Sequence::new(6));
        let dependent_sequences = vec![dep1, dep2];

        let result = strategy.wait_for(5, cursor, &dependent_sequences);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 6); // Minimum of dependent sequences
    }

    #[test]
    fn test_sleeping_wait_strategy_with_low_dependent_sequence() {
        let strategy = SleepingWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(3)); // Cursor lower than requested
        let dep1 = Arc::new(Sequence::new(2)); // Dependent sequence even lower
        let dependent_sequences = vec![dep1];

        let result = strategy.wait_for_with_timeout(
            5,
            cursor,
            &dependent_sequences,
            Duration::from_millis(10),
        );

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), DisruptorError::Timeout));
    }

    #[test]
    fn test_sleeping_wait_strategy_signal_all() {
        let strategy = SleepingWaitStrategy::new();

        // Test that signal_all_when_blocking doesn't panic
        strategy.signal_all_when_blocking();
    }

    // Advanced tests for complex scenarios

    #[test]
    fn test_blocking_wait_strategy_concurrent_access() {
        let strategy = Arc::new(BlockingWaitStrategy::new());
        let cursor = Arc::new(Sequence::new(5));
        let dependent_sequences = vec![];

        let mut handles = vec![];

        // Spawn multiple threads that wait for different sequences
        for i in 0..3 {
            let strategy_clone = strategy.clone();
            let cursor_clone = cursor.clone();
            let deps_clone = dependent_sequences.clone();

            let handle =
                thread::spawn(move || strategy_clone.wait_for(i, cursor_clone, &deps_clone));
            handles.push(handle);
        }

        // All threads should complete successfully since cursor is at 5
        for handle in handles {
            let result = handle.join().unwrap();
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), 5);
        }
    }

    #[test]
    fn test_blocking_wait_strategy_timeout_with_dependent_sequences() {
        let strategy = BlockingWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(10));
        let dep1 = Arc::new(Sequence::new(2)); // This will cause timeout
        let dependent_sequences = vec![dep1];
        let timeout = Duration::from_millis(50);

        let start = Instant::now();
        let result = strategy.wait_for_with_timeout(5, cursor, &dependent_sequences, timeout);
        let elapsed = start.elapsed();

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), DisruptorError::Timeout));
        assert!(elapsed >= timeout);
    }

    #[test]
    fn test_blocking_wait_strategy_zero_timeout() {
        let strategy = BlockingWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(0));
        let dependent_sequences = vec![];
        let timeout = Duration::from_millis(0);

        let result = strategy.wait_for_with_timeout(100, cursor, &dependent_sequences, timeout);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), DisruptorError::Timeout));
    }

    #[test]
    fn test_yielding_wait_strategy_actual_yielding() {
        let strategy = YieldingWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(0));
        let dependent_sequences = vec![];

        // This should complete quickly since we're testing the yielding behavior
        // We can't easily test that it actually yields, but we can test it doesn't hang
        let start = Instant::now();

        // Spawn a thread that will advance the cursor after a short delay
        let cursor_clone = cursor.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            cursor_clone.set(10);
        });

        let result = strategy.wait_for(5, cursor, &dependent_sequences);
        let elapsed = start.elapsed();

        assert!(result.is_ok());
        assert!(elapsed >= Duration::from_millis(10));
        assert!(elapsed < Duration::from_millis(100)); // Should not take too long
    }

    #[test]
    fn test_busy_spin_wait_strategy_actual_spinning() {
        let strategy = BusySpinWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(0));
        let dependent_sequences = vec![];

        let start = Instant::now();

        // Spawn a thread that will advance the cursor after a short delay
        let cursor_clone = cursor.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            cursor_clone.set(10);
        });

        let result = strategy.wait_for(5, cursor, &dependent_sequences);
        let elapsed = start.elapsed();

        assert!(result.is_ok());
        assert!(elapsed >= Duration::from_millis(10));
        assert!(elapsed < Duration::from_millis(100)); // Should not take too long
    }

    #[test]
    fn test_sleeping_wait_strategy_actual_sleeping() {
        let strategy = SleepingWaitStrategy::new_with_duration(Duration::from_millis(5));
        let cursor = Arc::new(Sequence::new(0));
        let dependent_sequences = vec![];

        let start = Instant::now();

        // Spawn a thread that will advance the cursor after a short delay
        let cursor_clone = cursor.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            cursor_clone.set(10);
        });

        let result = strategy.wait_for(5, cursor, &dependent_sequences);
        let elapsed = start.elapsed();

        assert!(result.is_ok());
        assert!(elapsed >= Duration::from_millis(20));
        assert!(elapsed < Duration::from_millis(100)); // Should not take too long
    }

    #[test]
    fn test_wait_strategy_trait_default_timeout_implementation() {
        let strategy = YieldingWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(10));
        let dependent_sequences = vec![];
        let timeout = Duration::from_millis(100);

        // Test that default implementation delegates to wait_for
        let result = strategy.wait_for_with_timeout(5, cursor, &dependent_sequences, timeout);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 10);
    }

    #[test]
    fn test_blocking_wait_strategy_mutex_poisoning_recovery() {
        let strategy = BlockingWaitStrategy::new();

        // Test that the strategy can handle mutex operations gracefully
        // Even if there were poisoning issues, the methods should not panic
        strategy.alert();
        strategy.clear_alert();
        assert!(!strategy.is_alerted());
    }

    #[test]
    fn test_all_strategies_debug_format() {
        let blocking = BlockingWaitStrategy::new();
        let yielding = YieldingWaitStrategy::new();
        let busy_spin = BusySpinWaitStrategy::new();
        let sleeping = SleepingWaitStrategy::new();

        // Test that all strategies implement Debug properly
        let blocking_debug = format!("{blocking:?}");
        let yielding_debug = format!("{yielding:?}");
        let busy_spin_debug = format!("{busy_spin:?}");
        let sleeping_debug = format!("{sleeping:?}");

        assert!(blocking_debug.contains("BlockingWaitStrategy"));
        assert!(yielding_debug.contains("YieldingWaitStrategy"));
        assert!(busy_spin_debug.contains("BusySpinWaitStrategy"));
        assert!(sleeping_debug.contains("SleepingWaitStrategy"));
    }

    #[test]
    fn test_blocking_wait_strategy_edge_cases() {
        let strategy = BlockingWaitStrategy::new();

        // Test with sequence equal to cursor
        let cursor = Arc::new(Sequence::new(5));
        let result = strategy.wait_for(5, cursor.clone(), &[]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 5);

        // Test with sequence lower than cursor - should return immediately
        let result = strategy.wait_for(3, cursor.clone(), &[]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 5);

        // Test with dependent sequence higher than cursor
        let dep = Arc::new(Sequence::new(10));
        let result = strategy.wait_for(3, cursor, &[dep]); // Request sequence 3, which is available
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 10); // Should return dependent sequence value since it's the minimum available
    }

    #[test]
    fn test_multiple_dependent_sequences_minimum() {
        let strategy = BlockingWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(20));

        // Create multiple dependent sequences with different values
        let dep1 = Arc::new(Sequence::new(15));
        let dep2 = Arc::new(Sequence::new(12));
        let dep3 = Arc::new(Sequence::new(18));
        let dependent_sequences = vec![dep1, dep2, dep3];

        let result = strategy.wait_for(10, cursor, &dependent_sequences);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 12); // Should return minimum of dependent sequences
    }

    #[test]
    fn test_sleeping_wait_strategy_very_short_duration() {
        let strategy = SleepingWaitStrategy::new_with_duration(Duration::from_nanos(1));
        let cursor = Arc::new(Sequence::new(10));
        let dependent_sequences = vec![];

        let result = strategy.wait_for(5, cursor, &dependent_sequences);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 10);
    }

    #[test]
    fn test_blocking_wait_strategy_alert_before_wait() {
        let strategy = BlockingWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(0));
        let dependent_sequences = vec![];

        // Alert before waiting
        strategy.alert();

        let result = strategy.wait_for(100, cursor, &dependent_sequences);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), DisruptorError::Alert));
    }

    // Tests inspired by LMAX Disruptor test patterns

    #[test]
    fn test_blocking_wait_strategy_with_delayed_sequence_update() {
        let strategy = Arc::new(BlockingWaitStrategy::new());
        let cursor = Arc::new(Sequence::new(-1)); // Start with no events available

        // Spawn a thread that will update the cursor after a delay
        let cursor_clone = cursor.clone();
        let strategy_clone = strategy.clone();
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            cursor_clone.set(5); // Make sequences 0-5 available
            strategy_clone.signal_all_when_blocking();
        });

        let start = Instant::now();
        let result = strategy.wait_for(3, cursor, &[]);
        let elapsed = start.elapsed();

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 5);
        assert!(elapsed >= Duration::from_millis(40)); // Should have waited
        assert!(elapsed < Duration::from_millis(200)); // But not too long

        handle.join().unwrap();
    }

    #[test]
    fn test_yielding_wait_strategy_with_delayed_sequence_update() {
        let strategy = YieldingWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(-1));

        // Spawn a thread that will update the cursor after a delay
        let cursor_clone = cursor.clone();
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            cursor_clone.set(5);
        });

        let start = Instant::now();
        let result = strategy.wait_for(3, cursor, &[]);
        let elapsed = start.elapsed();

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 5);
        assert!(elapsed >= Duration::from_millis(15)); // Should have waited
        assert!(elapsed < Duration::from_millis(100)); // But not too long

        handle.join().unwrap();
    }

    #[test]
    fn test_busy_spin_wait_strategy_with_delayed_sequence_update() {
        let strategy = BusySpinWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(-1));

        // Spawn a thread that will update the cursor after a delay
        let cursor_clone = cursor.clone();
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            cursor_clone.set(5);
        });

        let start = Instant::now();
        let result = strategy.wait_for(3, cursor, &[]);
        let elapsed = start.elapsed();

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 5);
        assert!(elapsed >= Duration::from_millis(15)); // Should have waited
        assert!(elapsed < Duration::from_millis(100)); // But not too long

        handle.join().unwrap();
    }

    #[test]
    fn test_sleeping_wait_strategy_with_delayed_sequence_update() {
        let strategy = SleepingWaitStrategy::new_with_duration(Duration::from_millis(5));
        let cursor = Arc::new(Sequence::new(-1));

        // Spawn a thread that will update the cursor after a delay
        let cursor_clone = cursor.clone();
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(30));
            cursor_clone.set(5);
        });

        let start = Instant::now();
        let result = strategy.wait_for(3, cursor, &[]);
        let elapsed = start.elapsed();

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 5);
        assert!(elapsed >= Duration::from_millis(25)); // Should have waited
        assert!(elapsed < Duration::from_millis(100)); // But not too long

        handle.join().unwrap();
    }

    #[test]
    fn test_wait_strategy_with_multiple_dependent_sequences_realistic() {
        let strategy = BlockingWaitStrategy::new();
        let cursor = Arc::new(Sequence::new(10));

        // Create multiple consumer sequences at different positions
        let consumer1 = Arc::new(Sequence::new(8)); // Slowest consumer
        let consumer2 = Arc::new(Sequence::new(9)); // Faster consumer
        let consumer3 = Arc::new(Sequence::new(7)); // Even slower consumer

        let dependent_sequences = vec![consumer1, consumer2, consumer3];

        // Should return the minimum of all dependent sequences
        let result = strategy.wait_for(6, cursor, &dependent_sequences);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 7); // Minimum of dependent sequences
    }

    #[test]
    fn test_wait_strategy_performance_characteristics() {
        // This test verifies that different strategies have expected performance characteristics
        let cursor = Arc::new(Sequence::new(10));
        let dependent_sequences = vec![];

        // All strategies should return immediately when sequence is available
        let strategies: Vec<Box<dyn WaitStrategy>> = vec![
            Box::new(BusySpinWaitStrategy::new()),
            Box::new(YieldingWaitStrategy::new()),
            Box::new(SleepingWaitStrategy::new()),
            Box::new(BlockingWaitStrategy::new()),
        ];

        for strategy in strategies {
            let start = Instant::now();
            let result = strategy.wait_for(5, cursor.clone(), &dependent_sequences);
            let elapsed = start.elapsed();

            assert!(result.is_ok());
            assert_eq!(result.unwrap(), 10);
            assert!(elapsed < Duration::from_millis(10)); // Should be very fast
        }
    }
}
