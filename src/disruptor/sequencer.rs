//! Sequencer Implementation
//!
//! This module provides sequencer implementations for coordinating access to the ring buffer.
//! Sequencers manage the allocation of sequence numbers and ensure that producers don't
//! overwrite events that haven't been consumed yet.

use crate::disruptor::{is_power_of_two, DisruptorError, Result, Sequence, WaitStrategy};
use crossbeam_utils::CachePadded;
use std::sync::atomic::{AtomicI32, AtomicI64, AtomicU64, Ordering};
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

    /// Check if there's available capacity for the required number of sequences
    ///
    /// # Arguments
    /// * `required_capacity` - The number of sequences needed
    ///
    /// # Returns
    /// True if the buffer has capacity, false otherwise
    fn has_available_capacity(&self, required_capacity: usize) -> bool;
}

/// Single producer sequencer
///
/// This sequencer is optimized for scenarios where only one thread will be
/// publishing events. It provides the best performance when you can guarantee
/// single-threaded publishing.
///
/// Key optimizations from LMAX Disruptor:
/// - nextValue: tracks the next sequence to be claimed (not published yet)
/// - cachedValue: cached minimum gating sequence for performance
/// - No atomic operations needed since only one producer thread
#[derive(Debug)]
pub struct SingleProducerSequencer {
    buffer_size: usize,
    wait_strategy: Arc<dyn WaitStrategy>,
    cursor: Arc<Sequence>,
    gating_sequences: parking_lot::RwLock<Vec<Arc<Sequence>>>,
    /// Next sequence value to be claimed (equivalent to LMAX nextValue)
    /// Using AtomicI64 for Rust thread safety, but logically only one thread accesses it
    next_value: AtomicI64,
    /// Cached minimum gating sequence (equivalent to LMAX cachedValue)
    /// Using AtomicI64 for Rust thread safety, but logically only one thread accesses it
    cached_value: AtomicI64,
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
            next_value: AtomicI64::new(-1),
            cached_value: AtomicI64::new(-1),
        }
    }

    /// Check if there's available capacity for the required number of sequences
    /// This matches the LMAX Disruptor SingleProducerSequencer.hasAvailableCapacity method
    fn has_available_capacity_internal(&self, required_capacity: usize, do_store: bool) -> bool {
        let next_value = self.next_value.load(Ordering::Relaxed);
        let wrap_point = (next_value + required_capacity as i64) - self.buffer_size as i64;
        let cached_gating_sequence = self.cached_value.load(Ordering::Relaxed);

        if wrap_point > cached_gating_sequence || cached_gating_sequence > next_value {
            if do_store {
                // StoreLoad fence (equivalent to cursor.setVolatile in LMAX)
                self.cursor.set_volatile(next_value);
            }

            let min_sequence = self.get_minimum_sequence();
            self.cached_value.store(min_sequence, Ordering::Relaxed);

            if wrap_point > min_sequence {
                return false;
            }
        }

        true
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
        self.next_n(1)
    }

    fn next_n(&self, n: i64) -> Result<i64> {
        if n < 1 || n > self.buffer_size as i64 {
            return Err(DisruptorError::InvalidSequence(n));
        }

        // This follows the exact LMAX Disruptor SingleProducerSequencer.next(int n) logic
        let next_value = self.next_value.load(Ordering::Relaxed);
        let next_sequence = next_value + n;
        let wrap_point = next_sequence - self.buffer_size as i64;
        let cached_gating_sequence = self.cached_value.load(Ordering::Relaxed);

        if wrap_point > cached_gating_sequence || cached_gating_sequence > next_value {
            // Set cursor with volatile semantics (equivalent to cursor.setVolatile in LMAX)
            self.cursor.set_volatile(next_value);

            // Wait for consumers to catch up
            let mut min_sequence;
            while {
                min_sequence = self.get_minimum_sequence();
                wrap_point > min_sequence
            } {
                // Wait briefly (equivalent to LockSupport.parkNanos(1L))
                std::thread::sleep(std::time::Duration::from_nanos(1));
            }

            // Update cached value
            self.cached_value.store(min_sequence, Ordering::Relaxed);
        }

        // Update next_value (equivalent to this.nextValue = nextSequence in LMAX)
        self.next_value.store(next_sequence, Ordering::Relaxed);

        Ok(next_sequence)
    }

    fn try_next(&self) -> Option<i64> {
        self.try_next_n(1)
    }

    fn try_next_n(&self, n: i64) -> Option<i64> {
        if n < 1 {
            return None;
        }

        // This follows the exact LMAX Disruptor SingleProducerSequencer.tryNext(int n) logic
        if !self.has_available_capacity_internal(n as usize, true) {
            return None; // Insufficient capacity
        }

        // Update next_value and return the sequence (equivalent to this.nextValue += n)
        let next_sequence = self.next_value.load(Ordering::Relaxed) + n;
        self.next_value.store(next_sequence, Ordering::Relaxed);

        Some(next_sequence)
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
        // This follows the exact LMAX Disruptor SingleProducerSequencer.remainingCapacity logic
        let next_value = self.next_value.load(Ordering::Relaxed);
        let consumed = self.get_minimum_sequence();

        // If no consumers are registered, consumed will be i64::MAX
        // In this case, we have full capacity available
        if consumed == i64::MAX {
            return self.buffer_size as i64;
        }

        let used_capacity = next_value.saturating_sub(consumed);
        (self.buffer_size as i64).saturating_sub(used_capacity)
    }

    fn has_available_capacity(&self, required_capacity: usize) -> bool {
        self.has_available_capacity_internal(required_capacity, false)
    }
}

