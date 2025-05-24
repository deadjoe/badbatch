//! Sequence Barrier implementation for the Disruptor
//!
//! The Sequence Barrier coordinates access between producers and consumers,
//! ensuring that consumers don't overtake producers and managing dependencies
//! between different consumers.

use crate::disruptor::{Sequence, WaitStrategy, Result};
use std::sync::Arc;

/// A barrier that coordinates access to the ring buffer
///
/// The sequence barrier contains the logic to determine if there are events
/// available for a consumer to process, taking into account both the main
/// published sequence and any dependent consumer sequences.
pub struct SequenceBarrier {
    /// The wait strategy to use when waiting for sequences
    wait_strategy: Arc<dyn WaitStrategy>,
    /// The main cursor sequence from the sequencer
    cursor_sequence: Arc<Sequence>,
    /// Sequences of consumers that this barrier depends on
    dependent_sequences: Vec<Arc<Sequence>>,
    /// Flag indicating if the barrier has been alerted (shutdown)
    alerted: Arc<std::sync::atomic::AtomicBool>,
}

impl SequenceBarrier {
    /// Create a new sequence barrier
    ///
    /// # Arguments
    /// * `wait_strategy` - The wait strategy to use
    /// * `cursor_sequence` - The main cursor sequence
    /// * `dependent_sequences` - Sequences this barrier depends on
    pub fn new(
        wait_strategy: Arc<dyn WaitStrategy>,
        cursor_sequence: Arc<Sequence>,
        dependent_sequences: Vec<Arc<Sequence>>,
    ) -> Self {
        Self {
            wait_strategy,
            cursor_sequence,
            dependent_sequences,
            alerted: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Wait for the given sequence to become available
    ///
    /// This method will block until the requested sequence is available,
    /// taking into account both the main cursor and any dependent sequences.
    ///
    /// # Arguments
    /// * `sequence` - The sequence to wait for
    ///
    /// # Returns
    /// The highest available sequence, or an error if alerted/timeout
    pub fn wait_for(&self, sequence: i64) -> Result<i64> {
        self.check_alert()?;
        
        let available_sequence = self.wait_strategy.wait_for(
            sequence,
            &self.cursor_sequence,
            &self.dependent_sequences,
        )?;
        
        self.check_alert()?;
        
        Ok(available_sequence)
    }

    /// Get the current cursor value
    pub fn get_cursor(&self) -> i64 {
        self.cursor_sequence.get()
    }

    /// Check if the barrier has been alerted (shutdown)
    pub fn is_alerted(&self) -> bool {
        self.alerted.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Alert the barrier to wake up any waiting consumers
    pub fn alert(&self) {
        self.alerted.store(true, std::sync::atomic::Ordering::Release);
        self.wait_strategy.signal_all_when_blocking();
    }

    /// Clear the alert status
    pub fn clear_alert(&self) {
        self.alerted.store(false, std::sync::atomic::Ordering::Release);
    }

    /// Check if alerted and return error if so
    fn check_alert(&self) -> Result<()> {
        if self.is_alerted() {
            Err(crate::disruptor::DisruptorError::Shutdown)
        } else {
            Ok(())
        }
    }

    /// Get the minimum sequence from all dependent sequences
    pub fn get_minimum_dependent_sequence(&self) -> i64 {
        if self.dependent_sequences.is_empty() {
            i64::MAX
        } else {
            self.dependent_sequences
                .iter()
                .map(|seq| seq.get())
                .min()
                .unwrap_or(i64::MAX)
        }
    }

    /// Add a dependent sequence
    pub fn add_dependent_sequence(&mut self, sequence: Arc<Sequence>) {
        self.dependent_sequences.push(sequence);
    }

    /// Remove a dependent sequence
    pub fn remove_dependent_sequence(&mut self, sequence: &Arc<Sequence>) -> bool {
        if let Some(pos) = self.dependent_sequences.iter().position(|s| Arc::ptr_eq(s, sequence)) {
            self.dependent_sequences.remove(pos);
            true
        } else {
            false
        }
    }

    /// Get the number of dependent sequences
    pub fn dependent_sequence_count(&self) -> usize {
        self.dependent_sequences.len()
    }
}

impl Clone for SequenceBarrier {
    fn clone(&self) -> Self {
        Self {
            wait_strategy: Arc::clone(&self.wait_strategy),
            cursor_sequence: Arc::clone(&self.cursor_sequence),
            dependent_sequences: self.dependent_sequences.clone(),
            alerted: Arc::clone(&self.alerted),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disruptor::{BlockingWaitStrategy, YieldingWaitStrategy};
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_sequence_barrier_creation() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let cursor = Arc::new(Sequence::new(10));
        let dependencies = vec![Arc::new(Sequence::new(5))];
        
        let barrier = SequenceBarrier::new(wait_strategy, cursor.clone(), dependencies);
        
        assert_eq!(barrier.get_cursor(), 10);
        assert_eq!(barrier.dependent_sequence_count(), 1);
        assert!(!barrier.is_alerted());
    }

    #[test]
    fn test_sequence_barrier_wait_for_available() {
        let wait_strategy = Arc::new(YieldingWaitStrategy::new());
        let cursor = Arc::new(Sequence::new(10));
        let dependencies = vec![];
        
        let barrier = SequenceBarrier::new(wait_strategy, cursor, dependencies);
        
        // Should return immediately since cursor is already >= sequence
        let result = barrier.wait_for(5);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 10);
    }

    #[test]
    fn test_sequence_barrier_with_dependencies() {
        let wait_strategy = Arc::new(YieldingWaitStrategy::new());
        let cursor = Arc::new(Sequence::new(10));
        let dep1 = Arc::new(Sequence::new(8));
        let dep2 = Arc::new(Sequence::new(9));
        let dependencies = vec![dep1.clone(), dep2.clone()];
        
        let barrier = SequenceBarrier::new(wait_strategy, cursor, dependencies);
        
        assert_eq!(barrier.get_minimum_dependent_sequence(), 8);
        
        // Update dependencies
        dep1.set(12);
        dep2.set(11);
        
        assert_eq!(barrier.get_minimum_dependent_sequence(), 11);
    }

    #[test]
    fn test_sequence_barrier_alert() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let cursor = Arc::new(Sequence::new(5));
        let dependencies = vec![];
        
        let barrier = SequenceBarrier::new(wait_strategy, cursor, dependencies);
        
        assert!(!barrier.is_alerted());
        
        barrier.alert();
        assert!(barrier.is_alerted());
        
        // Should return error when alerted
        let result = barrier.wait_for(10);
        assert!(result.is_err());
        
        barrier.clear_alert();
        assert!(!barrier.is_alerted());
    }

    #[test]
    fn test_sequence_barrier_add_remove_dependencies() {
        let wait_strategy = Arc::new(YieldingWaitStrategy::new());
        let cursor = Arc::new(Sequence::new(10));
        let dependencies = vec![];
        
        let mut barrier = SequenceBarrier::new(wait_strategy, cursor, dependencies);
        
        assert_eq!(barrier.dependent_sequence_count(), 0);
        
        let dep1 = Arc::new(Sequence::new(5));
        let dep2 = Arc::new(Sequence::new(7));
        
        barrier.add_dependent_sequence(dep1.clone());
        barrier.add_dependent_sequence(dep2.clone());
        
        assert_eq!(barrier.dependent_sequence_count(), 2);
        assert_eq!(barrier.get_minimum_dependent_sequence(), 5);
        
        assert!(barrier.remove_dependent_sequence(&dep1));
        assert_eq!(barrier.dependent_sequence_count(), 1);
        assert_eq!(barrier.get_minimum_dependent_sequence(), 7);
        
        assert!(!barrier.remove_dependent_sequence(&dep1)); // Already removed
    }

    #[test]
    fn test_sequence_barrier_clone() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let cursor = Arc::new(Sequence::new(10));
        let dependencies = vec![Arc::new(Sequence::new(5))];
        
        let barrier = SequenceBarrier::new(wait_strategy, cursor, dependencies);
        let cloned_barrier = barrier.clone();
        
        assert_eq!(barrier.get_cursor(), cloned_barrier.get_cursor());
        assert_eq!(barrier.dependent_sequence_count(), cloned_barrier.dependent_sequence_count());
        assert_eq!(barrier.is_alerted(), cloned_barrier.is_alerted());
        
        // Alert original should affect clone
        barrier.alert();
        assert!(cloned_barrier.is_alerted());
    }

    #[test]
    fn test_sequence_barrier_concurrent_access() {
        let wait_strategy = Arc::new(YieldingWaitStrategy::new());
        let cursor = Arc::new(Sequence::new(0));
        let dependencies = vec![];
        
        let barrier = Arc::new(SequenceBarrier::new(wait_strategy, cursor.clone(), dependencies));
        let barrier_clone = Arc::clone(&barrier);
        
        // Start a thread that will update the cursor
        let cursor_clone = Arc::clone(&cursor);
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            cursor_clone.set(10);
        });
        
        // Wait for the sequence to become available
        let result = barrier_clone.wait_for(5);
        assert!(result.is_ok());
        assert!(result.unwrap() >= 5);
        
        handle.join().unwrap();
    }
}
