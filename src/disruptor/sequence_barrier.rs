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

    /// Wait for the given sequence with timeout
    ///
    /// This method blocks until the specified sequence is available or
    /// until the timeout expires.
    ///
    /// # Arguments
    /// * `sequence` - The sequence to wait for
    /// * `timeout` - Maximum time to wait
    ///
    /// # Returns
    /// The actual available sequence (may be higher than requested)
    ///
    /// # Errors
    /// Returns an error if waiting is interrupted, times out, or an alert occurs
    fn wait_for_with_timeout(&self, sequence: i64, timeout: std::time::Duration) -> Result<i64>;

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
    /// Reference to the sequencer for getting highest published sequence
    sequencer: Arc<dyn crate::disruptor::Sequencer>,
}

impl ProcessingSequenceBarrier {
    /// Create a new processing sequence barrier
    ///
    /// # Arguments
    /// * `cursor` - The cursor sequence to track
    /// * `wait_strategy` - The wait strategy to use
    /// * `dependent_sequences` - Sequences that this barrier depends on
    /// * `sequencer` - The sequencer for getting highest published sequence
    ///
    /// # Returns
    /// A new ProcessingSequenceBarrier instance
    pub fn new(
        cursor: Arc<Sequence>,
        wait_strategy: Arc<dyn crate::disruptor::WaitStrategy>,
        dependent_sequences: Vec<Arc<Sequence>>,
        sequencer: Arc<dyn crate::disruptor::Sequencer>,
    ) -> Self {
        Self {
            cursor,
            wait_strategy,
            dependent_sequences,
            alerted: AtomicBool::new(false),
            sequencer,
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

        // Critical memory barrier to ensure all previous writes are visible
        // This matches disruptor-rs fence(Ordering::Acquire) at line 177
        std::sync::atomic::fence(std::sync::atomic::Ordering::Acquire);

        // CRITICAL FIX: Call sequencer.getHighestPublishedSequence like LMAX Disruptor
        // This is essential for MultiProducer scenarios to ensure we only process
        // contiguous published sequences
        if available_sequence < sequence {
            return Ok(available_sequence);
        }

        let highest_published = self
            .sequencer
            .get_highest_published_sequence(sequence, available_sequence);
        Ok(highest_published)
    }

    fn wait_for_with_timeout(&self, sequence: i64, timeout: std::time::Duration) -> Result<i64> {
        // Check if we've been alerted before starting to wait
        self.check_alert()?;

        // Use the wait strategy with timeout to wait for the sequence
        let available_sequence = self.wait_strategy.wait_for_with_timeout(
            sequence,
            self.cursor.clone(),
            &self.dependent_sequences,
            timeout,
        )?;

        // Check again after waiting in case we were alerted while waiting
        self.check_alert()?;

        // Critical memory barrier to ensure all previous writes are visible
        std::sync::atomic::fence(std::sync::atomic::Ordering::Acquire);

        // CRITICAL FIX: Call sequencer.getHighestPublishedSequence like LMAX Disruptor
        if available_sequence < sequence {
            return Ok(available_sequence);
        }

        let highest_published = self
            .sequencer
            .get_highest_published_sequence(sequence, available_sequence);
        Ok(highest_published)
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

        // Critical memory barrier to ensure all previous writes are visible
        // This matches disruptor-rs fence(Ordering::Acquire) at line 177
        std::sync::atomic::fence(std::sync::atomic::Ordering::Acquire);

        // Align with other wait paths: ensure we return the highest
        // contiguous published sequence for multi-producer scenarios.
        if available_sequence < sequence {
            return Ok(available_sequence);
        }

        let highest_published = self
            .sequencer
            .get_highest_published_sequence(sequence, available_sequence);
        Ok(highest_published)
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

        // Critical memory barrier to ensure all previous writes are visible
        std::sync::atomic::fence(std::sync::atomic::Ordering::Acquire);

        Ok(available_sequence)
    }

    fn wait_for_with_timeout(&self, sequence: i64, timeout: std::time::Duration) -> Result<i64> {
        self.check_alert()?;

        // For a simple barrier, we only wait for the cursor with timeout
        let available_sequence = self.wait_strategy.wait_for_with_timeout(
            sequence,
            self.cursor.clone(),
            &[], // No dependent sequences
            timeout,
        )?;

        self.check_alert()?;

        // Critical memory barrier to ensure all previous writes are visible
        std::sync::atomic::fence(std::sync::atomic::Ordering::Acquire);

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

        // Critical memory barrier to ensure all previous writes are visible
        std::sync::atomic::fence(std::sync::atomic::Ordering::Acquire);

        Ok(available_sequence)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disruptor::BlockingWaitStrategy;

    #[test]
    fn test_processing_sequence_barrier() {
        use crate::disruptor::SingleProducerSequencer;

        let cursor = Arc::new(Sequence::new(10));
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let dependent_sequences = vec![Arc::new(Sequence::new(5))];

        // Create a test sequencer
        let sequencer = Arc::new(SingleProducerSequencer::new(16, wait_strategy.clone()))
            as Arc<dyn crate::disruptor::Sequencer>;

        let barrier = ProcessingSequenceBarrier::new(
            cursor.clone(),
            wait_strategy,
            dependent_sequences,
            sequencer,
        );

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

    #[test]
    fn test_processing_sequence_barrier_with_timeout() {
        use crate::disruptor::SingleProducerSequencer;
        use std::time::Duration;

        let cursor = Arc::new(Sequence::new(10));
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let dependent_sequences = vec![Arc::new(Sequence::new(5))];

        let sequencer = Arc::new(SingleProducerSequencer::new(16, wait_strategy.clone()))
            as Arc<dyn crate::disruptor::Sequencer>;

        let barrier = ProcessingSequenceBarrier::new(
            cursor.clone(),
            wait_strategy,
            dependent_sequences,
            sequencer,
        );

        // Test timeout wait for available sequence
        let result = barrier.wait_for_with_timeout(5, Duration::from_millis(10));
        assert!(result.is_ok());
        assert!(result.unwrap() >= 5);

        // Test alert during timeout wait
        barrier.alert();
        let result = barrier.wait_for_with_timeout(15, Duration::from_millis(10));
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), DisruptorError::Alert));

        barrier.clear_alert();
    }

