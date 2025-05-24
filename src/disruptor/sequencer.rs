//! Sequencer Implementation
//!
//! This module provides sequencer implementations for coordinating access to the ring buffer.
//! Sequencers manage the allocation of sequence numbers and ensure that producers don't
//! overwrite events that haven't been consumed yet.

use crate::disruptor::{Result, DisruptorError, Sequence, WaitStrategy};
use std::sync::Arc;

/// Trait for sequencers that coordinate access to the ring buffer
///
/// This trait defines the interface for sequencers, which are responsible
/// for allocating sequence numbers and coordinating between producers and consumers.
/// This follows the exact design from the original LMAX Disruptor Sequencer interface.
pub trait Sequencer: Send + Sync + std::fmt::Debug {
    /// Get the current cursor position
    ///
    /// # Returns
    /// The current cursor sequence
    fn get_cursor(&self) -> Arc<Sequence>;

    /// Get the buffer size
    ///
    /// # Returns
    /// The size of the ring buffer
    fn get_buffer_size(&self) -> usize;

    /// Claim the next sequence number
    ///
    /// # Returns
    /// The next available sequence number
    ///
    /// # Errors
    /// Returns an error if the buffer is full
    fn next(&self) -> Result<i64>;

    /// Claim the next n sequence numbers
    ///
    /// # Arguments
    /// * `n` - The number of sequence numbers to claim
    ///
    /// # Returns
    /// The highest sequence number claimed
    ///
    /// # Errors
    /// Returns an error if there isn't enough capacity
    fn next_n(&self, n: i64) -> Result<i64>;

    /// Try to claim the next sequence number without blocking
    ///
    /// # Returns
    /// The next available sequence number, or None if not available
    fn try_next(&self) -> Option<i64>;

    /// Try to claim the next n sequence numbers without blocking
    ///
    /// # Arguments
    /// * `n` - The number of sequence numbers to claim
    ///
    /// # Returns
    /// The highest sequence number claimed, or None if not enough capacity
    fn try_next_n(&self, n: i64) -> Option<i64>;

    /// Publish a sequence number
    ///
    /// # Arguments
    /// * `sequence` - The sequence number to publish
    fn publish(&self, sequence: i64);

    /// Publish a range of sequence numbers
    ///
    /// # Arguments
    /// * `low` - The lowest sequence number to publish
    /// * `high` - The highest sequence number to publish
    fn publish_range(&self, low: i64, high: i64);

    /// Check if a sequence is available
    ///
    /// # Arguments
    /// * `sequence` - The sequence to check
    ///
    /// # Returns
    /// True if the sequence is available for consumption
    fn is_available(&self, sequence: i64) -> bool;

    /// Get the highest published sequence
    ///
    /// # Arguments
    /// * `next_sequence` - The next sequence to check from
    /// * `available_sequence` - The highest known available sequence
    ///
    /// # Returns
    /// The highest published sequence
    fn get_highest_published_sequence(&self, next_sequence: i64, available_sequence: i64) -> i64;

    /// Add gating sequences that this sequencer must not overtake
    ///
    /// # Arguments
    /// * `gating_sequences` - The sequences that gate this sequencer
    fn add_gating_sequences(&self, gating_sequences: &[Arc<Sequence>]);

    /// Remove gating sequences
    ///
    /// # Arguments
    /// * `sequence` - The sequence to remove
    ///
    /// # Returns
    /// True if the sequence was removed
    fn remove_gating_sequence(&self, sequence: Arc<Sequence>) -> bool;

    /// Create a new sequence barrier
    ///
    /// # Arguments
    /// * `sequences_to_track` - The sequences to track in the barrier
    ///
    /// # Returns
    /// A new sequence barrier
    fn new_barrier(&self, sequences_to_track: Vec<Arc<Sequence>>) -> Arc<dyn SequenceBarrier>;

    /// Get the minimum sequence from gating sequences
    ///
    /// # Returns
    /// The minimum sequence that gates this sequencer
    fn get_minimum_sequence(&self) -> i64;

    /// Get the remaining capacity
    ///
    /// # Returns
    /// The remaining capacity in the buffer
    fn remaining_capacity(&self) -> i64;
}

/// Single producer sequencer
///
/// This sequencer is optimized for scenarios where only one thread will be
/// publishing events. It provides the best performance when you can guarantee
/// single-threaded publishing.
#[derive(Debug)]
pub struct SingleProducerSequencer {
    buffer_size: usize,
    wait_strategy: Arc<dyn WaitStrategy>,
    cursor: Arc<Sequence>,
    gating_sequences: parking_lot::RwLock<Vec<Arc<Sequence>>>,
}

