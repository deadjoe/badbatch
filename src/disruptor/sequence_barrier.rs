//! Sequence Barrier Implementation
//!
//! This module provides sequence barriers for coordinating dependencies between
//! event processors in the Disruptor pattern. Sequence barriers ensure that
//! consumers don't process events until their dependencies have been satisfied.

use crate::disruptor::{DisruptorError, Result, Sequence};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Coordination barrier for managing dependencies between event processors
///
/// This trait defines the interface for sequence barriers, which are used to
/// coordinate dependencies between event processors. This follows the exact
/// design from the original LMAX Disruptor SequenceBarrier interface.
pub trait SequenceBarrier: Send + Sync {
    /// Wait for the given sequence to become available
    ///
    /// This method blocks until the specified sequence is available for processing,
    /// taking into account any dependent sequences that must be processed first.
    ///
    /// # Arguments
    /// * `sequence` - The sequence to wait for
    ///
    /// # Returns
    /// The actual available sequence (may be higher than requested)
    ///
    /// # Errors
    /// Returns an error if waiting is interrupted or an alert occurs
    fn wait_for(&self, sequence: i64) -> Result<i64>;

    /// Get the cursor sequence that this barrier is tracking
    ///
    /// # Returns
    /// The cursor sequence
    fn get_cursor(&self) -> Arc<Sequence>;

    /// Check if this barrier has been alerted
    ///
    /// # Returns
    /// True if the barrier has been alerted, false otherwise
    fn is_alerted(&self) -> bool;

    /// Alert this barrier to wake up any waiting threads
    ///
    /// This is used to interrupt waiting threads, typically during shutdown.
    fn alert(&self);

    /// Clear the alert status
    ///
    /// This resets the alert flag so that the barrier can be used again.
    fn clear_alert(&self);

    /// Check if the barrier is alerted and throw an exception if it is
    ///
    /// # Returns
    /// Ok(()) if not alerted
    ///
    /// # Errors
    /// Returns `DisruptorError::Alert` if the barrier has been alerted
    fn check_alert(&self) -> Result<()>;

    /// Wait for a sequence with external shutdown signal
    ///
    /// This method waits for the specified sequence to become available,
    /// but also checks an external shutdown flag for early termination.
    ///
    /// # Arguments
    /// * `sequence` - The sequence to wait for
    /// * `shutdown_flag` - External shutdown signal to check
    ///
    /// # Returns
    /// The available sequence number when ready
    ///
    /// # Errors
    /// Returns `DisruptorError::Alert` if shutdown is signaled or barrier is alerted
    fn wait_for_with_shutdown(&self, sequence: i64, shutdown_flag: &AtomicBool) -> Result<i64> {
        // Check shutdown flag first
        if shutdown_flag.load(Ordering::Acquire) {
            return Err(DisruptorError::Alert);
        }

        // Check if we're alerted
        self.check_alert()?;

        // Try to wait for the sequence (this might block)
        self.wait_for(sequence)
    }
}

/// Standard implementation of a sequence barrier
///
/// This barrier coordinates between a cursor sequence (typically from a sequencer)
/// and a set of dependent sequences (typically from other event processors).
/// It ensures that events are not processed until all dependencies are satisfied.
#[derive(Debug)]
pub struct ProcessingSequenceBarrier {
    /// The main cursor sequence to track
    cursor: Arc<Sequence>,
    /// The wait strategy to use when waiting for sequences
    wait_strategy: Arc<dyn crate::disruptor::WaitStrategy>,
    /// Sequences that this barrier depends on
    dependent_sequences: Vec<Arc<Sequence>>,
    /// Alert flag for interrupting waiting threads
    alerted: AtomicBool,
}

impl ProcessingSequenceBarrier {
    /// Create a new processing sequence barrier
    ///
    /// # Arguments
    /// * `cursor` - The cursor sequence to track
    /// * `wait_strategy` - The wait strategy to use
    /// * `dependent_sequences` - Sequences that this barrier depends on
    ///
    /// # Returns
    /// A new ProcessingSequenceBarrier instance
    pub fn new(
        cursor: Arc<Sequence>,
        wait_strategy: Arc<dyn crate::disruptor::WaitStrategy>,
        dependent_sequences: Vec<Arc<Sequence>>,
    ) -> Self {
        Self {
            cursor,
            wait_strategy,
            dependent_sequences,
            alerted: AtomicBool::new(false),
        }
    }
}

impl SequenceBarrier for ProcessingSequenceBarrier {
    fn wait_for(&self, sequence: i64) -> Result<i64> {
        // Check if we've been alerted before starting to wait
        self.check_alert()?;

        // Use the wait strategy to wait for the sequence
        let available_sequence = self.wait_strategy.wait_for(
            sequence,
            self.cursor.clone(),
            &self.dependent_sequences,
        )?;

        // Check again after waiting in case we were alerted while waiting
        self.check_alert()?;

        Ok(available_sequence)
    }

    fn get_cursor(&self) -> Arc<Sequence> {
        self.cursor.clone()
    }

    fn is_alerted(&self) -> bool {
        self.alerted.load(Ordering::Acquire)
    }

    fn alert(&self) {
        self.alerted.store(true, Ordering::Release);
        self.wait_strategy.signal_all_when_blocking();
    }

    fn clear_alert(&self) {
        self.alerted.store(false, Ordering::Release);
    }