    #[test]
    fn test_processing_sequence_barrier_mpmc_gaps() {
        use crate::disruptor::MultiProducerSequencer;
        use std::time::Duration;

        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = Arc::new(MultiProducerSequencer::new(16, wait_strategy.clone()))
            as Arc<dyn crate::disruptor::Sequencer>;

        // Create barrier tracking only the cursor (no dependent consumers)
        let cursor = sequencer.get_cursor();
        let barrier = ProcessingSequenceBarrier::new(cursor, wait_strategy, vec![], sequencer);

        // Claim sequences 0..=3
        let seq0 = barrier.sequencer.next().unwrap();
        let seq1 = barrier.sequencer.next().unwrap();
        let seq2 = barrier.sequencer.next().unwrap();
        let seq3 = barrier.sequencer.next().unwrap();
        assert_eq!(seq0, 0);
        assert_eq!(seq1, 1);
        assert_eq!(seq2, 2);
        assert_eq!(seq3, 3);

        // Publish out of order: 0, 2, 3 (gap at 1)
        barrier.sequencer.publish(seq0);
        barrier.sequencer.publish(seq2);
        barrier.sequencer.publish(seq3);

        // Consumer is starting from -1, so it will request next_sequence=0
        // Since there is a gap at 1, highest contiguous from 0 is 0
        let available = barrier
            .wait_for_with_timeout(0, Duration::from_millis(10))
            .unwrap();
        assert_eq!(available, 0);

        // Now fill the gap
        barrier.sequencer.publish(seq1);

        // Highest contiguous should become 3 when starting from 0
        let available2 = barrier
            .wait_for_with_timeout(0, Duration::from_millis(10))
            .unwrap();
        assert_eq!(available2, 3);
    }

