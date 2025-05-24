//! Ring Buffer implementation for the Disruptor
//!
//! The Ring Buffer is the core data structure that stores events in a circular
//! buffer. It provides lock-free access to pre-allocated events and ensures
//! mechanical sympathy through careful memory layout.

use crate::disruptor::{Event, EventFactory, Result, DisruptorError, is_power_of_two};
use std::sync::Arc;

/// A lock-free ring buffer for storing events
///
/// The ring buffer pre-allocates all events and provides lock-free access
/// through careful use of memory barriers and atomic operations.
#[derive(Debug)]
pub struct RingBuffer<T: Event> {
    /// The buffer storing all events
    buffer: Vec<T>,
    /// Mask for fast modulo operations (buffer_size - 1)
    index_mask: usize,
    /// Size of the buffer (must be power of 2)
    buffer_size: usize,
}

impl<T: Event> RingBuffer<T> {
    /// Create a new ring buffer with the specified size and event factory
    ///
    /// # Arguments
    /// * `buffer_size` - Size of the buffer (must be power of 2)
    /// * `event_factory` - Factory for creating events
    ///
    /// # Returns
    /// A new ring buffer instance
    ///
    /// # Errors
    /// Returns `DisruptorError::InvalidBufferSize` if buffer_size is not a power of 2
    pub fn new<F>(buffer_size: usize, event_factory: F) -> Result<Self>
    where
        F: EventFactory<T>,
    {
        if !is_power_of_two(buffer_size) {
            return Err(DisruptorError::InvalidBufferSize(buffer_size));
        }

        let mut buffer = Vec::with_capacity(buffer_size);
        for _ in 0..buffer_size {
            buffer.push(event_factory.new_instance());
        }

        Ok(Self {
            buffer,
            index_mask: buffer_size - 1,
            buffer_size,
        })
    }

    /// Get the event at the specified sequence
    ///
    /// # Arguments
    /// * `sequence` - The sequence number
    ///
    /// # Returns
    /// A reference to the event at the sequence
    #[inline]
    pub fn get(&self, sequence: i64) -> &T {
        let index = (sequence as usize) & self.index_mask;
        &self.buffer[index]
    }

    /// Get a mutable reference to the event at the specified sequence
    ///
    /// # Arguments
    /// * `sequence` - The sequence number
    ///
    /// # Returns
    /// A mutable reference to the event at the sequence
    #[inline]
    pub fn get_mut(&mut self, sequence: i64) -> &mut T {
        let index = (sequence as usize) & self.index_mask;
        &mut self.buffer[index]
    }

    /// Get the buffer size
    #[inline]
    pub fn buffer_size(&self) -> usize {
        self.buffer_size
    }

    /// Check if the buffer has capacity for the given sequence
    ///
    /// # Arguments
    /// * `required_capacity` - The sequence that needs to be available
    /// * `available_capacity` - The current available capacity
    ///
    /// # Returns
    /// True if there is sufficient capacity
    #[inline]
    pub fn has_available_capacity(&self, required_capacity: i64, available_capacity: i64) -> bool {
        required_capacity <= available_capacity + (self.buffer_size as i64)
    }

    /// Get the remaining capacity
    ///
    /// # Arguments
    /// * `current_sequence` - The current sequence
    /// * `next_sequence` - The next sequence to be published
    ///
    /// # Returns
    /// The remaining capacity
    #[inline]
    pub fn remaining_capacity(&self, current_sequence: i64, next_sequence: i64) -> i64 {
        (current_sequence + self.buffer_size as i64) - next_sequence
    }
}

// Ring buffer is Send + Sync as long as the events are Send + Sync
unsafe impl<T: Event> Send for RingBuffer<T> {}
unsafe impl<T: Event> Sync for RingBuffer<T> {}

/// A thread-safe wrapper around the ring buffer
pub struct SharedRingBuffer<T: Event> {
    inner: Arc<parking_lot::RwLock<RingBuffer<T>>>,
}

impl<T: Event> SharedRingBuffer<T> {
    /// Create a new shared ring buffer
    pub fn new<F>(buffer_size: usize, event_factory: F) -> Result<Self>
    where
        F: EventFactory<T>,
    {
        let ring_buffer = RingBuffer::new(buffer_size, event_factory)?;
        Ok(Self {
            inner: Arc::new(parking_lot::RwLock::new(ring_buffer)),
        })
    }

    /// Get a read-only reference to the event at the specified sequence
    pub fn get(&self, sequence: i64) -> parking_lot::MappedRwLockReadGuard<T> {
        parking_lot::RwLockReadGuard::map(self.inner.read(), |rb| rb.get(sequence))
    }