/// Multi producer sequencer with bitmap optimization
///
/// This sequencer supports multiple threads publishing events concurrently.
/// It uses optimized algorithms inspired by both LMAX Disruptor and disruptor-rs,
/// combining the best of both approaches for maximum performance.
///
/// Key features:
/// - Bitmap-based availability tracking (inspired by disruptor-rs)
/// - CAS-based coordination between producers (LMAX Disruptor)
/// - Optimized batch publishing support
/// - Cache-friendly memory layout
/// - ABA problem prevention
#[derive(Debug)]
pub struct MultiProducerSequencer {
    buffer_size: usize,
    wait_strategy: Arc<dyn WaitStrategy>,
    cursor: Arc<Sequence>,
    gating_sequences: parking_lot::RwLock<Vec<Arc<Sequence>>>,
    /// Bitmap tracking availability of slots using AtomicU64 arrays
    /// Each bit represents whether a slot was published in an even or odd round
    /// This is inspired by disruptor-rs's innovative bitmap approach
    available_bitmap: Box<[AtomicU64]>,
    /// Index mask for fast modulo operations (buffer_size - 1)
    index_mask: usize,

    /// Cached minimum gating sequence to reduce lock contention
    /// This is an optimization from the LMAX Disruptor to avoid frequent reads of gating sequences
    cached_gating_sequence: CachePadded<AtomicI64>,
    /// Legacy available buffer for backward compatibility
    /// This maintains compatibility with existing LMAX-style availability checking
    available_buffer: Vec<AtomicI32>,
}

impl MultiProducerSequencer {
    /// Create a new multi producer sequencer with optional bitmap optimization
    ///
    /// # Arguments
    /// * `buffer_size` - The size of the ring buffer (must be a power of 2)
    /// * `wait_strategy` - The wait strategy to use
    ///
    /// # Returns
    /// A new MultiProducerSequencer instance
    ///
    /// # Panics
    /// Panics if buffer_size is not a power of 2
    pub fn new(buffer_size: usize, wait_strategy: Arc<dyn WaitStrategy>) -> Self {
        assert!(
            is_power_of_two(buffer_size),
            "Buffer size must be a power of 2"
        );

        // Initialize legacy available buffer with -1 (unavailable) for all slots
        let available_buffer: Vec<AtomicI32> =
            (0..buffer_size).map(|_| AtomicI32::new(-1)).collect();

        // Initialize bitmap for availability tracking if buffer is large enough
        // For smaller buffers, we'll use an empty bitmap and fall back to legacy method
        let available_bitmap = if buffer_size >= 64 {
            let bitmap_size = buffer_size / 64; // Each AtomicU64 tracks 64 slots
            let bitmap: Box<[AtomicU64]> = (0..bitmap_size)
                .map(|_| AtomicU64::new(!0_u64)) // Initialize with all 1's (nothing published initially)
                .collect();
            bitmap
        } else {
            // For small buffers, use empty bitmap and fall back to legacy method
            let empty_bitmap: Box<[AtomicU64]> = Box::new([]);
            empty_bitmap
        };

        Self {
            buffer_size,
            wait_strategy,
            cursor: Arc::new(Sequence::new_with_initial_value()),
            gating_sequences: parking_lot::RwLock::new(Vec::new()),
            available_bitmap,
            index_mask: buffer_size - 1,
            cached_gating_sequence: CachePadded::new(AtomicI64::new(-1)),
            available_buffer,
        }
    }

    /// Calculate the index in the available buffer for a given sequence
    /// This matches the LMAX Disruptor calculateIndex method
    fn calculate_index(&self, sequence: i64) -> usize {
        (sequence as usize) & self.index_mask
    }

