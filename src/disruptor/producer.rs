//! Producer module inspired by disruptor-rs
//!
//! This module provides a simplified, user-friendly API for publishing events
//! into the Disruptor, following the patterns established by disruptor-rs.

use crate::disruptor::{RingBuffer, ring_buffer::BatchIterMut};
use std::sync::Arc;

/// Error indicating that the ring buffer is full
#[derive(Debug, Clone, PartialEq)]
pub struct RingBufferFull;

impl std::fmt::Display for RingBufferFull {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Ring Buffer is full")
    }
}

impl std::error::Error for RingBufferFull {}

/// Error indicating missing free slots for batch publication
#[derive(Debug, Clone, PartialEq)]
pub struct MissingFreeSlots(pub u64);

impl std::fmt::Display for MissingFreeSlots {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Missing free slots in Ring Buffer: {}", self.0)
    }
}

impl std::error::Error for MissingFreeSlots {}

/// Producer trait inspired by disruptor-rs
///
/// Provides a simplified API for publishing events into the Disruptor.
/// This follows the exact pattern from disruptor-rs for maximum compatibility
/// and ease of use.
pub trait Producer<E>
where
    E: Send + Sync,
{
    /// Publish an Event into the Disruptor
    ///
    /// Returns a `Result` with the published sequence number or a [RingBufferFull]
    /// in case the ring buffer is full.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// producer.try_publish(|e| { e.price = 42.0; })?;
    /// ```
    fn try_publish<F>(&mut self, update: F) -> std::result::Result<i64, RingBufferFull>
    where
        F: FnOnce(&mut E);

    /// Publish a batch of Events into the Disruptor
    ///
    /// Returns a `Result` with the upper published sequence number or a [MissingFreeSlots]
    /// in case the ring buffer did not have enough available slots for the batch.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// producer.try_batch_publish(3, |iter| {
    ///     for e in iter {
    ///         e.price = 42.0;
    ///     }
    /// })?;
    /// ```
    fn try_batch_publish<'a, F>(&'a mut self, n: usize, update: F) -> std::result::Result<i64, MissingFreeSlots>
    where
        E: 'a,
        F: FnOnce(BatchIterMut<'a, E>);

    /// Publish an Event into the Disruptor
    ///
    /// Spins until there is an available slot in case the ring buffer is full.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// producer.publish(|e| { e.price = 42.0; });
    /// ```
    fn publish<F>(&mut self, update: F)
    where
        F: FnOnce(&mut E);

    /// Publish a batch of Events into the Disruptor
    ///
    /// Spins until there are enough available slots in the ring buffer.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// producer.batch_publish(3, |iter| {
    ///     for e in iter {
    ///         e.price = 42.0;
    ///     }
    /// });
    /// ```
    fn batch_publish<'a, F>(&'a mut self, n: usize, update: F)
    where
        E: 'a,
        F: FnOnce(BatchIterMut<'a, E>);
}

/// Simple producer implementation for single-threaded scenarios
///
/// This provides a simplified interface over the existing RingBuffer,
/// making it easier to use while maintaining full performance.
pub struct SimpleProducer<T>
where
    T: Send + Sync,
{
    ring_buffer: Arc<RingBuffer<T>>,
    sequence: i64,
}

impl<T> SimpleProducer<T>
where
    T: Send + Sync,
{
    /// Create a new simple producer
    ///
    /// # Arguments
    /// * `ring_buffer` - The ring buffer to publish to
    ///
    /// # Returns
    /// A new SimpleProducer instance
    pub fn new(ring_buffer: Arc<RingBuffer<T>>) -> Self {
        Self {
            ring_buffer,
            sequence: 0,
        }
    }

    /// Get the current sequence
    pub fn current_sequence(&self) -> i64 {
        self.sequence
    }

    /// Check if we have capacity for n events
    fn has_capacity(&self, n: usize) -> bool {
        // For now, we'll implement a simple capacity check
        // In a full implementation, this would check against consumer sequences
        let buffer_size = self.ring_buffer.size();
        (self.sequence + n as i64) < buffer_size
    }

    /// Wait for capacity (spinning implementation)
    fn wait_for_capacity(&self, n: usize) {
        while !self.has_capacity(n) {
            std::hint::spin_loop();
        }
    }
}