    fn check_alert(&self) -> Result<()> {
        if self.is_alerted() {
            Err(DisruptorError::Alert)
        } else {
            Ok(())
        }
    }

    fn wait_for_with_shutdown(&self, sequence: i64, shutdown_flag: &AtomicBool) -> Result<i64> {
        // Check shutdown flag first
        if shutdown_flag.load(Ordering::Acquire) {
            return Err(DisruptorError::Alert);
        }

        // Check if we've been alerted before starting to wait
        self.check_alert()?;

        // Use the wait strategy with shutdown support
        let available_sequence = self.wait_strategy.wait_for_with_shutdown(
            sequence,
            self.cursor.clone(),
            &self.dependent_sequences,
            shutdown_flag,
        )?;

        // Check again after waiting in case we were alerted while waiting
        self.check_alert()?;

        Ok(available_sequence)
    }
}

/// A simple sequence barrier that only tracks a cursor
///
/// This is a simplified barrier that doesn't have any dependent sequences.
/// It's useful for the first consumer in a processing chain.
#[derive(Debug)]
pub struct SimpleSequenceBarrier {
    /// The cursor sequence to track
    cursor: Arc<Sequence>,
    /// The wait strategy to use
    wait_strategy: Arc<dyn crate::disruptor::WaitStrategy>,
    /// Alert flag
    alerted: AtomicBool,
}

impl SimpleSequenceBarrier {
    /// Create a new simple sequence barrier
    ///
    /// # Arguments
    /// * `cursor` - The cursor sequence to track
    /// * `wait_strategy` - The wait strategy to use
    ///
    /// # Returns
    /// A new SimpleSequenceBarrier instance
    pub fn new(
        cursor: Arc<Sequence>,
        wait_strategy: Arc<dyn crate::disruptor::WaitStrategy>,
    ) -> Self {
        Self {
            cursor,
            wait_strategy,
            alerted: AtomicBool::new(false),
        }
    }
}

impl SequenceBarrier for SimpleSequenceBarrier {
    fn wait_for(&self, sequence: i64) -> Result<i64> {
        self.check_alert()?;

        // For a simple barrier, we only wait for the cursor
        let available_sequence = self.wait_strategy.wait_for(
            sequence,
            self.cursor.clone(),
            &[], // No dependent sequences
        )?;

        self.check_alert()?;
        Ok(available_sequence)
    }

    fn get_cursor(&self) -> Arc<Sequence> {
        self.cursor.clone()
    }

    fn is_alerted(&self) -> bool {
        self.alerted.load(Ordering::Acquire)
    }

    fn alert(&self) {
        self.alerted.store(true, Ordering::Release);
        self.wait_strategy.signal_all_when_blocking();
    }

    fn clear_alert(&self) {
        self.alerted.store(false, Ordering::Release);
    }

    fn check_alert(&self) -> Result<()> {
        if self.is_alerted() {
            Err(DisruptorError::Alert)
        } else {
            Ok(())
        }
    }

    fn wait_for_with_shutdown(&self, sequence: i64, shutdown_flag: &AtomicBool) -> Result<i64> {
        // Check shutdown flag first
        if shutdown_flag.load(Ordering::Acquire) {
            return Err(DisruptorError::Alert);
        }

        self.check_alert()?;

        // For a simple barrier, we only wait for the cursor with shutdown support
        let available_sequence = self.wait_strategy.wait_for_with_shutdown(
            sequence,
            self.cursor.clone(),
            &[], // No dependent sequences
            shutdown_flag,
        )?;

        self.check_alert()?;
        Ok(available_sequence)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disruptor::BlockingWaitStrategy;

    #[test]
    fn test_processing_sequence_barrier() {
        let cursor = Arc::new(Sequence::new(10));
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let dependent_sequences = vec![Arc::new(Sequence::new(5))];

        let barrier =
            ProcessingSequenceBarrier::new(cursor.clone(), wait_strategy, dependent_sequences);

        // Should be able to wait for a sequence that's already available
        let result = barrier.wait_for(5);
        assert!(result.is_ok());
        assert!(result.unwrap() >= 5);

        // Test alert functionality
        assert!(!barrier.is_alerted());
        barrier.alert();
        assert!(barrier.is_alerted());

        // Should fail when alerted
        let result = barrier.wait_for(15);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), DisruptorError::Alert));

        // Clear alert and try again
        barrier.clear_alert();
        assert!(!barrier.is_alerted());
        assert!(barrier.check_alert().is_ok());
    }

    #[test]
    fn test_simple_sequence_barrier() {
        let cursor = Arc::new(Sequence::new(10));
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());

        let barrier = SimpleSequenceBarrier::new(cursor.clone(), wait_strategy);

        // Should be able to wait for a sequence that's already available
        let result = barrier.wait_for(5);
        assert!(result.is_ok());
        assert!(result.unwrap() >= 5);

        // Test alert functionality
        assert!(!barrier.is_alerted());
        barrier.alert();
        assert!(barrier.is_alerted());

        barrier.clear_alert();
        assert!(!barrier.is_alerted());
    }

    #[test]
    fn test_barrier_cursor_access() {
        let cursor = Arc::new(Sequence::new(42));
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());

        let barrier = SimpleSequenceBarrier::new(cursor.clone(), wait_strategy);
        let barrier_cursor = barrier.get_cursor();

        assert_eq!(barrier_cursor.get(), 42);
        assert!(Arc::ptr_eq(&cursor, &barrier_cursor));
    }
}