    /// Calculate the availability flag for a given sequence
    /// This matches the LMAX Disruptor calculateAvailabilityFlag method
    fn calculate_availability_flag(&self, sequence: i64) -> i32 {
        (sequence >> self.buffer_size.trailing_zeros()) as i32
    }

    /// Calculate availability indices for bitmap (inspired by disruptor-rs)
    fn calculate_availability_indices(&self, sequence: i64) -> (usize, usize) {
        let slot_index = (sequence as usize) & self.index_mask;
        let availability_index = slot_index >> 6; // Divide by 64
        let bit_index = slot_index - availability_index * 64;
        (availability_index, bit_index)
    }

    /// Get availability bitmap at index (inspired by disruptor-rs)
    fn availability_at(&self, index: usize) -> &AtomicU64 {
        // SAFETY: Index is always calculated with calculate_availability_indices and is within bounds
        unsafe { self.available_bitmap.get_unchecked(index) }
    }

    /// Set a sequence as available for consumption using both methods
    /// This maintains compatibility while adding bitmap optimization
    fn set_available(&self, sequence: i64) {
        // Legacy LMAX method (always used for compatibility)
        let index = self.calculate_index(sequence);
        let flag = self.calculate_availability_flag(sequence);
        self.available_buffer[index].store(flag, Ordering::Release);

        // New bitmap method (inspired by disruptor-rs) - only for large buffers
        if self.buffer_size >= 64 && !self.available_bitmap.is_empty() {
            self.publish_bitmap(sequence);
        }
    }

    /// Publish using bitmap method (inspired by disruptor-rs)
    fn publish_bitmap(&self, sequence: i64) {
        let (availability_index, bit_index) = self.calculate_availability_indices(sequence);
        if availability_index < self.available_bitmap.len() {
            let availability = self.availability_at(availability_index);
            let mask = 1 << bit_index;
            // XOR operation flips the bit to encode even/odd round publication
            availability.fetch_xor(mask, Ordering::Release);
        }
    }

    /// Check if a sequence is available for consumption
    /// This matches the LMAX Disruptor isAvailable method with proper flag checking
    fn is_available_internal(&self, sequence: i64) -> bool {
        let index = self.calculate_index(sequence);
        let flag = self.calculate_availability_flag(sequence);
        self.available_buffer[index].load(Ordering::Acquire) == flag
    }

    /// Check if there's available capacity for the required number of sequences
    /// This matches the LMAX Disruptor hasAvailableCapacity method
    fn has_available_capacity_internal(
        &self,
        gating_sequences: &[Arc<Sequence>],
        required_capacity: usize,
        cursor_value: i64,
    ) -> bool {
        let wrap_point = (cursor_value + required_capacity as i64) - self.buffer_size as i64;
        let cached_gating_sequence = self.cached_gating_sequence.load(Ordering::Acquire);

        if wrap_point > cached_gating_sequence || cached_gating_sequence > cursor_value {
            let min_sequence = Sequence::get_minimum_sequence(gating_sequences);
            self.cached_gating_sequence
                .store(min_sequence, Ordering::Release);

            if wrap_point > min_sequence {
                return false;
            }
        }

        true
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
        self.next_n(1)
    }

    fn next_n(&self, n: i64) -> Result<i64> {
        if n < 1 || n > self.buffer_size as i64 {
            return Err(DisruptorError::InvalidSequence(n));
        }

        // This is the key difference from the original implementation:
        // Use getAndAdd to atomically claim the sequences
        let current = self.cursor.get_and_add(n);
        let next_sequence = current + n;
        let wrap_point = next_sequence - self.buffer_size as i64;
        let cached_gating_sequence = self.cached_gating_sequence.load(Ordering::Acquire);

        // Check if we would wrap around and overtake consumers
        if wrap_point > cached_gating_sequence || cached_gating_sequence > current {
            // Get the actual minimum sequence from gating sequences
            let mut gating_sequence;
            while {
                gating_sequence = self.get_minimum_sequence();
                wrap_point > gating_sequence
            } {
                // Wait briefly before checking again (equivalent to LockSupport.parkNanos(1L))
                std::thread::sleep(std::time::Duration::from_nanos(1));
            }

            // Update the cached gating sequence
            self.cached_gating_sequence
                .store(gating_sequence, Ordering::Release);
        }

        Ok(next_sequence)
    }

    fn try_next(&self) -> Option<i64> {
        self.try_next_n(1)
    }