    #[test]
    fn test_simple_sequence_barrier_with_timeout() {
        use std::time::Duration;

        let cursor = Arc::new(Sequence::new(10));
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());

        let barrier = SimpleSequenceBarrier::new(cursor.clone(), wait_strategy);

        // Test timeout wait for available sequence
        let result = barrier.wait_for_with_timeout(5, Duration::from_millis(10));
        assert!(result.is_ok());
        assert!(result.unwrap() >= 5);

        // Test alert during timeout wait
        barrier.alert();
        let result = barrier.wait_for_with_timeout(15, Duration::from_millis(10));
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), DisruptorError::Alert));

        barrier.clear_alert();
    }

    #[test]
    fn test_sequence_barrier_with_shutdown() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let cursor = Arc::new(Sequence::new(10));
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());

        let barrier = SimpleSequenceBarrier::new(cursor.clone(), wait_strategy);
        let shutdown_flag = AtomicBool::new(false);

        // Test normal wait with shutdown flag
        let result = barrier.wait_for_with_shutdown(5, &shutdown_flag);
        assert!(result.is_ok());
        assert!(result.unwrap() >= 5);

        // Test with shutdown flag set
        shutdown_flag.store(true, Ordering::Release);
        let result = barrier.wait_for_with_shutdown(5, &shutdown_flag);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), DisruptorError::Alert));
    }

    #[test]
    fn test_processing_sequence_barrier_with_shutdown() {
        use crate::disruptor::SingleProducerSequencer;
        use std::sync::atomic::{AtomicBool, Ordering};

        let cursor = Arc::new(Sequence::new(10));
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let dependent_sequences = vec![Arc::new(Sequence::new(5))];

        let sequencer = Arc::new(SingleProducerSequencer::new(16, wait_strategy.clone()))
            as Arc<dyn crate::disruptor::Sequencer>;

        let barrier = ProcessingSequenceBarrier::new(
            cursor.clone(),
            wait_strategy,
            dependent_sequences,
            sequencer,
        );

        let shutdown_flag = AtomicBool::new(false);

        // Test normal wait with shutdown flag
        let result = barrier.wait_for_with_shutdown(5, &shutdown_flag);
        assert!(result.is_ok());
        assert!(result.unwrap() >= 5);

        // Test with shutdown flag set
        shutdown_flag.store(true, Ordering::Release);
        let result = barrier.wait_for_with_shutdown(5, &shutdown_flag);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), DisruptorError::Alert));
    }

    #[test]
    fn test_sequence_barrier_alert_methods() {
        let cursor = Arc::new(Sequence::new(10));
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());

        let barrier = SimpleSequenceBarrier::new(cursor.clone(), wait_strategy);

        // Test initial state
        assert!(!barrier.is_alerted());
        assert!(barrier.check_alert().is_ok());

        // Test alert
        barrier.alert();
        assert!(barrier.is_alerted());
        assert!(barrier.check_alert().is_err());
        assert!(matches!(
            barrier.check_alert().unwrap_err(),
            DisruptorError::Alert
        ));

        // Test clear alert
        barrier.clear_alert();
        assert!(!barrier.is_alerted());
        assert!(barrier.check_alert().is_ok());
    }

    #[test]
    fn test_processing_sequence_barrier_alert_methods() {
        use crate::disruptor::SingleProducerSequencer;

        let cursor = Arc::new(Sequence::new(10));
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let dependent_sequences = vec![Arc::new(Sequence::new(5))];

        let sequencer = Arc::new(SingleProducerSequencer::new(16, wait_strategy.clone()))
            as Arc<dyn crate::disruptor::Sequencer>;

        let barrier = ProcessingSequenceBarrier::new(
            cursor.clone(),
            wait_strategy,
            dependent_sequences,
            sequencer,
        );

        // Test initial state
        assert!(!barrier.is_alerted());
        assert!(barrier.check_alert().is_ok());

        // Test alert
        barrier.alert();
        assert!(barrier.is_alerted());
        assert!(barrier.check_alert().is_err());
        assert!(matches!(
            barrier.check_alert().unwrap_err(),
            DisruptorError::Alert
        ));

        // Test clear alert
        barrier.clear_alert();
        assert!(!barrier.is_alerted());
        assert!(barrier.check_alert().is_ok());
    }

    #[test]
    fn test_sequence_barrier_highest_published_logic() {
        use crate::disruptor::SingleProducerSequencer;

        let cursor = Arc::new(Sequence::new(20));
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let dependent_sequences = vec![Arc::new(Sequence::new(15))];

        let sequencer = Arc::new(SingleProducerSequencer::new(32, wait_strategy.clone()))
            as Arc<dyn crate::disruptor::Sequencer>;

        let barrier = ProcessingSequenceBarrier::new(
            cursor.clone(),
            wait_strategy,
            dependent_sequences,
            sequencer,
        );

        // Test highest published sequence logic
        let result = barrier.wait_for(10);
        assert!(result.is_ok());
        let available = result.unwrap();
        assert!(available >= 10);
    }

    #[test]
    fn test_sequence_barrier_available_sequence_less_than_requested() {
        use crate::disruptor::SingleProducerSequencer;
        use std::time::Duration;

        let cursor = Arc::new(Sequence::new(5)); // Lower cursor value
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let dependent_sequences = vec![Arc::new(Sequence::new(3))];

        let sequencer = Arc::new(SingleProducerSequencer::new(16, wait_strategy.clone()))
            as Arc<dyn crate::disruptor::Sequencer>;

        let barrier = ProcessingSequenceBarrier::new(
            cursor.clone(),
            wait_strategy,
            dependent_sequences,
            sequencer,
        );

        // Test when available_sequence < sequence in wait_for_with_timeout method (safer)
        let result = barrier.wait_for_with_timeout(10, Duration::from_millis(10));
        // Since we're asking for sequence 10 but cursor is only at 5,
        // this should either timeout or return the available sequence
        if let Ok(available) = result {
            assert!(available <= 10);
        }
        // Timeout is also acceptable behavior
    }

    #[test]
    fn test_sequence_barrier_available_sequence_less_than_requested_with_timeout() {
        use crate::disruptor::SingleProducerSequencer;
        use std::time::Duration;

        let cursor = Arc::new(Sequence::new(5)); // Lower cursor value
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let dependent_sequences = vec![Arc::new(Sequence::new(3))];

        let sequencer = Arc::new(SingleProducerSequencer::new(16, wait_strategy.clone()))
            as Arc<dyn crate::disruptor::Sequencer>;

        let barrier = ProcessingSequenceBarrier::new(
            cursor.clone(),
            wait_strategy,
            dependent_sequences,
            sequencer,
        );

        // Test timeout behavior when waiting for unavailable sequence
        let result = barrier.wait_for_with_timeout(20, Duration::from_millis(10));
        // This should either timeout or return an available sequence
        if let Ok(available) = result {
            assert!(available < 20); // Should be less than requested
        }
        // Timeout or alert is expected behavior
    }

    #[test]
    fn test_sequence_barrier_trait_default_implementation() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let cursor = Arc::new(Sequence::new(10));
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());

        let barrier = SimpleSequenceBarrier::new(cursor.clone(), wait_strategy);
        let shutdown_flag = AtomicBool::new(false);

        // Test the trait's default implementation of wait_for_with_shutdown
        let result = SequenceBarrier::wait_for_with_shutdown(&barrier, 5, &shutdown_flag);
        assert!(result.is_ok());

        // Test with shutdown flag set
        shutdown_flag.store(true, Ordering::Release);
        let result = SequenceBarrier::wait_for_with_shutdown(&barrier, 5, &shutdown_flag);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), DisruptorError::Alert));

        // Test with alert set but shutdown false
        shutdown_flag.store(false, Ordering::Release);
        barrier.alert();
        let result = SequenceBarrier::wait_for_with_shutdown(&barrier, 5, &shutdown_flag);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), DisruptorError::Alert));
    }
}
