//! Sequence Implementation
//!
//! This module provides the Sequence type, which is the core coordination primitive
//! in the LMAX Disruptor pattern. Sequences are used to track progress through the
//! ring buffer and coordinate between producers and consumers.
//!
//! The implementation follows the original LMAX Disruptor design with optimizations
//! for preventing false sharing and ensuring memory ordering.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use crate::disruptor::INITIAL_CURSOR_VALUE;

/// A sequence counter that provides atomic operations with memory ordering guarantees.
/// 
/// This is equivalent to the Sequence class in the original LMAX Disruptor.
/// It uses padding to prevent false sharing between sequence values and provides
/// atomic operations with appropriate memory barriers.
#[repr(align(64))] // Cache line alignment to prevent false sharing
#[derive(Debug)]
pub struct Sequence {
    /// The actual sequence value
    value: AtomicI64,
    /// Padding to prevent false sharing (cache line is typically 64 bytes)
    _padding: [u8; 56], // 64 - 8 = 56 bytes padding
}

impl Sequence {
    /// Create a new sequence with the initial value
    /// 
    /// # Arguments
    /// * `initial_value` - The initial sequence value (typically INITIAL_CURSOR_VALUE)
    /// 
    /// # Returns
    /// A new Sequence instance
    pub fn new(initial_value: i64) -> Self {
        Self {
            value: AtomicI64::new(initial_value),
            _padding: [0; 56],
        }
    }

    /// Create a new sequence with the default initial value
    /// 
    /// # Returns
    /// A new Sequence instance with INITIAL_CURSOR_VALUE
    pub fn new_with_initial_value() -> Self {
        Self::new(INITIAL_CURSOR_VALUE)
    }

    /// Get the current sequence value
    /// 
    /// Uses Acquire ordering to ensure proper synchronization with other threads.
    /// 
    /// # Returns
    /// The current sequence value
    pub fn get(&self) -> i64 {
        self.value.load(Ordering::Acquire)
    }

    /// Set the sequence value
    /// 
    /// Uses Release ordering to ensure proper synchronization with other threads.
    /// 
    /// # Arguments
    /// * `value` - The new sequence value
    pub fn set(&self, value: i64) {
        self.value.store(value, Ordering::Release);
    }

    /// Set the sequence value with volatile semantics
    /// 
    /// Uses SeqCst ordering for strongest memory ordering guarantees.
    /// This is equivalent to the setVolatile method in the original LMAX Disruptor.
    /// 
    /// # Arguments
    /// * `value` - The new sequence value
    pub fn set_volatile(&self, value: i64) {
        self.value.store(value, Ordering::SeqCst);
    }

    /// Compare and swap the sequence value
    /// 
    /// Atomically compares the current value with expected and sets it to new_value
    /// if they match.
    /// 
    /// # Arguments
    /// * `expected` - The expected current value
    /// * `new_value` - The new value to set if comparison succeeds
    /// 
    /// # Returns
    /// True if the swap was successful, false otherwise
    pub fn compare_and_set(&self, expected: i64, new_value: i64) -> bool {
        self.value
            .compare_exchange(expected, new_value, Ordering::SeqCst, Ordering::Acquire)
            .is_ok()
    }

    /// Increment and get the new value
    /// 
    /// Atomically increments the sequence value and returns the new value.
    /// 
    /// # Returns
    /// The new sequence value after incrementing
    pub fn increment_and_get(&self) -> i64 {
        self.value.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Add a value and get the new result
    /// 
    /// Atomically adds the increment to the sequence value and returns the new value.
    /// 
    /// # Arguments
    /// * `increment` - The value to add
    /// 
    /// # Returns
    /// The new sequence value after adding the increment
    pub fn add_and_get(&self, increment: i64) -> i64 {
        self.value.fetch_add(increment, Ordering::SeqCst) + increment
    }
}

impl Default for Sequence {
    fn default() -> Self {
        Self::new_with_initial_value()
    }
}

impl Clone for Sequence {
    fn clone(&self) -> Self {
        Self::new(self.get())
    }
}

/// Utility functions for working with sequences
impl Sequence {
    /// Get the minimum sequence value from a slice of sequences
    /// 
    /// This is used to find the slowest consumer when coordinating multiple consumers.
    /// 
    /// # Arguments
    /// * `sequences` - A slice of sequence references
    /// 
    /// # Returns
    /// The minimum sequence value, or i64::MAX if the slice is empty
    pub fn get_minimum_sequence(sequences: &[Arc<Sequence>]) -> i64 {
        if sequences.is_empty() {
            return i64::MAX;
        }

        let mut minimum = sequences[0].get();
        for sequence in sequences.iter().skip(1) {
            let value = sequence.get();
            if value < minimum {
                minimum = value;
            }
        }
        minimum
    }
}

// Ensure Sequence is Send and Sync for multi-threading
unsafe impl Send for Sequence {}
unsafe impl Sync for Sequence {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn test_sequence_creation() {
        let seq = Sequence::new(42);
        assert_eq!(seq.get(), 42);

        let seq_default = Sequence::new_with_initial_value();
        assert_eq!(seq_default.get(), INITIAL_CURSOR_VALUE);

        let seq_default2 = Sequence::default();
        assert_eq!(seq_default2.get(), INITIAL_CURSOR_VALUE);
    }

    #[test]
    fn test_sequence_operations() {
        let seq = Sequence::new(0);
        
        // Test set and get
        seq.set(10);
        assert_eq!(seq.get(), 10);

        // Test set_volatile
        seq.set_volatile(20);
        assert_eq!(seq.get(), 20);

        // Test compare_and_set
        assert!(seq.compare_and_set(20, 30));
        assert_eq!(seq.get(), 30);
        assert!(!seq.compare_and_set(20, 40)); // Should fail
        assert_eq!(seq.get(), 30);

        // Test increment_and_get
        assert_eq!(seq.increment_and_get(), 31);
        assert_eq!(seq.get(), 31);

        // Test add_and_get
        assert_eq!(seq.add_and_get(5), 36);
        assert_eq!(seq.get(), 36);
    }

    #[test]
    fn test_sequence_thread_safety() {
        let seq = Arc::new(Sequence::new(0));
        let mut handles = vec![];

        // Spawn multiple threads to increment the sequence
        for _ in 0..10 {
            let seq_clone = Arc::clone(&seq);
            let handle = thread::spawn(move || {
                for _ in 0..100 {
                    seq_clone.increment_and_get();
                }
            });
            handles.push(handle);
        }

        // Wait for all threads to complete
        for handle in handles {
            handle.join().unwrap();
        }

        // Should have incremented 10 * 100 = 1000 times
        assert_eq!(seq.get(), 1000);
    }

    #[test]
    fn test_get_minimum_sequence() {
        let seq1 = Arc::new(Sequence::new(10));
        let seq2 = Arc::new(Sequence::new(5));
        let seq3 = Arc::new(Sequence::new(15));

        let sequences = vec![seq1, seq2, seq3];
        assert_eq!(Sequence::get_minimum_sequence(&sequences), 5);

        // Test empty slice
        assert_eq!(Sequence::get_minimum_sequence(&[]), i64::MAX);
    }

    #[test]
    fn test_sequence_clone() {
        let seq1 = Sequence::new(42);
        let seq2 = seq1.clone();
        
        assert_eq!(seq1.get(), 42);
        assert_eq!(seq2.get(), 42);
        
        // Clones should be independent
        seq1.set(100);
        assert_eq!(seq1.get(), 100);
        assert_eq!(seq2.get(), 42);
    }
}
