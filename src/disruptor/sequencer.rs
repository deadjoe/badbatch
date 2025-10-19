//! Sequencer Implementation
//!
//! This module provides sequencer implementations for coordinating access to the ring buffer.
//! Sequencers manage the allocation of sequence numbers and ensure that producers don't
//! overwrite events that haven't been consumed yet.

use crate::disruptor::sequence_barrier::{ProcessingSequenceBarrier, SimpleSequenceBarrier};
use crate::disruptor::{
    is_power_of_two, DisruptorError, Result, Sequence, SequenceBarrier, WaitStrategy,
};
use crossbeam_utils::CachePadded;
use std::convert::TryFrom;
use std::sync::atomic::{AtomicI32, AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;

/// Wrapper to break circular dependency between Sequencer and SequenceBarrier
///
/// This wrapper provides a limited sequencer interface that can be used
/// by sequence barriers without creating circular dependencies.
#[derive(Debug)]
struct SequencerWrapper {
    buffer_size: usize,
    cursor: Arc<Sequence>,
    /// Unsafe back-reference to the real sequencer for delegation.
    /// This is required so the barrier can query publication continuity
    /// without changing the public Sequencer API surface.
    sequencer_ptr: *const (dyn Sequencer + 'static),
}

// SAFETY: The raw pointer is only used to delegate read-only queries
// to the underlying sequencer and is expected to outlive the wrapper
// (in production held by Arc, in tests by stack within the same scope).
// No mutation is performed through this pointer.
unsafe impl Send for SequencerWrapper {}
unsafe impl Sync for SequencerWrapper {}

impl SequencerWrapper {
    fn new_single_producer(
        buffer_size: usize,
        sequencer_ptr: *const (dyn Sequencer + 'static),
    ) -> Self {
        Self {
            buffer_size,
            cursor: Arc::new(Sequence::new(-1)),
            sequencer_ptr,
        }
    }

    fn new_multi_producer(
        buffer_size: usize,
        sequencer_ptr: *const (dyn Sequencer + 'static),
    ) -> Self {
        Self {
            buffer_size,
            cursor: Arc::new(Sequence::new(-1)),
            sequencer_ptr,
        }
    }
}

impl Sequencer for SequencerWrapper {
    fn get_cursor(&self) -> Arc<Sequence> {
        self.cursor.clone()
    }

    fn get_buffer_size(&self) -> usize {
        self.buffer_size
    }

    fn next(&self) -> Result<i64> {
        // This is a simplified implementation for barrier use
        // The actual sequencing is handled by the real sequencer
        Ok(self.cursor.get() + 1)
    }

    fn next_n(&self, n: i64) -> Result<i64> {
        Ok(self.cursor.get() + n)
    }

    fn try_next(&self) -> Option<i64> {
        Some(self.cursor.get() + 1)
    }

    fn try_next_n(&self, n: i64) -> Option<i64> {
        Some(self.cursor.get() + n)
    }

    fn publish(&self, _sequence: i64) {
        // No-op for wrapper
    }

    fn publish_range(&self, _low: i64, _high: i64) {
        // No-op for wrapper
    }

    fn has_available_capacity(&self, _required_capacity: usize) -> bool {
        true // Simplified for wrapper
    }

    fn is_available(&self, sequence: i64) -> bool {
        // Delegate to the real sequencer if possible
        let ptr = self.sequencer_ptr;
        if ptr.is_null() {
            return false;
        }
        // SAFETY: The underlying sequencer is owned elsewhere (Arc in production
        // paths or stack in tests) and outlives the barrier created here.
        unsafe { (&*ptr).is_available(sequence) }
    }

    fn get_highest_published_sequence(&self, next_sequence: i64, available_sequence: i64) -> i64 {
        let ptr = self.sequencer_ptr;
        if ptr.is_null() {
            return available_sequence;
        }
        // SAFETY: See note above.
        unsafe { (&*ptr).get_highest_published_sequence(next_sequence, available_sequence) }
    }

    fn new_barrier(&self, _sequences_to_track: Vec<Arc<Sequence>>) -> Arc<dyn SequenceBarrier> {
        // Return a simple barrier to avoid infinite recursion
        Arc::new(SimpleSequenceBarrier::new(
            self.cursor.clone(),
            // We don't have access to wait strategy here, so use a dummy
            Arc::new(crate::disruptor::BlockingWaitStrategy::new()),
        ))
    }

    fn add_gating_sequences(&self, _gating_sequences: &[Arc<Sequence>]) {
        // No-op for wrapper
    }

    fn remove_gating_sequence(&self, _sequence: Arc<Sequence>) -> bool {
        false // No-op for wrapper
    }

    fn get_minimum_sequence(&self) -> i64 {
        self.cursor.get()
    }

    fn remaining_capacity(&self) -> i64 {
        i64::try_from(self.buffer_size).expect("buffer size must fit into i64")
    }
}

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
    buffer_size_i64: i64,
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
        let buffer_size_i64 = i64::try_from(buffer_size).expect("buffer size must fit into i64");
        Self {
            buffer_size,
            buffer_size_i64,
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
        let required_capacity_i64 =
            i64::try_from(required_capacity).expect("required capacity must fit into i64");
        let wrap_point = (next_value + required_capacity_i64) - self.buffer_size_i64;
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
        if n < 1 || n > self.buffer_size_i64 {
            return Err(DisruptorError::InvalidSequence(n));
        }

        // This follows the exact LMAX Disruptor SingleProducerSequencer.next(int n) logic
        let next_value = self.next_value.load(Ordering::Relaxed);
        let next_sequence = next_value + n;
        let wrap_point = next_sequence - self.buffer_size_i64;
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
        let Ok(required) = usize::try_from(n) else {
            return None;
        };

        if !self.has_available_capacity_internal(required, true) {
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
        // Create a processing barrier with proper dependency tracking
        // Delegate continuity queries through a thin wrapper that
        // holds an unsafe back-reference to this sequencer.
        let self_ptr: *const (dyn Sequencer + 'static) = self as &dyn Sequencer;
        Arc::new(ProcessingSequenceBarrier::new(
            self.cursor.clone(),
            Arc::clone(&self.wait_strategy),
            sequences_to_track,
            Arc::new(SequencerWrapper::new_single_producer(
                self.buffer_size,
                self_ptr,
            )) as Arc<dyn Sequencer>,
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
            return self.buffer_size_i64;
        }

        let used_capacity = next_value.saturating_sub(consumed);
        self.buffer_size_i64.saturating_sub(used_capacity)
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
/// ## Cursor Semantics
///
/// **Important**: This implementation uses a different cursor semantic than the original LMAX Disruptor:
/// - **Our cursor**: Tracks the highest **claimed** sequence across all producers
/// - **LMAX cursor**: Tracks the highest **published contiguous** sequence
///
/// This design choice prioritizes performance by avoiding cursor updates during publish operations,
/// instead relying on sequence barriers to perform contiguity convergence via `get_highest_published_sequence()`.
/// This approach is safe but requires proper barrier implementation for correctness.
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
    buffer_size_i64: i64,
    wait_strategy: Arc<dyn WaitStrategy>,
    /// Cursor tracks the highest **claimed** sequence number by any producer.
    ///
    /// **Important**: Unlike LMAX Disruptor's cursor which represents the "highest published
    /// contiguous sequence", our cursor represents the "highest claimed sequence" across all
    /// producers. This design choice requires barrier-side contiguity convergence via
    /// `get_highest_published_sequence()` to ensure correctness in MPMC scenarios.
    ///
    /// The publish operation only marks availability bits without advancing the cursor,
    /// relying on consumers to perform contiguity checking through sequence barriers.
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
            let bitmap_size = buffer_size.div_ceil(64); // Each AtomicU64 tracks 64 slots, round up
            let bitmap: Box<[AtomicU64]> = (0..bitmap_size)
                .map(|_| AtomicU64::new(!0_u64)) // Initialize with all 1's (nothing published initially, matching disruptor-rs)
                .collect();
            bitmap
        } else {
            // For small buffers, use empty bitmap and fall back to legacy method
            let empty_bitmap: Box<[AtomicU64]> = Box::new([]);
            empty_bitmap
        };

        let buffer_size_i64 = i64::try_from(buffer_size).expect("buffer size must fit into i64");

        Self {
            buffer_size,
            buffer_size_i64,
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
        let normalized = sequence.rem_euclid(self.buffer_size_i64);
        let seq = usize::try_from(normalized).expect("normalized sequence must fit");
        seq & self.index_mask
    }

    /// Calculate the availability flag for a given sequence
    /// This matches the LMAX Disruptor calculateAvailabilityFlag method
    /// Returns 0 for even rounds, 1 for odd rounds
    fn calculate_availability_flag(&self, sequence: i64) -> i32 {
        let round = sequence.div_euclid(self.buffer_size_i64);
        let flag = round & 1;
        i32::try_from(flag).expect("flag must fit in i32")
    }

    /// Calculate the round flag for bitmap availability checking (inspired by disruptor-rs)
    /// Returns 0 for even rounds, 1 for odd rounds
    #[inline]
    fn calculate_round_flag(&self, sequence: i64) -> u64 {
        let round = sequence.div_euclid(self.buffer_size_i64);
        let flag = u8::try_from(round & 1).expect("round flag must be 0 or 1");
        u64::from(flag)
    }

    /// Calculate availability indices for bitmap (inspired by disruptor-rs)
    fn calculate_availability_indices(&self, sequence: i64) -> (usize, usize) {
        let normalized = sequence.rem_euclid(self.buffer_size_i64);
        let slot_index =
            usize::try_from(normalized).expect("normalized sequence must fit") & self.index_mask;
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
    /// Uses XOR operation to flip the bit, encoding even/odd round publication
    fn publish_bitmap(&self, sequence: i64) {
        let (availability_index, bit_index) = self.calculate_availability_indices(sequence);
        if availability_index < self.available_bitmap.len() {
            let availability = self.availability_at(availability_index);
            let mask = 1_u64 << bit_index;
            // XOR operation flips the bit to encode even/odd round publication
            availability.fetch_xor(mask, Ordering::Release);
        }
    }

    /// Check if a sequence is available using bitmap method (inspired by disruptor-rs)
    /// Compares the bit value with the expected round flag, handling bitmap wraparound
    fn is_bitmap_available(&self, sequence: i64) -> bool {
        let (availability_index, bit_index) = self.calculate_availability_indices(sequence);
        if availability_index < self.available_bitmap.len() {
            let availability = self.availability_at(availability_index);
            let current_value = availability.load(Ordering::Acquire);

            // Calculate expected flag for this sequence
            let expected_flag = self.calculate_round_flag(sequence);

            let actual_flag = (current_value >> bit_index) & 1;
            // Check if the bit matches the expected round flag
            actual_flag == expected_flag
        } else {
            false
        }
    }

    /// Get highest published sequence using bitmap method (inspired by disruptor-rs get_after)
    /// This correctly handles wraparound by tracking when availability_index wraps to 0
    fn get_highest_published_sequence_bitmap(
        &self,
        next_sequence: i64,
        available_sequence: i64,
    ) -> i64 {
        if next_sequence > available_sequence {
            return available_sequence;
        }

        // Use disruptor-rs approach: start from previous sequence and find highest available
        let prev_sequence = next_sequence - 1;
        let mut availability_flag = self.calculate_round_flag(prev_sequence);
        let (mut availability_index, mut bit_index) =
            self.calculate_availability_indices(prev_sequence);

        if availability_index >= self.available_bitmap.len() {
            return prev_sequence; // No bitmap available, fall back
        }

        let mut availability = self.available_bitmap[availability_index].load(Ordering::Acquire);
        // Shift to the first relevant bit
        availability >>= bit_index;
        let mut highest_available = prev_sequence;

        loop {
            // Check if current sequence is available
            if (availability & 1) != availability_flag {
                // Found unavailable sequence, return the previous one
                return highest_available - 1;
            }

            highest_available += 1;

            // If we've checked all sequences up to available_sequence, we're done
            if highest_available > available_sequence {
                return available_sequence;
            }

            // Prepare for checking the next bit
            if bit_index < 63 {
                bit_index += 1;
                availability >>= 1;
            } else {
                // Move to next AtomicU64 (bitmap field)
                let (next_availability_index, _) =
                    self.calculate_availability_indices(highest_available);

                if next_availability_index >= self.available_bitmap.len() {
                    // No more bitmap data available
                    return highest_available - 1;
                }

                availability_index = next_availability_index;
                availability = self.available_bitmap[availability_index].load(Ordering::Acquire);
                bit_index = 0;

                // Critical wraparound detection: when availability_index becomes 0 again
                if availability_index == 0 {
                    // We've wrapped around the bitmap, flip the expected flag
                    availability_flag ^= 1;
                }
            }
        }
    }

    /// Get highest published sequence using legacy LMAX Disruptor algorithm
    fn get_highest_published_sequence_legacy(
        &self,
        next_sequence: i64,
        available_sequence: i64,
    ) -> i64 {
        // This is the core algorithm from LMAX Disruptor for finding the highest
        // contiguous published sequence in a multi-producer environment

        // Start from the next sequence we're looking for
        let mut sequence = next_sequence;

        // Scan through the available buffer to find the highest contiguous sequence
        while sequence <= available_sequence {
            if !self.is_available_internal(sequence) {
                // Found a gap, return the sequence before this gap
                return sequence - 1;
            }
            sequence += 1;
        }

        // All sequences up to available_sequence are published
        available_sequence
    }

    /// Check if a sequence is available for consumption
    /// This matches the LMAX Disruptor isAvailable method with proper flag checking
    fn is_available_internal(&self, sequence: i64) -> bool {
        // For large buffers, use bitmap method as the primary check
        if self.buffer_size >= 64 && !self.available_bitmap.is_empty() {
            self.is_bitmap_available(sequence)
        } else {
            // For small buffers, use legacy method
            let index = self.calculate_index(sequence);
            let flag = self.calculate_availability_flag(sequence);
            self.available_buffer[index].load(Ordering::Acquire) == flag
        }
    }

    /// Check if there's available capacity for the required number of sequences
    /// This matches the LMAX Disruptor hasAvailableCapacity method
    fn has_available_capacity_internal(
        &self,
        gating_sequences: &[Arc<Sequence>],
        required_capacity: usize,
        cursor_value: i64,
    ) -> bool {
        let required_capacity_i64 =
            i64::try_from(required_capacity).expect("required capacity must fit into i64");
        let wrap_point = (cursor_value + required_capacity_i64) - self.buffer_size_i64;
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
        if n < 1 || n > self.buffer_size_i64 {
            return Err(DisruptorError::InvalidSequence(n));
        }

        // Following LMAX Disruptor design: Use CAS loop instead of getAndAdd
        // This ensures sequence allocation is contiguous
        let mut current;
        let mut next;

        // Try claiming the sequence using CAS
        loop {
            current = self.cursor.get();
            next = current + n;

            // Check if we have available capacity
            let wrap_point = next - self.buffer_size_i64;
            let cached_gating_sequence = self.cached_gating_sequence.load(Ordering::Acquire);

            if wrap_point > cached_gating_sequence || cached_gating_sequence > current {
                // Get the actual minimum sequence from gating sequences
                let min_sequence = self.get_minimum_sequence();

                // If we don't have enough capacity, wait until we do
                if wrap_point > min_sequence {
                    // Wait briefly before checking again (equivalent to LockSupport.parkNanos(1L))
                    std::thread::sleep(std::time::Duration::from_nanos(1));
                    continue;
                }

                // Update the cached gating sequence
                self.cached_gating_sequence
                    .store(min_sequence, Ordering::Release);
            }

            // Try to claim the sequences using CAS
            if self.cursor.compare_and_set(current, next) {
                break;
            }
            // If CAS failed, another producer claimed this sequence, try again with new current
        }

        Ok(next)
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
            let Ok(required) = usize::try_from(n) else {
                return None;
            };

            if !self.has_available_capacity_internal(
                &self.gating_sequences.read(),
                required,
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
        // For large buffers with bitmap, use disruptor-rs inspired algorithm
        if self.buffer_size >= 64 && !self.available_bitmap.is_empty() {
            self.get_highest_published_sequence_bitmap(next_sequence, available_sequence)
        } else {
            // For small buffers, use legacy LMAX Disruptor algorithm
            self.get_highest_published_sequence_legacy(next_sequence, available_sequence)
        }
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
        // Create a processing barrier with proper dependency tracking.
        // The wrapper delegates continuity queries to the real sequencer.
        let self_ptr: *const (dyn Sequencer + 'static) = self as &dyn Sequencer;
        Arc::new(ProcessingSequenceBarrier::new(
            self.cursor.clone(),
            Arc::clone(&self.wait_strategy),
            sequences_to_track,
            Arc::new(SequencerWrapper::new_multi_producer(
                self.buffer_size,
                self_ptr,
            )) as Arc<dyn Sequencer>,
        ))
    }

    fn get_minimum_sequence(&self) -> i64 {
        let sequences = self.gating_sequences.read();
        Sequence::get_minimum_sequence(&sequences)
    }

    fn remaining_capacity(&self) -> i64 {
        let next_value = self.cursor.get();
        let consumed = self.get_minimum_sequence();

        // If no consumers are registered, consumed will be i64::MAX
        // In this case, we have full capacity available
        if consumed == i64::MAX {
            return self.buffer_size_i64;
        }

        let used_capacity = next_value.saturating_sub(consumed);
        self.buffer_size_i64.saturating_sub(used_capacity)
    }

    fn has_available_capacity(&self, required_capacity: usize) -> bool {
        let gating_sequences = self.gating_sequences.read();
        let cursor_value = self.cursor.get();
        self.has_available_capacity_internal(&gating_sequences, required_capacity, cursor_value)
    }
}

// SequenceBarrier is already imported above

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

    #[test]
    fn test_single_producer_sequencer_next_n() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = SingleProducerSequencer::new(8, wait_strategy);

        // Test claiming multiple sequences
        let seq = sequencer.next_n(3).unwrap();
        assert_eq!(seq, 2); // Returns highest sequence (0, 1, 2)

        // Test invalid sequence count
        let result = sequencer.next_n(0);
        assert!(result.is_err());

        let result = sequencer.next_n(-1);
        assert!(result.is_err());

        // Test claiming more than buffer size
        let result = sequencer.next_n(10);
        assert!(result.is_err());
    }

    #[test]
    fn test_single_producer_sequencer_try_next() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = SingleProducerSequencer::new(8, wait_strategy);

        // Test try_next success
        let seq = sequencer.try_next().unwrap();
        assert_eq!(seq, 0);

        // Test try_next_n success
        let seq = sequencer.try_next_n(2).unwrap();
        assert_eq!(seq, 2);

        // Test try_next_n with invalid count
        let result = sequencer.try_next_n(0);
        assert!(result.is_none());

        let result = sequencer.try_next_n(-1);
        assert!(result.is_none());
    }

    #[test]
    fn test_single_producer_sequencer_publish() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = SingleProducerSequencer::new(8, wait_strategy);

        let seq = sequencer.next().unwrap();
        assert!(!sequencer.is_available(seq));

        sequencer.publish(seq);
        assert!(sequencer.is_available(seq));

        // Test publish_range
        let seq2 = sequencer.next_n(3).unwrap();
        sequencer.publish_range(seq + 1, seq2);
        assert!(sequencer.is_available(seq2));
    }

    #[test]
    fn test_single_producer_sequencer_gating_sequences() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = SingleProducerSequencer::new(8, wait_strategy);

        let gating_seq1 = Arc::new(Sequence::new(5));
        let gating_seq2 = Arc::new(Sequence::new(3));

        // Add gating sequences
        sequencer.add_gating_sequences(&[gating_seq1.clone(), gating_seq2.clone()]);

        // Test minimum sequence calculation
        let min_seq = sequencer.get_minimum_sequence();
        assert_eq!(min_seq, 3); // Should be minimum of gating sequences

        // Test remove gating sequence
        let removed = sequencer.remove_gating_sequence(gating_seq2.clone());
        assert!(removed);

        let min_seq = sequencer.get_minimum_sequence();
        assert_eq!(min_seq, 5); // Should be remaining gating sequence

        // Test removing non-existent sequence
        let removed = sequencer.remove_gating_sequence(Arc::new(Sequence::new(100)));
        assert!(!removed);
    }

    #[test]
    fn test_single_producer_sequencer_capacity() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = SingleProducerSequencer::new(8, wait_strategy);

        // Test remaining capacity with no consumers
        let capacity = sequencer.remaining_capacity();
        assert_eq!(capacity, 8);

        // Test has_available_capacity
        assert!(sequencer.has_available_capacity(4));
        assert!(sequencer.has_available_capacity(8));

        // Add a consumer sequence
        let consumer_seq = Arc::new(Sequence::new(-1));
        sequencer.add_gating_sequences(std::slice::from_ref(&consumer_seq));

        // Claim some sequences
        let _seq1 = sequencer.next_n(4).unwrap();

        // Test remaining capacity with consumer
        let capacity = sequencer.remaining_capacity();
        assert!(capacity <= 8);

        // Update consumer sequence and test again
        consumer_seq.set(2);
        let capacity = sequencer.remaining_capacity();
        assert!(capacity > 0);
    }

    #[test]
    fn test_single_producer_sequencer_barrier() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = SingleProducerSequencer::new(8, wait_strategy);

        let dep_seq = Arc::new(Sequence::new(0));
        let barrier = sequencer.new_barrier(vec![dep_seq]);

        assert_eq!(barrier.get_cursor().get(), -1);
        assert!(!barrier.is_alerted());
    }

    #[test]
    fn test_multi_producer_sequencer_next_n() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = MultiProducerSequencer::new(8, wait_strategy);

        // Test claiming multiple sequences
        let seq = sequencer.next_n(3).unwrap();
        assert_eq!(seq, 2); // Returns highest sequence (0, 1, 2)

        // Test invalid sequence count
        let result = sequencer.next_n(0);
        assert!(result.is_err());

        let result = sequencer.next_n(-1);
        assert!(result.is_err());

        // Test claiming more than buffer size
        let result = sequencer.next_n(10);
        assert!(result.is_err());
    }

    #[test]
    fn test_multi_producer_sequencer_try_next() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = MultiProducerSequencer::new(8, wait_strategy);

        // Test try_next success
        let seq = sequencer.try_next().unwrap();
        assert_eq!(seq, 0);

        // Test try_next_n success
        let seq = sequencer.try_next_n(2).unwrap();
        assert_eq!(seq, 2);

        // Test try_next_n with invalid count
        let result = sequencer.try_next_n(0);
        assert!(result.is_none());

        let result = sequencer.try_next_n(-1);
        assert!(result.is_none());
    }

    #[test]
    fn test_multi_producer_sequencer_publish_range() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = MultiProducerSequencer::new(8, wait_strategy);

        // Claim sequences
        let seq1 = sequencer.next().unwrap();
        let seq2 = sequencer.next().unwrap();
        let seq3 = sequencer.next().unwrap();

        // Publish range
        sequencer.publish_range(seq1, seq3);

        // All sequences in range should be available
        assert!(sequencer.is_available(seq1));
        assert!(sequencer.is_available(seq2));
        assert!(sequencer.is_available(seq3));

        // Test invalid range
        sequencer.publish_range(10, 5); // high < low should be handled gracefully
    }

    #[test]
    fn test_multi_producer_sequencer_gating_sequences() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = MultiProducerSequencer::new(8, wait_strategy);

        let gating_seq1 = Arc::new(Sequence::new(5));
        let gating_seq2 = Arc::new(Sequence::new(3));

        // Add gating sequences
        sequencer.add_gating_sequences(&[gating_seq1.clone(), gating_seq2.clone()]);

        // Test minimum sequence calculation
        let min_seq = sequencer.get_minimum_sequence();
        assert_eq!(min_seq, 3); // Should be minimum of gating sequences

        // Test remove gating sequence
        let removed = sequencer.remove_gating_sequence(gating_seq2.clone());
        assert!(removed);

        let min_seq = sequencer.get_minimum_sequence();
        assert_eq!(min_seq, 5); // Should be remaining gating sequence

        // Test removing non-existent sequence
        let removed = sequencer.remove_gating_sequence(Arc::new(Sequence::new(100)));
        assert!(!removed);
    }

    #[test]
    fn test_multi_producer_sequencer_capacity() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = MultiProducerSequencer::new(8, wait_strategy);

        // Test remaining capacity with no consumers
        assert_eq!(sequencer.remaining_capacity(), 8);

        // Test has_available_capacity
        assert!(sequencer.has_available_capacity(4));
        assert!(sequencer.has_available_capacity(8));

        // Add a consumer sequence
        let consumer_seq = Arc::new(Sequence::new(-1));
        sequencer.add_gating_sequences(std::slice::from_ref(&consumer_seq));

        // Claim some sequences
        let _seq1 = sequencer.next_n(4).unwrap();

        // With 4 sequences claimed and none consumed, remaining capacity should drop by 4
        assert_eq!(sequencer.remaining_capacity(), 4);

        // Update consumer sequence and test again
        consumer_seq.set(2);
        assert_eq!(sequencer.remaining_capacity(), 7);
    }

    #[test]
    fn test_multi_producer_sequencer_barrier() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = MultiProducerSequencer::new(8, wait_strategy);

        let dep_seq = Arc::new(Sequence::new(0));
        let barrier = sequencer.new_barrier(vec![dep_seq]);

        assert_eq!(barrier.get_cursor().get(), -1);
        assert!(!barrier.is_alerted());
    }

    #[test]
    fn test_multi_producer_sequencer_highest_published_with_gaps() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = MultiProducerSequencer::new(8, wait_strategy);

        // Claim sequences 0, 1, 2, 3
        let seq0 = sequencer.next().unwrap();
        let seq1 = sequencer.next().unwrap();
        let seq2 = sequencer.next().unwrap();
        let seq3 = sequencer.next().unwrap();

        // Publish 0, 2, 3 but leave gap at 1
        sequencer.publish(seq0);
        sequencer.publish(seq2);
        sequencer.publish(seq3);

        // Should only return 0 due to gap at 1
        let highest = sequencer.get_highest_published_sequence(0, 3);
        assert_eq!(highest, 0);

        // Now publish sequence 1
        sequencer.publish(seq1);

        // Should now return 3 as all sequences are published
        let highest = sequencer.get_highest_published_sequence(0, 3);
        assert_eq!(highest, 3);
    }

    #[test]
    fn test_multi_producer_bitmap_functionality() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = MultiProducerSequencer::new(64, wait_strategy); // Large enough for bitmap

        // Test bitmap functionality
        let seq = sequencer.next().unwrap();
        assert!(!sequencer.is_available(seq));

        sequencer.publish(seq);
        assert!(sequencer.is_available(seq));

        // Test that bitmap is used for large buffers
        assert!(!sequencer.available_bitmap.is_empty());
    }

    #[test]
    fn test_multi_producer_small_buffer_fallback() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = MultiProducerSequencer::new(8, wait_strategy); // Small buffer

        // Test that bitmap is empty for small buffers (falls back to legacy method)
        assert!(sequencer.available_bitmap.is_empty());

        // Test that legacy method still works
        let seq = sequencer.next().unwrap();
        assert!(!sequencer.is_available(seq));

        sequencer.publish(seq);
        assert!(sequencer.is_available(seq));
    }

    #[test]
    fn test_sequencer_wrapper_functionality() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());

        // Backing sequencer for delegation
        let sp = SingleProducerSequencer::new(8, wait_strategy.clone());
        let sp_ptr: *const (dyn Sequencer + 'static) = &sp as &dyn Sequencer;

        // Test single producer wrapper
        let wrapper = SequencerWrapper::new_single_producer(8, sp_ptr);
        assert_eq!(wrapper.get_buffer_size(), 8);
        assert_eq!(wrapper.get_cursor().get(), -1);
        assert_eq!(wrapper.next().unwrap(), 0);
        assert_eq!(wrapper.next_n(3).unwrap(), 2);
        assert_eq!(wrapper.try_next().unwrap(), 0);
        assert_eq!(wrapper.try_next_n(2).unwrap(), 1);
        assert!(wrapper.has_available_capacity(4));
        // Delegated is_available should reflect real sequencer state (initially unpublished)
        assert!(!wrapper.is_available(0));
        assert_eq!(wrapper.get_highest_published_sequence(0, 5), 5);
        assert_eq!(wrapper.get_minimum_sequence(), -1);
        assert_eq!(wrapper.remaining_capacity(), 8);

        // Test multi producer wrapper
        let sp2 = SingleProducerSequencer::new(16, Arc::new(BlockingWaitStrategy::new()));
        let ptr2: *const (dyn Sequencer + 'static) = &sp2 as &dyn Sequencer;
        let wrapper = SequencerWrapper::new_multi_producer(16, ptr2);
        assert_eq!(wrapper.get_buffer_size(), 16);
        assert_eq!(wrapper.get_cursor().get(), -1);

        // Test publish methods (no-ops)
        wrapper.publish(0);
        wrapper.publish_range(0, 5);
        wrapper.add_gating_sequences(&[]);
        assert!(!wrapper.remove_gating_sequence(Arc::new(Sequence::new(0))));

        // Test barrier creation
        let barrier = wrapper.new_barrier(vec![]);
        assert!(!barrier.is_alerted());
    }

    #[test]
    fn test_bitmap_basic_functionality() {
        let wait_strategy = Arc::new(crate::disruptor::BlockingWaitStrategy::new());
        let sequencer = MultiProducerSequencer::new(64, wait_strategy);

        // Initially, sequence 0 should not be available
        assert!(
            !sequencer.is_bitmap_available(0),
            "Sequence 0 should not be available initially"
        );

        // Publish sequence 0
        sequencer.publish_bitmap(0);

        // Now sequence 0 should be available
        assert!(
            sequencer.is_bitmap_available(0),
            "Sequence 0 should be available after publishing"
        );

        // Sequence 1 should still not be available
        assert!(
            !sequencer.is_bitmap_available(1),
            "Sequence 1 should not be available"
        );

        // Publish sequence 1
        sequencer.publish_bitmap(1);

        // Now sequence 1 should be available
        assert!(
            sequencer.is_bitmap_available(1),
            "Sequence 1 should be available after publishing"
        );

        // Test round wraparound - sequence 64 should use different round flag
        sequencer.publish_bitmap(64);
        assert!(
            sequencer.is_bitmap_available(64),
            "Sequence 64 should be available after publishing"
        );
    }

    #[test]
    fn test_bitmap_flags_calculation() {
        let wait_strategy = Arc::new(crate::disruptor::BlockingWaitStrategy::new());
        let sequencer = MultiProducerSequencer::new(64, wait_strategy);

        // Test round flag calculation
        assert_eq!(
            sequencer.calculate_round_flag(0),
            0,
            "Sequence 0 should be round 0 (even)"
        );
        assert_eq!(
            sequencer.calculate_round_flag(63),
            0,
            "Sequence 63 should be round 0 (even)"
        );
        assert_eq!(
            sequencer.calculate_round_flag(64),
            1,
            "Sequence 64 should be round 1 (odd)"
        );
        assert_eq!(
            sequencer.calculate_round_flag(127),
            1,
            "Sequence 127 should be round 1 (odd)"
        );
        assert_eq!(
            sequencer.calculate_round_flag(128),
            0,
            "Sequence 128 should be round 0 (even)"
        );

        // Test availability indices calculation
        let (avail_idx, bit_idx) = sequencer.calculate_availability_indices(0);
        assert_eq!(avail_idx, 0);
        assert_eq!(bit_idx, 0);

        let (avail_idx, bit_idx) = sequencer.calculate_availability_indices(63);
        assert_eq!(avail_idx, 0);
        assert_eq!(bit_idx, 63);

        let (avail_idx, bit_idx) = sequencer.calculate_availability_indices(64);
        assert_eq!(
            avail_idx, 0,
            "Sequence 64 should map to availability_index 0 (same as sequence 0)"
        );
        assert_eq!(
            bit_idx, 0,
            "Sequence 64 should map to bit_index 0 (same as sequence 0)"
        );
    }

    #[test]
    fn test_bitmap_get_highest_published() {
        let wait_strategy = Arc::new(crate::disruptor::BlockingWaitStrategy::new());
        let sequencer = MultiProducerSequencer::new(64, wait_strategy);

        // No sequences published yet
        let highest = sequencer.get_highest_published_sequence_bitmap(0, 5);
        assert_eq!(
            highest, -1,
            "Should return -1 when no sequences are published"
        );

        // Publish sequence 0
        sequencer.publish_bitmap(0);
        let highest = sequencer.get_highest_published_sequence_bitmap(0, 5);
        assert_eq!(
            highest, 0,
            "Should return 0 when only sequence 0 is published"
        );

        // Publish sequences 0, 1, 2
        sequencer.publish_bitmap(1);
        sequencer.publish_bitmap(2);
        let highest = sequencer.get_highest_published_sequence_bitmap(0, 5);
        assert_eq!(
            highest, 2,
            "Should return 2 when sequences 0,1,2 are published"
        );

        // Gap test: publish 0,1,2,4 (skip 3)
        sequencer.publish_bitmap(4);
        let highest = sequencer.get_highest_published_sequence_bitmap(0, 5);
        assert_eq!(
            highest, 2,
            "Should return 2 when there's a gap at sequence 3"
        );

        // Fill the gap
        sequencer.publish_bitmap(3);
        let highest = sequencer.get_highest_published_sequence_bitmap(0, 5);
        assert_eq!(
            highest, 4,
            "Should return 4 when all sequences 0-4 are published"
        );
    }

    #[test]
    fn test_multi_producer_availability_calculations() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        let sequencer = MultiProducerSequencer::new(64, wait_strategy);

        // Test index calculation
        let index = sequencer.calculate_index(65);
        assert_eq!(index, 1); // 65 & 63 = 1

        // Test availability flag calculation
        let flag = sequencer.calculate_availability_flag(64);
        assert_eq!(flag, 1); // 64 >> 6 = 1

        // Test availability indices calculation
        let (availability_index, bit_index) = sequencer.calculate_availability_indices(70);
        assert_eq!(availability_index, 0); // 6 >> 6 = 0
        assert_eq!(bit_index, 6); // 6 - 0 * 64 = 6
    }

    #[test]
    #[should_panic(expected = "Buffer size must be a power of 2")]
    fn test_multi_producer_invalid_buffer_size() {
        let wait_strategy = Arc::new(BlockingWaitStrategy::new());
        MultiProducerSequencer::new(7, wait_strategy); // Not a power of 2
    }
}