    /// Get a mutable reference to the event at the specified sequence
    pub fn get_mut(&self, sequence: i64) -> parking_lot::MappedRwLockWriteGuard<T> {
        parking_lot::RwLockWriteGuard::map(self.inner.write(), |rb| rb.get_mut(sequence))
    }

    /// Get the buffer size
    pub fn buffer_size(&self) -> usize {
        self.inner.read().buffer_size()
    }

    /// Check if the buffer has available capacity
    pub fn has_available_capacity(&self, required_capacity: i64, available_capacity: i64) -> bool {
        self.inner.read().has_available_capacity(required_capacity, available_capacity)
    }

    /// Get the remaining capacity
    pub fn remaining_capacity(&self, current_sequence: i64, next_sequence: i64) -> i64 {
        self.inner.read().remaining_capacity(current_sequence, next_sequence)
    }

    /// Clone the shared reference
    pub fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone)]
    struct TestEvent {
        value: i64,
    }

    impl Event for TestEvent {
        fn clear(&mut self) {
            self.value = 0;
        }
    }

    struct TestEventFactory;

    impl EventFactory<TestEvent> for TestEventFactory {
        fn new_instance(&self) -> TestEvent {
            TestEvent { value: 0 }
        }
    }

    #[test]
    fn test_ring_buffer_creation() {
        let factory = TestEventFactory;
        let buffer = RingBuffer::new(8, factory).unwrap();
        assert_eq!(buffer.buffer_size(), 8);
    }

    #[test]
    fn test_ring_buffer_invalid_size() {
        let factory = TestEventFactory;
        let result = RingBuffer::new(7, factory); // Not power of 2
        assert!(result.is_err());

        match result.unwrap_err() {
            DisruptorError::InvalidBufferSize(size) => assert_eq!(size, 7),
            _ => panic!("Expected InvalidBufferSize error"),
        }
    }

    #[test]
    fn test_ring_buffer_get() {
        let factory = TestEventFactory;
        let mut buffer = RingBuffer::new(8, factory).unwrap();

        // Test wrapping around the buffer
        for i in 0..16 {
            let event = buffer.get_mut(i);
            event.value = i;
        }

        // Check that values wrapped correctly
        assert_eq!(buffer.get(0).value, 8);  // Overwritten by sequence 8
        assert_eq!(buffer.get(1).value, 9);  // Overwritten by sequence 9
        assert_eq!(buffer.get(8).value, 8);  // Same as sequence 0
        assert_eq!(buffer.get(15).value, 15); // Last written value
    }

    #[test]
    fn test_ring_buffer_capacity() {
        let factory = TestEventFactory;
        let buffer = RingBuffer::new(8, factory).unwrap();

        // Test capacity calculations
        assert!(buffer.has_available_capacity(0, 0));
        assert!(buffer.has_available_capacity(7, 0));
        assert!(!buffer.has_available_capacity(9, 0));

        assert_eq!(buffer.remaining_capacity(0, 0), 8);
        assert_eq!(buffer.remaining_capacity(0, 4), 4);
        assert_eq!(buffer.remaining_capacity(10, 5), 13);
    }

    #[test]
    fn test_shared_ring_buffer() {
        let factory = TestEventFactory;
        let shared_buffer = SharedRingBuffer::new(8, factory).unwrap();

        // Test basic operations
        assert_eq!(shared_buffer.buffer_size(), 8);

        // Test mutable access
        {
            let mut event = shared_buffer.get_mut(0);
            event.value = 42;
        }

        // Test read access
        {
            let event = shared_buffer.get(0);
            assert_eq!(event.value, 42);
        }

        // Test cloning
        let cloned = shared_buffer.clone();
        assert_eq!(cloned.buffer_size(), 8);
    }

    #[test]
    fn test_ring_buffer_thread_safety() {
        use std::thread;
        use std::sync::Arc;

        let factory = TestEventFactory;
        let shared_buffer = Arc::new(SharedRingBuffer::new(8, factory).unwrap());
        let mut handles = vec![];

        // Spawn multiple threads to write to different sequences
        for i in 0..4 {
            let buffer_clone = Arc::clone(&shared_buffer);
            let handle = thread::spawn(move || {
                let mut event = buffer_clone.get_mut(i);
                event.value = i;
            });
            handles.push(handle);
        }

        // Wait for all threads to complete
        for handle in handles {
            handle.join().unwrap();
        }

        // Verify all values were written correctly
        for i in 0..4 {
            let event = shared_buffer.get(i);
            assert_eq!(event.value, i);
        }
    }
}