impl<T> Producer<T> for SimpleProducer<T>
where
    T: Send + Sync,
{
    fn try_publish<F>(&mut self, update: F) -> std::result::Result<i64, RingBufferFull>
    where
        F: FnOnce(&mut T),
    {
        if !self.has_capacity(1) {
            return Err(RingBufferFull);
        }

        let sequence = self.sequence;
        // SAFETY: We have exclusive access to this sequence
        let event = unsafe { &mut *self.ring_buffer.get_mut_unchecked(sequence) };
        update(event);

        self.sequence += 1;
        Ok(sequence)
    }

    fn try_batch_publish<'a, F>(&'a mut self, n: usize, update: F) -> std::result::Result<i64, MissingFreeSlots>
    where
        T: 'a,
        F: FnOnce(BatchIterMut<'a, T>),
    {
        if !self.has_capacity(n) {
            let missing = n - self.ring_buffer.size() as usize;
            return Err(MissingFreeSlots(missing as u64));
        }

        let start_sequence = self.sequence;
        let end_sequence = start_sequence + n as i64 - 1;

        // SAFETY: We have exclusive access to this sequence range
        let iter = unsafe { self.ring_buffer.batch_iter_mut(start_sequence, end_sequence) };
        update(iter);

        self.sequence += n as i64;
        Ok(end_sequence)
    }

    fn publish<F>(&mut self, update: F)
    where
        F: FnOnce(&mut T),
    {
        self.wait_for_capacity(1);

        let sequence = self.sequence;
        // SAFETY: We have exclusive access to this sequence
        let event = unsafe { &mut *self.ring_buffer.get_mut_unchecked(sequence) };
        update(event);

        self.sequence += 1;
    }

    fn batch_publish<'a, F>(&'a mut self, n: usize, update: F)
    where
        T: 'a,
        F: FnOnce(BatchIterMut<'a, T>),
    {
        self.wait_for_capacity(n);

        let start_sequence = self.sequence;
        let end_sequence = start_sequence + n as i64 - 1;

        // SAFETY: We have exclusive access to this sequence range
        let iter = unsafe { self.ring_buffer.batch_iter_mut(start_sequence, end_sequence) };
        update(iter);

        self.sequence += n as i64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disruptor::{RingBuffer, event_factory::ClosureEventFactory};

    #[derive(Debug, Clone)]
    struct TestEvent {
        value: i32,
    }

    #[test]
    fn test_simple_producer_creation() {
        let factory = ClosureEventFactory::new(|| TestEvent { value: 0 });
        let ring_buffer = Arc::new(RingBuffer::new(8, factory).unwrap());
        let producer = SimpleProducer::new(ring_buffer);

        assert_eq!(producer.current_sequence(), 0);
    }

    #[test]
    fn test_try_publish() {
        let factory = ClosureEventFactory::new(|| TestEvent { value: 0 });
        let ring_buffer = Arc::new(RingBuffer::new(8, factory).unwrap());
        let mut producer = SimpleProducer::new(ring_buffer.clone());

        let result = producer.try_publish(|e| { e.value = 42; });
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);

        // Verify the event was updated
        let event = ring_buffer.get(0);
        assert_eq!(event.value, 42);
    }

    #[test]
    fn test_batch_publish() {
        let factory = ClosureEventFactory::new(|| TestEvent { value: 0 });
        let ring_buffer = Arc::new(RingBuffer::new(8, factory).unwrap());
        let mut producer = SimpleProducer::new(ring_buffer.clone());

        let result = producer.try_batch_publish(3, |iter| {
            for (i, e) in iter.enumerate() {
                e.value = i as i32 + 10;
            }
        });

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 2); // End sequence

        // Verify the events were updated
        assert_eq!(ring_buffer.get(0).value, 10);
        assert_eq!(ring_buffer.get(1).value, 11);
        assert_eq!(ring_buffer.get(2).value, 12);
    }
}