    fn try_next_n(&self, n: i64) -> Option<i64> {
        if n < 1 {
            return None;
        }

        // This follows the LMAX Disruptor tryNext implementation
        loop {
            let current = self.cursor.get();
            let next = current + n;

            // Check if we have available capacity
            if !self.has_available_capacity_internal(
                &self.gating_sequences.read(),
                n as usize,
                current,
            ) {
                return None; // Insufficient capacity
            }

            // Try to claim the sequences using CAS
            if self.cursor.compare_and_set(current, next) {
                return Some(next);
            }
            // If CAS failed, another producer claimed this sequence, try again
        }
    }

    fn publish(&self, sequence: i64) {
        // Use the proper LMAX Disruptor setAvailable method
        self.set_available(sequence);

        // Signal waiting consumers
        self.wait_strategy.signal_all_when_blocking();
    }

    fn publish_range(&self, low: i64, high: i64) {
        if low > high {
            return; // Invalid range
        }

        // Mark all sequences in the range as available
        // This follows the LMAX Disruptor MultiProducerSequencer.publish(long lo, long hi) logic
        for sequence in low..=high {
            self.set_available(sequence);
        }

        // Signal waiting consumers once after marking all sequences
        self.wait_strategy.signal_all_when_blocking();
    }

    fn is_available(&self, sequence: i64) -> bool {
        // Use the proper LMAX Disruptor isAvailable method with flag checking
        self.is_available_internal(sequence)
    }

    fn get_highest_published_sequence(&self, next_sequence: i64, available_sequence: i64) -> i64 {
        // This is the core algorithm from LMAX Disruptor for finding the highest
        // contiguous published sequence in a multi-producer environment

        // Start from the next sequence we're looking for
        let mut sequence = next_sequence;

        // Scan through the available buffer to find the highest contiguous sequence
        while sequence <= available_sequence {
            if !self.is_available(sequence) {
                // Found a gap, return the sequence before this gap
                return sequence - 1;
            }
            sequence += 1;
        }

        // All sequences up to available_sequence are published
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

        // If no consumers are registered, consumed will be i64::MAX
        // In this case, we have full capacity available
        if consumed == i64::MAX {
            return self.buffer_size as i64;
        }

        let used_capacity = next_value.saturating_sub(consumed);
        (self.buffer_size as i64).saturating_sub(used_capacity)
    }

    fn has_available_capacity(&self, required_capacity: usize) -> bool {
        let gating_sequences = self.gating_sequences.read();
        let cursor_value = self.cursor.get();
        self.has_available_capacity_internal(&gating_sequences, required_capacity, cursor_value)
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
        self.wait_strategy
            .wait_for(sequence, self.cursor.clone(), &self.dependent_sequences)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disruptor::BlockingWaitStrategy;

    #[test]
    fn test_multi_producer_sequencer_creation() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = MultiProducerSequencer::new(1024, wait_strategy);

        assert_eq!(sequencer.buffer_size, 1024);
        assert_eq!(sequencer.index_mask, 1023);
        assert_eq!(sequencer.available_buffer.len(), 1024);
    }

    #[test]
    fn test_multi_producer_sequencer_next() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = MultiProducerSequencer::new(8, wait_strategy);

        // Test basic sequence claiming
        let seq1 = sequencer.next().unwrap();
        assert_eq!(seq1, 0);

        let seq2 = sequencer.next().unwrap();
        assert_eq!(seq2, 1);
    }

    #[test]
    fn test_multi_producer_sequencer_publish_and_availability() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = MultiProducerSequencer::new(8, wait_strategy);

        // Claim and publish a sequence
        let sequence = sequencer.next().unwrap();
        assert!(!sequencer.is_available(sequence)); // Not available until published

        sequencer.publish(sequence);
        assert!(sequencer.is_available(sequence)); // Now available
    }

    #[test]
    fn test_multi_producer_highest_published_sequence() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = MultiProducerSequencer::new(8, wait_strategy);

        // Publish sequences 0, 1, 2
        for _i in 0..3 {
            let seq = sequencer.next().unwrap();
            sequencer.publish(seq);
        }

        // All sequences should be available
        let highest = sequencer.get_highest_published_sequence(0, 2);
        assert_eq!(highest, 2);
    }

    #[test]
    fn test_single_producer_sequencer_creation() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = SingleProducerSequencer::new(1024, wait_strategy);

        assert_eq!(sequencer.buffer_size, 1024);
    }

    #[test]
    fn test_single_producer_sequencer_next() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = SingleProducerSequencer::new(8, wait_strategy);

        // Test basic sequence claiming
        let seq1 = sequencer.next().unwrap();
        assert_eq!(seq1, 0);

        let seq2 = sequencer.next().unwrap();
        assert_eq!(seq2, 1);
    }
}