impl SingleProducerSequencer {
    /// Create a new single producer sequencer
    ///
    /// # Arguments
    /// * `buffer_size` - The size of the ring buffer
    /// * `wait_strategy` - The wait strategy to use
    ///
    /// # Returns
    /// A new SingleProducerSequencer instance
    pub fn new(buffer_size: usize, wait_strategy: Arc<dyn WaitStrategy>) -> Self {
        Self {
            buffer_size,
            wait_strategy,
            cursor: Arc::new(Sequence::new_with_initial_value()),
            gating_sequences: parking_lot::RwLock::new(Vec::new()),
        }
    }
}

impl Sequencer for SingleProducerSequencer {
    fn get_cursor(&self) -> Arc<Sequence> {
        Arc::clone(&self.cursor)
    }

    fn get_buffer_size(&self) -> usize {
        self.buffer_size
    }

    fn next(&self) -> Result<i64> {
        let next_sequence = self.cursor.get() + 1;
        let wrap_point = next_sequence - self.buffer_size as i64;
        let cached_gating_sequence = self.get_minimum_sequence();

        if wrap_point > cached_gating_sequence {
            return Err(DisruptorError::InsufficientCapacity);
        }

        Ok(next_sequence)
    }

    fn next_n(&self, n: i64) -> Result<i64> {
        let current = self.cursor.get();
        let next_sequence = current + n;
        let wrap_point = next_sequence - self.buffer_size as i64;
        let cached_gating_sequence = self.get_minimum_sequence();

        if wrap_point > cached_gating_sequence {
            return Err(DisruptorError::InsufficientCapacity);
        }

        Ok(next_sequence)
    }

    fn try_next(&self) -> Option<i64> {
        self.next().ok()
    }

    fn try_next_n(&self, n: i64) -> Option<i64> {
        self.next_n(n).ok()
    }

    fn publish(&self, sequence: i64) {
        self.cursor.set(sequence);
        self.wait_strategy.signal_all_when_blocking();
    }

    fn publish_range(&self, _low: i64, high: i64) {
        self.publish(high);
    }

    fn is_available(&self, sequence: i64) -> bool {
        sequence <= self.cursor.get()
    }

    fn get_highest_published_sequence(&self, _next_sequence: i64, available_sequence: i64) -> i64 {
        available_sequence
    }

    fn add_gating_sequences(&self, gating_sequences: &[Arc<Sequence>]) {
        let mut sequences = self.gating_sequences.write();
        sequences.extend_from_slice(gating_sequences);
    }

    fn remove_gating_sequence(&self, sequence: Arc<Sequence>) -> bool {
        let mut sequences = self.gating_sequences.write();
        if let Some(pos) = sequences.iter().position(|s| Arc::ptr_eq(s, &sequence)) {
            sequences.remove(pos);
            true
        } else {
            false
        }
    }

    fn new_barrier(&self, sequences_to_track: Vec<Arc<Sequence>>) -> Arc<dyn SequenceBarrier> {
        Arc::new(ProcessingSequenceBarrier::new(
            self.cursor.clone(),
            Arc::clone(&self.wait_strategy),
            sequences_to_track,
        ))
    }

    fn get_minimum_sequence(&self) -> i64 {
        let sequences = self.gating_sequences.read();
        Sequence::get_minimum_sequence(&sequences)
    }

    fn remaining_capacity(&self) -> i64 {
        let next_value = self.cursor.get() + 1;
        let consumed = self.get_minimum_sequence();
        self.buffer_size as i64 - (next_value - consumed)
    }
}

/// Multi producer sequencer
///
/// This sequencer supports multiple threads publishing events concurrently.
/// It uses more complex algorithms to handle coordination between producers.
#[derive(Debug)]
pub struct MultiProducerSequencer {
    buffer_size: usize,
    wait_strategy: Arc<dyn WaitStrategy>,
    cursor: Arc<Sequence>,
    gating_sequences: parking_lot::RwLock<Vec<Arc<Sequence>>>,
    // In a full implementation, this would have additional fields for
    // tracking individual producer sequences
}

impl MultiProducerSequencer {
    /// Create a new multi producer sequencer
    ///
    /// # Arguments
    /// * `buffer_size` - The size of the ring buffer
    /// * `wait_strategy` - The wait strategy to use
    ///
    /// # Returns
    /// A new MultiProducerSequencer instance
    pub fn new(buffer_size: usize, wait_strategy: Arc<dyn WaitStrategy>) -> Self {
        Self {
            buffer_size,
            wait_strategy,
            cursor: Arc::new(Sequence::new_with_initial_value()),
            gating_sequences: parking_lot::RwLock::new(Vec::new()),
        }
    }
}

