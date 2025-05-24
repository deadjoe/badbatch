//! Sequence implementation for the Disruptor
//!
//! The Sequence is used to track progress through the ring buffer and coordinate
//! between producers and consumers. It provides atomic operations while preventing
//! false sharing through careful memory layout.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

/// Cache line size for padding to prevent false sharing
const CACHE_LINE_SIZE: usize = 64;

/// A sequence number that prevents false sharing
///
/// This structure is carefully designed to prevent false sharing by padding
/// the atomic value to ensure it occupies its own cache line.
#[repr(align(64))]
pub struct Sequence {
    /// The actual sequence value
    value: AtomicI64,
    /// Padding to prevent false sharing (cache line size - size of AtomicI64)
    _padding: [u8; CACHE_LINE_SIZE - std::mem::size_of::<AtomicI64>()],
}

impl Sequence {
    /// Create a new sequence with the given initial value
    pub fn new(initial_value: i64) -> Self {
        Self {
            value: AtomicI64::new(initial_value),
            _padding: [0; CACHE_LINE_SIZE - std::mem::size_of::<AtomicI64>()],
        }
    }

    /// Get the current sequence value
    #[inline]
    pub fn get(&self) -> i64 {
        self.value.load(Ordering::Acquire)
    }

    /// Set the sequence value
    #[inline]
    pub fn set(&self, value: i64) {
        self.value.store(value, Ordering::Release);
    }

    /// Set the sequence value with volatile semantics
    #[inline]
    pub fn set_volatile(&self, value: i64) {
        self.value.store(value, Ordering::SeqCst);
    }

    /// Compare and swap the sequence value
    #[inline]
    pub fn compare_and_swap(&self, expected: i64, new: i64) -> Result<i64, i64> {
        self.value.compare_exchange_weak(expected, new, Ordering::AcqRel, Ordering::Acquire)
    }

    /// Increment and get the new value
    #[inline]
    pub fn increment_and_get(&self) -> i64 {
        self.value.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// Add a value and get the new result
    #[inline]
    pub fn add_and_get(&self, increment: i64) -> i64 {
        self.value.fetch_add(increment, Ordering::AcqRel) + increment
    }

    /// Get the current value and then increment
    #[inline]
    pub fn get_and_increment(&self) -> i64 {
        self.value.fetch_add(1, Ordering::AcqRel)
    }

    /// Get the current value and then add
    #[inline]
    pub fn get_and_add(&self, increment: i64) -> i64 {
        self.value.fetch_add(increment, Ordering::AcqRel)
    }
}

impl Default for Sequence {
    fn default() -> Self {
        Self::new(crate::disruptor::INITIAL_CURSOR_VALUE)
    }
}

impl std::fmt::Debug for Sequence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sequence")
            .field("value", &self.get())
            .finish()
    }
}

impl std::fmt::Display for Sequence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.get())
    }
}

/// A group of sequences that can be tracked together
pub struct SequenceGroup {
    sequences: Vec<Arc<Sequence>>,
}

impl SequenceGroup {
    /// Create a new empty sequence group
    pub fn new() -> Self {
        Self {
            sequences: Vec::new(),
        }
    }

    /// Add a sequence to the group
    pub fn add(&mut self, sequence: Arc<Sequence>) {
        self.sequences.push(sequence);
    }

    /// Remove a sequence from the group
    pub fn remove(&mut self, sequence: &Arc<Sequence>) -> bool {
        if let Some(pos) = self.sequences.iter().position(|s| Arc::ptr_eq(s, sequence)) {
            self.sequences.remove(pos);
            true
        } else {
            false
        }
    }

    /// Get the minimum sequence value from all sequences in the group
    pub fn get_minimum_sequence(&self) -> i64 {
        self.sequences
            .iter()
            .map(|seq| seq.get())
            .min()
            .unwrap_or(i64::MAX)
    }

    /// Get the number of sequences in the group
    pub fn len(&self) -> usize {
        self.sequences.len()
    }

    /// Check if the group is empty
    pub fn is_empty(&self) -> bool {
        self.sequences.is_empty()
    }

    /// Get all sequences as a slice
    pub fn sequences(&self) -> &[Arc<Sequence>] {
        &self.sequences
    }
}

impl Default for SequenceGroup {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_sequence_creation() {
        let seq = Sequence::new(42);
        assert_eq!(seq.get(), 42);
    }

    #[test]
    fn test_sequence_default() {
        let seq = Sequence::default();
        assert_eq!(seq.get(), crate::disruptor::INITIAL_CURSOR_VALUE);
    }

    #[test]
    fn test_sequence_set_get() {
        let seq = Sequence::new(0);
        seq.set(100);
        assert_eq!(seq.get(), 100);
    }

    #[test]
    fn test_sequence_increment() {
        let seq = Sequence::new(0);
        assert_eq!(seq.increment_and_get(), 1);
        assert_eq!(seq.get(), 1);
        
        assert_eq!(seq.get_and_increment(), 1);
        assert_eq!(seq.get(), 2);
    }

    #[test]
    fn test_sequence_add() {
        let seq = Sequence::new(10);
        assert_eq!(seq.add_and_get(5), 15);
        assert_eq!(seq.get(), 15);
        
        assert_eq!(seq.get_and_add(3), 15);
        assert_eq!(seq.get(), 18);
    }

    #[test]
    fn test_sequence_compare_and_swap() {
        let seq = Sequence::new(10);
        
        // Successful CAS
        assert_eq!(seq.compare_and_swap(10, 20), Ok(10));
        assert_eq!(seq.get(), 20);
        
        // Failed CAS
        assert_eq!(seq.compare_and_swap(10, 30), Err(20));
        assert_eq!(seq.get(), 20);
    }

    #[test]
    fn test_sequence_group() {
        let mut group = SequenceGroup::new();
        assert!(group.is_empty());
        assert_eq!(group.len(), 0);
        
        let seq1 = Arc::new(Sequence::new(10));
        let seq2 = Arc::new(Sequence::new(20));
        let seq3 = Arc::new(Sequence::new(5));
        
        group.add(seq1.clone());
        group.add(seq2.clone());
        group.add(seq3.clone());
        
        assert_eq!(group.len(), 3);
        assert_eq!(group.get_minimum_sequence(), 5);
        
        assert!(group.remove(&seq3));
        assert_eq!(group.len(), 2);
        assert_eq!(group.get_minimum_sequence(), 10);
        
        assert!(!group.remove(&seq3)); // Already removed
    }

    #[test]
    fn test_sequence_thread_safety() {
        let seq = Arc::new(Sequence::new(0));
        let mut handles = vec![];
        
        // Spawn multiple threads to increment the sequence
        for _ in 0..10 {
            let seq_clone = Arc::clone(&seq);
            let handle = thread::spawn(move || {
                for _ in 0..1000 {
                    seq_clone.increment_and_get();
                }
            });
            handles.push(handle);
        }
        
        // Wait for all threads to complete
        for handle in handles {
            handle.join().unwrap();
        }
        
        // Should have incremented 10 * 1000 = 10000 times
        assert_eq!(seq.get(), 10000);
    }
}
