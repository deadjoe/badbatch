//! Sequencer implementations for the Disruptor
//!
//! The Sequencer is responsible for coordinating access between producers and consumers.
//! It provides different implementations optimized for single-producer and multi-producer
//! scenarios.

use crate::disruptor::{
    Sequence, SequenceBarrier, WaitStrategy, Result, DisruptorError, INITIAL_CURSOR_VALUE
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Trait for sequencer implementations
pub trait Sequencer: Send + Sync {
    /// Get the buffer size
    fn buffer_size(&self) -> usize;

    /// Claim the next sequence for publishing
    fn next(&self) -> Result<i64>;

    /// Claim the next n sequences for publishing
    fn next_n(&self, n: usize) -> Result<i64>;

    /// Try to claim the next sequence without blocking
    fn try_next(&self) -> Result<Option<i64>>;

    /// Try to claim the next n sequences without blocking
    fn try_next_n(&self, n: usize) -> Result<Option<i64>>;

    /// Publish the sequence, making it available to consumers
    fn publish(&self, sequence: i64);

    /// Publish a range of sequences
    fn publish_range(&self, lo: i64, hi: i64);

    /// Check if a sequence is available for consumption
    fn is_available(&self, sequence: i64) -> bool;

    /// Get the highest published sequence
    fn get_highest_published_sequence(&self, next_sequence: i64, available_sequence: i64) -> i64;

    /// Create a sequence barrier for consumers
    fn new_barrier(&self, sequences_to_track: Vec<Arc<Sequence>>) -> SequenceBarrier;

    /// Get the minimum sequence from gating sequences
    fn get_minimum_sequence(&self) -> i64;

    /// Add gating sequences (consumers that gate the producer)
    fn add_gating_sequences(&self, gating_sequences: &[Arc<Sequence>]);

    /// Remove a gating sequence
    fn remove_gating_sequence(&self, sequence: &Arc<Sequence>) -> bool;

    /// Get the current cursor value
    fn get_cursor(&self) -> i64;

    /// Get the remaining capacity
    fn remaining_capacity(&self) -> i64;
}

/// Single producer sequencer optimized for single-threaded publishing
pub struct SingleProducerSequencer {
    /// Size of the buffer (must be power of 2)
    buffer_size: usize,
    /// Wait strategy for consumers
    wait_strategy: Arc<dyn WaitStrategy>,
    /// Current cursor position
    cursor: Arc<Sequence>,
    /// Next value to be published
    next_value: Sequence,
    /// Cached value of gating sequences
    cached_value: Sequence,
    /// Gating sequences (consumers)
    gating_sequences: parking_lot::RwLock<Vec<Arc<Sequence>>>,
}

impl SingleProducerSequencer {
    /// Create a new single producer sequencer
    pub fn new(buffer_size: usize, wait_strategy: Arc<dyn WaitStrategy>) -> Self {
        Self {
            buffer_size,
            wait_strategy,
            cursor: Arc::new(Sequence::new(INITIAL_CURSOR_VALUE)),
            next_value: Sequence::new(INITIAL_CURSOR_VALUE),
            cached_value: Sequence::new(INITIAL_CURSOR_VALUE),
            gating_sequences: parking_lot::RwLock::new(Vec::new()),
        }
    }

    /// Check if there is available capacity for the required sequence
    fn has_available_capacity(&self, required_capacity: i64) -> bool {
        let next_value = self.next_value.get();
        let wrap_point = (next_value + required_capacity) - self.buffer_size as i64;
        let cached_gating_sequence = self.cached_value.get();

        if wrap_point > cached_gating_sequence || cached_gating_sequence > next_value {
            let min_sequence = self.get_minimum_sequence();
            self.cached_value.set(min_sequence);

            if wrap_point > min_sequence {
                return false;
            }
        }

        true
    }
}

impl Sequencer for SingleProducerSequencer {
    fn buffer_size(&self) -> usize {
        self.buffer_size
    }

    fn next(&self) -> Result<i64> {
        self.next_n(1)
    }

    fn next_n(&self, n: usize) -> Result<i64> {
        if n < 1 {
            return Err(DisruptorError::InvalidSequence(n as i64));
        }

        let next_value = self.next_value.get();
        let next_sequence = next_value + n as i64;

        if !self.has_available_capacity(n as i64) {
            return Err(DisruptorError::BufferFull);
        }

        self.next_value.set(next_sequence);
        Ok(next_sequence)
    }

    fn try_next(&self) -> Result<Option<i64>> {
        match self.try_next_n(1)? {
            Some(seq) => Ok(Some(seq)),
            None => Ok(None),
        }
    }

    fn try_next_n(&self, n: usize) -> Result<Option<i64>> {
        if n < 1 {
            return Err(DisruptorError::InvalidSequence(n as i64));
        }

        if !self.has_available_capacity(n as i64) {
            return Ok(None);
        }

        let next_value = self.next_value.get();
        let next_sequence = next_value + n as i64;
        self.next_value.set(next_sequence);
        Ok(Some(next_sequence))
    }

    fn publish(&self, sequence: i64) {
        self.cursor.set(sequence);
        self.wait_strategy.signal_all_when_blocking();
    }

    fn publish_range(&self, _lo: i64, hi: i64) {
        self.publish(hi);
    }

    fn is_available(&self, sequence: i64) -> bool {
        sequence <= self.cursor.get()
    }

    fn get_highest_published_sequence(&self, _next_sequence: i64, available_sequence: i64) -> i64 {
        available_sequence
    }

    fn new_barrier(&self, sequences_to_track: Vec<Arc<Sequence>>) -> SequenceBarrier {
        SequenceBarrier::new(
            Arc::clone(&self.wait_strategy),
            Arc::clone(&self.cursor),
            sequences_to_track,
        )
    }

    fn get_minimum_sequence(&self) -> i64 {
        let gating_sequences = self.gating_sequences.read();
        if gating_sequences.is_empty() {
            self.cursor.get()
        } else {
            gating_sequences
                .iter()
                .map(|seq| seq.get())
                .min()
                .unwrap_or(self.cursor.get())
        }
    }

    fn add_gating_sequences(&self, gating_sequences: &[Arc<Sequence>]) {
        let mut sequences = self.gating_sequences.write();
        sequences.extend_from_slice(gating_sequences);
    }

    fn remove_gating_sequence(&self, sequence: &Arc<Sequence>) -> bool {
        let mut sequences = self.gating_sequences.write();
        if let Some(pos) = sequences.iter().position(|s| Arc::ptr_eq(s, sequence)) {
            sequences.remove(pos);
            true
        } else {
            false
        }
    }

    fn get_cursor(&self) -> i64 {
        self.cursor.get()
    }

    fn remaining_capacity(&self) -> i64 {
        let next_value = self.next_value.get();
        let consumed = self.get_minimum_sequence();
        (consumed + self.buffer_size as i64) - next_value
    }
}

/// Multi-producer sequencer for concurrent publishing
pub struct MultiProducerSequencer {
    /// Size of the buffer (must be power of 2)
    buffer_size: usize,
    /// Wait strategy for consumers
    wait_strategy: Arc<dyn WaitStrategy>,
    /// Current cursor position
    cursor: Arc<Sequence>,
    /// Gating sequences (consumers)
    gating_sequences: parking_lot::RwLock<Vec<Arc<Sequence>>>,
    /// Available buffer for tracking published sequences
    available_buffer: Vec<AtomicBool>,
    /// Index mask for fast modulo operations
    index_mask: usize,
    /// Index shift for calculating array index
    index_shift: usize,
}

impl MultiProducerSequencer {
    /// Create a new multi-producer sequencer
    pub fn new(buffer_size: usize, wait_strategy: Arc<dyn WaitStrategy>) -> Self {
        let mut available_buffer = Vec::with_capacity(buffer_size);
        for _ in 0..buffer_size {
            available_buffer.push(AtomicBool::new(false));
        }

        Self {
            buffer_size,
            wait_strategy,
            cursor: Arc::new(Sequence::new(INITIAL_CURSOR_VALUE)),
            gating_sequences: parking_lot::RwLock::new(Vec::new()),
            available_buffer,
            index_mask: buffer_size - 1,
            index_shift: Self::calculate_shift(buffer_size),
        }
    }

    fn calculate_shift(buffer_size: usize) -> usize {
        (buffer_size as f64).log2() as usize
    }

    fn calculate_index(&self, sequence: i64) -> usize {
        (sequence as usize) & self.index_mask
    }

    fn calculate_availability_flag(&self, sequence: i64) -> bool {
        (sequence >> self.index_shift) != 0
    }

    fn set_available(&self, sequence: i64) {
        let index = self.calculate_index(sequence);
        let flag = self.calculate_availability_flag(sequence);
        self.available_buffer[index].store(flag, Ordering::Release);
    }

    fn is_sequence_available(&self, sequence: i64) -> bool {
        let index = self.calculate_index(sequence);
        let expected_flag = self.calculate_availability_flag(sequence);
        self.available_buffer[index].load(Ordering::Acquire) == expected_flag
    }
}

impl Sequencer for MultiProducerSequencer {
    fn buffer_size(&self) -> usize {
        self.buffer_size
    }

    fn next(&self) -> Result<i64> {
        self.next_n(1)
    }

    fn next_n(&self, n: usize) -> Result<i64> {
        if n < 1 {
            return Err(DisruptorError::InvalidSequence(n as i64));
        }

        loop {
            let current = self.cursor.get();
            let next = current + n as i64;

            let wrap_point = next - self.buffer_size as i64;
            let cached_gating_sequence = self.get_minimum_sequence();

            if wrap_point > cached_gating_sequence {
                return Err(DisruptorError::BufferFull);
            }

            match self.cursor.compare_and_swap(current, next) {
                Ok(_) => return Ok(next),
                Err(_) => continue, // Retry
            }
        }
    }

    fn try_next(&self) -> Result<Option<i64>> {
        self.try_next_n(1)
    }

    fn try_next_n(&self, n: usize) -> Result<Option<i64>> {
        if n < 1 {
            return Err(DisruptorError::InvalidSequence(n as i64));
        }

        let current = self.cursor.get();
        let next = current + n as i64;

        let wrap_point = next - self.buffer_size as i64;
        let cached_gating_sequence = self.get_minimum_sequence();

        if wrap_point > cached_gating_sequence {
            return Ok(None);
        }

        match self.cursor.compare_and_swap(current, next) {
            Ok(_) => Ok(Some(next)),
            Err(_) => Ok(None), // Someone else claimed it
        }
    }

    fn publish(&self, sequence: i64) {
        self.set_available(sequence);
        self.wait_strategy.signal_all_when_blocking();
    }

    fn publish_range(&self, lo: i64, hi: i64) {
        for seq in lo..=hi {
            self.set_available(seq);
        }
        self.wait_strategy.signal_all_when_blocking();
    }

    fn is_available(&self, sequence: i64) -> bool {
        self.is_sequence_available(sequence)
    }

    fn get_highest_published_sequence(&self, next_sequence: i64, available_sequence: i64) -> i64 {
        for seq in next_sequence..=available_sequence {
            if !self.is_available(seq) {
                return seq - 1;
            }
        }
        available_sequence
    }

    fn new_barrier(&self, sequences_to_track: Vec<Arc<Sequence>>) -> SequenceBarrier {
        SequenceBarrier::new(
            Arc::clone(&self.wait_strategy),
            Arc::clone(&self.cursor),
            sequences_to_track,
        )
    }

    fn get_minimum_sequence(&self) -> i64 {
        let gating_sequences = self.gating_sequences.read();
        if gating_sequences.is_empty() {
            self.cursor.get()
        } else {
            gating_sequences
                .iter()
                .map(|seq| seq.get())
                .min()
                .unwrap_or(self.cursor.get())
        }
    }

    fn add_gating_sequences(&self, gating_sequences: &[Arc<Sequence>]) {
        let mut sequences = self.gating_sequences.write();
        sequences.extend_from_slice(gating_sequences);
    }

    fn remove_gating_sequence(&self, sequence: &Arc<Sequence>) -> bool {
        let mut sequences = self.gating_sequences.write();
        if let Some(pos) = sequences.iter().position(|s| Arc::ptr_eq(s, sequence)) {
            sequences.remove(pos);
            true
        } else {
            false
        }
    }

    fn get_cursor(&self) -> i64 {
        self.cursor.get()
    }

    fn remaining_capacity(&self) -> i64 {
        let consumed = self.get_minimum_sequence();
        let produced = self.cursor.get();
        (consumed + self.buffer_size as i64) - produced
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disruptor::YieldingWaitStrategy;

    #[test]
    fn test_single_producer_sequencer() {
        let wait_strategy = Arc::new(YieldingWaitStrategy::new());
        let sequencer = SingleProducerSequencer::new(8, wait_strategy);

        assert_eq!(sequencer.buffer_size(), 8);
        assert_eq!(sequencer.get_cursor(), INITIAL_CURSOR_VALUE);

        // Test claiming sequences
        let seq1 = sequencer.next().unwrap();
        assert_eq!(seq1, 0);

        let seq2 = sequencer.next_n(3).unwrap();
        assert_eq!(seq2, 3);

        // Test publishing
        sequencer.publish(seq1);
        assert!(sequencer.is_available(seq1));

        sequencer.publish_range(1, seq2);
        assert!(sequencer.is_available(seq2));
    }

    #[test]
    fn test_multi_producer_sequencer() {
        let wait_strategy = Arc::new(YieldingWaitStrategy::new());
        let sequencer = MultiProducerSequencer::new(8, wait_strategy);

        assert_eq!(sequencer.buffer_size(), 8);
        assert_eq!(sequencer.get_cursor(), INITIAL_CURSOR_VALUE);

        // Test claiming sequences
        let seq1 = sequencer.next().unwrap();
        let seq2 = sequencer.next().unwrap();

        assert!(seq1 >= 0);
        assert!(seq2 > seq1);

        // Test publishing
        sequencer.publish(seq1);
        sequencer.publish(seq2);

        assert!(sequencer.is_available(seq1));
        assert!(sequencer.is_available(seq2));
    }

    #[test]
    fn test_sequencer_gating() {
        let wait_strategy = Arc::new(YieldingWaitStrategy::new());
        let sequencer = SingleProducerSequencer::new(4, wait_strategy);

        let consumer_sequence = Arc::new(Sequence::new(INITIAL_CURSOR_VALUE));
        sequencer.add_gating_sequences(&[consumer_sequence.clone()]);

        // Should be able to claim up to buffer size
        for _i in 0..4 {
            let seq = sequencer.next().unwrap();
            sequencer.publish(seq);
        }

        // Should not be able to claim more without consumer progress
        assert!(sequencer.try_next().unwrap().is_none());

        // Consumer progresses
        consumer_sequence.set(1);

        // Should now be able to claim more
        assert!(sequencer.try_next().unwrap().is_some());
    }
}