impl Sequencer for MultiProducerSequencer {
    fn get_cursor(&self) -> Arc<Sequence> {
        Arc::clone(&self.cursor)
    }

    fn get_buffer_size(&self) -> usize {
        self.buffer_size
    }

    fn next(&self) -> Result<i64> {
        // Simplified implementation - a real multi-producer sequencer
        // would use more sophisticated coordination
        let next_sequence = self.cursor.increment_and_get();
        let wrap_point = next_sequence - self.buffer_size as i64;
        let cached_gating_sequence = self.get_minimum_sequence();

        if wrap_point > cached_gating_sequence {
            return Err(DisruptorError::InsufficientCapacity);
        }

        Ok(next_sequence)
    }

    fn next_n(&self, n: i64) -> Result<i64> {
        let next_sequence = self.cursor.add_and_get(n);
        let wrap_point = next_sequence - self.buffer_size as i64;
        let cached_gating_sequence = self.get_minimum_sequence();

        if wrap_point > cached_gating_sequence {
            return Err(DisruptorError::InsufficientCapacity);
        }

        Ok(next_sequence)
    }

    fn try_next(&self) -> Option<i64> {
        self.next().ok()
    }

    fn try_next_n(&self, n: i64) -> Option<i64> {
        self.next_n(n).ok()
    }

    fn publish(&self, sequence: i64) {
        // In a real multi-producer implementation, this would be more complex
        self.cursor.set(sequence);
        self.wait_strategy.signal_all_when_blocking();
    }

    fn publish_range(&self, _low: i64, high: i64) {
        self.publish(high);
    }

    fn is_available(&self, sequence: i64) -> bool {
        sequence <= self.cursor.get()
    }

    fn get_highest_published_sequence(&self, _next_sequence: i64, available_sequence: i64) -> i64 {
        available_sequence
    }

    fn add_gating_sequences(&self, gating_sequences: &[Arc<Sequence>]) {
        let mut sequences = self.gating_sequences.write();
        sequences.extend_from_slice(gating_sequences);
    }

    fn remove_gating_sequence(&self, sequence: Arc<Sequence>) -> bool {
        let mut sequences = self.gating_sequences.write();
        if let Some(pos) = sequences.iter().position(|s| Arc::ptr_eq(s, &sequence)) {
            sequences.remove(pos);
            true
        } else {
            false
        }
    }

    fn new_barrier(&self, sequences_to_track: Vec<Arc<Sequence>>) -> Arc<dyn SequenceBarrier> {
        Arc::new(ProcessingSequenceBarrier::new(
            self.cursor.clone(),
            Arc::clone(&self.wait_strategy),
            sequences_to_track,
        ))
    }

    fn get_minimum_sequence(&self) -> i64 {
        let sequences = self.gating_sequences.read();
        Sequence::get_minimum_sequence(&sequences)
    }

    fn remaining_capacity(&self) -> i64 {
        let next_value = self.cursor.get() + 1;
        let consumed = self.get_minimum_sequence();
        self.buffer_size as i64 - (next_value - consumed)
    }
}

// Forward declaration for SequenceBarrier - we'll implement this in sequence_barrier.rs
use crate::disruptor::SequenceBarrier;

// Temporary implementation for ProcessingSequenceBarrier
// This will be moved to sequence_barrier.rs
pub struct ProcessingSequenceBarrier {
    cursor: Arc<Sequence>,
    wait_strategy: Arc<dyn WaitStrategy>,
    dependent_sequences: Vec<Arc<Sequence>>,
}

impl ProcessingSequenceBarrier {
    pub fn new(
        cursor: Arc<Sequence>,
        wait_strategy: Arc<dyn WaitStrategy>,
        dependent_sequences: Vec<Arc<Sequence>>,
    ) -> Self {
        Self {
            cursor,
            wait_strategy,
            dependent_sequences,
        }
    }
}

impl SequenceBarrier for ProcessingSequenceBarrier {
    fn wait_for(&self, sequence: i64) -> Result<i64> {
        self.wait_strategy.wait_for(sequence, self.cursor.clone(), &self.dependent_sequences)
    }

    fn get_cursor(&self) -> Arc<Sequence> {
        self.cursor.clone()
    }

    fn is_alerted(&self) -> bool {
        false // Simplified implementation
    }

    fn alert(&self) {
        // Simplified implementation
    }

    fn clear_alert(&self) {
        // Simplified implementation
    }

    fn check_alert(&self) -> Result<()> {
        Ok(()) // Simplified implementation
    }
}
