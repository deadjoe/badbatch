//! Ring Buffer Implementation
//!
//! This module provides the core ring buffer for the Disruptor pattern.
//! The ring buffer is a pre-allocated circular array that stores events
//! and provides lock-free access through careful use of memory barriers.

use crate::disruptor::core_interfaces::DataProvider;
use crate::disruptor::{is_power_of_two, DisruptorError, EventFactory, Result};
use std::cell::UnsafeCell;

#[cfg(feature = "shared-ring-buffer")]
use std::sync::Arc;

/// The core ring buffer for storing events
///
/// This is the heart of the Disruptor pattern. It pre-allocates all events
/// and provides lock-free access through careful use of memory barriers and
/// atomic operations. This follows the exact design from the original LMAX
/// Disruptor RingBuffer with optimizations inspired by disruptor-rs.
///
/// # Type Parameters
/// * `T` - The event type stored in the buffer
#[derive(Debug)]
pub struct RingBuffer<T> {
    /// The buffer storing all events using UnsafeCell for interior mutability
    /// Using `Box<[UnsafeCell<T>]>` for better memory layout than `Vec<T>`
    slots: Box<[UnsafeCell<T>]>,
    /// Mask for fast modulo operations (buffer_size - 1)
    /// Using i64 to match sequence type and avoid casting
    index_mask: i64,
}

impl<T> RingBuffer<T>
where
    T: Send + Sync,
{
    /// Create a new ring buffer with the specified size and event factory
    ///
    /// # Arguments
    /// * `buffer_size` - The size of the ring buffer (must be a power of 2)
    /// * `event_factory` - Factory for creating events to pre-populate the buffer
    ///
    /// # Returns
    /// A new RingBuffer instance
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

        // Pre-allocate all events using UnsafeCell for interior mutability
        // Using Box<[UnsafeCell<T>]> for better memory layout than Vec<T>
        let slots: Box<[UnsafeCell<T>]> = (0..buffer_size)
            .map(|_| UnsafeCell::new(event_factory.new_instance()))
            .collect();

        Ok(Self {
            slots,
            index_mask: (buffer_size - 1) as i64,
        })
    }

    /// Get a reference to the event at the specified sequence
    ///
    /// # Arguments
    /// * `sequence` - The sequence number of the event
    ///
    /// # Returns
    /// A reference to the event at the specified sequence
    pub fn get(&self, sequence: i64) -> &T {
        let index = (sequence & self.index_mask) as usize;
        // SAFETY: Index is within bounds - guaranteed by invariant and index mask.
        let slot = unsafe { self.slots.get_unchecked(index) };
        unsafe { &*slot.get() }
    }

    /// Get a mutable reference to the event at the specified sequence
    ///
    /// # Arguments
    /// * `sequence` - The sequence number of the event
    ///
    /// # Returns
    /// A mutable reference to the event at the specified sequence
    pub fn get_mut(&mut self, sequence: i64) -> &mut T {
        let index = (sequence & self.index_mask) as usize;
        // SAFETY: We have exclusive access to self, so this is safe
        let slot = unsafe { self.slots.get_unchecked(index) };
        unsafe { &mut *slot.get() }
    }

    /// Get a mutable reference to the event at the specified sequence (unsafe version)
    ///
    /// This follows the disruptor-rs pattern of returning a raw pointer for maximum performance.
    /// Callers must ensure that only a single mutable reference or multiple immutable references
    /// exist at any point in time.
    ///
    /// # Arguments
    /// * `sequence` - The sequence number of the event
    ///
    /// # Returns
    /// A raw mutable pointer to the event at the specified sequence
    ///
    /// # Safety
    /// This method is unsafe because it allows mutable access without checking
    /// for exclusive access. The caller must ensure that only one thread
    /// accesses the event mutably at a time.
    pub unsafe fn get_mut_unchecked(&self, sequence: i64) -> *mut T {
        let index = (sequence & self.index_mask) as usize;
        // SAFETY: Index is within bounds - guaranteed by invariant and index mask.
        let slot = self.slots.get_unchecked(index);
        slot.get()
    }

    /// Get the size of the buffer
    ///
    /// # Returns
    /// The size of the ring buffer
    pub fn buffer_size(&self) -> usize {
        self.slots.len()
    }

    /// Get the size of the buffer as i64
    ///
    /// # Returns
    /// The size of the ring buffer as i64 (matching disruptor-rs pattern)
    pub fn size(&self) -> i64 {
        self.slots.len() as i64
    }

    /// Check if the buffer has available capacity
    ///
    /// This is used by producers to check if they can publish more events
    /// without overwriting events that haven't been consumed yet.
    ///
    /// # Arguments
    /// * `required_capacity` - The number of slots required
    /// * `available_capacity` - The number of slots currently available
    ///
    /// # Returns
    /// True if there is sufficient capacity, false otherwise
    pub fn has_available_capacity(&self, required_capacity: i64, available_capacity: i64) -> bool {
        available_capacity >= required_capacity
    }

    /// Get the remaining capacity
    ///
    /// # Arguments
    /// * `current_sequence` - The current sequence position
    /// * `next_sequence` - The next sequence position
    ///
    /// # Returns
    /// The remaining capacity in the buffer
    pub fn remaining_capacity(&self, current_sequence: i64, next_sequence: i64) -> i64 {
        let buffer_size = self.size();
        buffer_size - (next_sequence - current_sequence)
    }

    /// Get the number of free slots in the buffer
    ///
    /// This follows the disruptor-rs pattern for capacity calculation
    ///
    /// # Arguments
    /// * `producer_sequence` - The current producer sequence
    /// * `consumer_sequence` - The current consumer sequence
    ///
    /// # Returns
    /// The number of free slots available
    pub fn free_slots(&self, producer_sequence: i64, consumer_sequence: i64) -> i64 {
        self.size() - (producer_sequence - consumer_sequence)
    }

    /// Create a batch iterator for mutable access to a range of events
    ///
    /// This follows the disruptor-rs pattern for batch processing
    ///
    /// # Arguments
    /// * `start` - The starting sequence (inclusive)
    /// * `end` - The ending sequence (inclusive)
    ///
    /// # Returns
    /// A batch iterator for mutable access to events
    ///
    /// # Safety
    /// The caller must ensure exclusive access to the specified range
    pub unsafe fn batch_iter_mut(&self, start: i64, end: i64) -> BatchIterMut<'_, T>
    where
        T: Send + Sync,
    {
        BatchIterMut::new(start, end, self)
    }
}

/// Iterator for batch mutable access to events (inspired by disruptor-rs)
pub struct BatchIterMut<'a, T>
where
    T: Send + Sync,
{
    ring_buffer: &'a RingBuffer<T>,
    current: i64,
    last: i64,
}

impl<'a, T> BatchIterMut<'a, T>
where
    T: Send + Sync,
{
    fn new(start: i64, end: i64, ring_buffer: &'a RingBuffer<T>) -> Self {
        Self {
            ring_buffer,
            current: start,
            last: end,
        }
    }

    fn remaining(&self) -> usize {
        (self.last - self.current + 1) as usize
    }
}

impl<'a, T> Iterator for BatchIterMut<'a, T>
where
    T: Send + Sync,
{
    type Item = &'a mut T;

    fn next(&mut self) -> Option<Self::Item> {
        if self.current > self.last {
            None
        } else {
            // SAFETY: Iterator has exclusive access to event range
            let event_ptr = unsafe { self.ring_buffer.get_mut_unchecked(self.current) };
            let event = unsafe { &mut *event_ptr };
            self.current += 1;
            Some(event)
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.remaining();
        (remaining, Some(remaining))
    }

    fn count(self) -> usize
    where
        Self: Sized,
    {
        self.remaining()
    }
}

// Ensure RingBuffer is Send and Sync for multi-threading
// SAFETY: RingBuffer<T> is Send and Sync when T is Send + Sync because:
// - The ring buffer uses UnsafeCell internally, but access is coordinated through sequencers
// - All mutations are protected by sequence ordering and happen-before relationships
// - The atomic sequence numbers ensure proper synchronization across threads
unsafe impl<T: Send + Sync> Send for RingBuffer<T> {}
unsafe impl<T: Send + Sync> Sync for RingBuffer<T> {}

/// Implementation of DataProvider trait for RingBuffer
///
/// This allows RingBuffer to be used as a data provider in the core interfaces,
/// providing a clean abstraction for data access.
impl<T> DataProvider<T> for RingBuffer<T>
where
    T: Send + Sync,
{
    fn get(&self, sequence: i64) -> &T {
        self.get(sequence)
    }
}

/// A thread-safe wrapper around the ring buffer
///
/// This provides shared access to the ring buffer across multiple threads
/// using Arc and appropriate synchronization primitives.
///
/// **Warning**: This implementation uses locks and violates the lock-free
/// principle of the Disruptor pattern. It's provided for compatibility
/// but should be avoided in performance-critical scenarios.
#[cfg(feature = "shared-ring-buffer")]
#[derive(Debug)]
pub struct SharedRingBuffer<T>
where
    T: Send + Sync,
{
    inner: Arc<parking_lot::RwLock<RingBuffer<T>>>,
}

#[cfg(feature = "shared-ring-buffer")]
impl<T> SharedRingBuffer<T>
where
    T: Send + Sync,
{
    /// Create a new shared ring buffer with a default event factory
    ///
    /// # Arguments
    /// * `buffer_size` - The size of the ring buffer (must be a power of 2)
    ///
    /// # Returns
    /// A new SharedRingBuffer instance
    ///
    /// # Errors
    /// Returns `DisruptorError::InvalidBufferSize` if buffer_size is not a power of 2
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
    ///
    /// # Arguments
    /// * `sequence` - The sequence number of the event
    ///
    /// # Returns
    /// A mapped read guard containing the event
    pub fn get(&self, sequence: i64) -> parking_lot::MappedRwLockReadGuard<T> {
        parking_lot::RwLockReadGuard::map(self.inner.read(), |rb| rb.get(sequence))
    }

    /// Get a mutable reference to the event at the specified sequence
    ///
    /// # Arguments
    /// * `sequence` - The sequence number of the event
    ///
    /// # Returns
    /// A mapped write guard containing the event
    pub fn get_mut(&self, sequence: i64) -> parking_lot::MappedRwLockWriteGuard<T> {
        parking_lot::RwLockWriteGuard::map(self.inner.write(), |rb| rb.get_mut(sequence))
    }

    /// Get the buffer size
    ///
    /// # Returns
    /// The size of the ring buffer
    pub fn buffer_size(&self) -> usize {
        self.inner.read().buffer_size()
    }

    /// Check if the buffer has available capacity
    ///
    /// # Arguments
    /// * `required_capacity` - The number of slots required
    /// * `available_capacity` - The number of slots currently available
    ///
    /// # Returns
    /// True if there is sufficient capacity, false otherwise
    pub fn has_available_capacity(&self, required_capacity: i64, available_capacity: i64) -> bool {
        self.inner
            .read()
            .has_available_capacity(required_capacity, available_capacity)
    }

    /// Get the remaining capacity
    ///
    /// # Arguments
    /// * `current_sequence` - The current sequence position
    /// * `next_sequence` - The next sequence position
    ///
    /// # Returns
    /// The remaining capacity in the buffer
    pub fn remaining_capacity(&self, current_sequence: i64, next_sequence: i64) -> i64 {
        self.inner
            .read()
            .remaining_capacity(current_sequence, next_sequence)
    }
}

#[cfg(feature = "shared-ring-buffer")]
impl<T> Clone for SharedRingBuffer<T>
where
    T: Send + Sync,
{
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disruptor::DefaultEventFactory;

    #[derive(Debug, Default, Clone)]
    struct TestEvent {
        value: i64,
    }

    #[test]
    fn test_ring_buffer_creation() {
        let factory = DefaultEventFactory::<TestEvent>::new();
        let buffer = RingBuffer::new(8, factory).unwrap();
        assert_eq!(buffer.buffer_size(), 8);
    }

    #[test]
    fn test_ring_buffer_invalid_size() {
        let factory = DefaultEventFactory::<TestEvent>::new();
        let result = RingBuffer::new(7, factory); // Not a power of 2
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            DisruptorError::InvalidBufferSize(7)
        ));
    }

    #[test]
    fn test_ring_buffer_access() {
        let factory = DefaultEventFactory::<TestEvent>::new();
        let mut buffer = RingBuffer::new(8, factory).unwrap();

        // Test mutable access
        {
            let event = buffer.get_mut(0);
            event.value = 42;
        }

        // Test read access
        {
            let event = buffer.get(0);
            assert_eq!(event.value, 42);
        }

        // Test wrapping
        {
            let event = buffer.get_mut(8); // Should wrap to index 0
            event.value = 100;
        }

        {
            let event = buffer.get(0);
            assert_eq!(event.value, 100); // Should be the same slot
        }
    }

    #[test]
    #[cfg(feature = "shared-ring-buffer")]
    fn test_shared_ring_buffer() {
        let factory = DefaultEventFactory::<TestEvent>::new();
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
    fn test_ring_buffer_size_methods() {
        let factory = DefaultEventFactory::<TestEvent>::new();
        let buffer = RingBuffer::new(16, factory).unwrap();

        assert_eq!(buffer.buffer_size(), 16);
        assert_eq!(buffer.size(), 16);
    }

    #[test]
    fn test_ring_buffer_capacity_methods() {
        let factory = DefaultEventFactory::<TestEvent>::new();
        let buffer = RingBuffer::new(8, factory).unwrap();

        // Test has_available_capacity
        assert!(buffer.has_available_capacity(4, 8));
        assert!(buffer.has_available_capacity(8, 8));
        assert!(!buffer.has_available_capacity(10, 8));

        // Test remaining_capacity
        let remaining = buffer.remaining_capacity(0, 4);
        assert_eq!(remaining, 4); // buffer_size(8) - (4 - 0) = 4

        // Test free_slots
        let free = buffer.free_slots(5, 2);
        assert_eq!(free, 5); // buffer_size(8) - (5 - 2) = 5
    }

    #[test]
    fn test_ring_buffer_unsafe_get_mut() {
        let factory = DefaultEventFactory::<TestEvent>::new();
        let buffer = RingBuffer::new(8, factory).unwrap();

        unsafe {
            let ptr = buffer.get_mut_unchecked(0);
            (*ptr).value = 123;
        }

        // Verify the value was set
        let event = buffer.get(0);
        assert_eq!(event.value, 123);
    }

    #[test]
    fn test_batch_iterator() {
        let factory = DefaultEventFactory::<TestEvent>::new();
        let buffer = RingBuffer::new(8, factory).unwrap();

        // Create a batch iterator
        unsafe {
            let iter = buffer.batch_iter_mut(0, 3);

            // Test size_hint
            let (lower, upper) = iter.size_hint();
            assert_eq!(lower, 4);
            assert_eq!(upper, Some(4));

            // Test count (consumes the iterator)
            let count = iter.count();
            assert_eq!(count, 4);
        }

        // Test iteration
        unsafe {
            let iter = buffer.batch_iter_mut(1, 3);
            let mut values = Vec::new();

            for event in iter {
                event.value = values.len() as i64;
                values.push(event.value);
            }

            assert_eq!(values, vec![0, 1, 2]);
        }

        // Test empty iterator
        unsafe {
            let mut iter = buffer.batch_iter_mut(5, 3); // start > end
            assert!(iter.next().is_none());
        }
    }

    #[test]
    fn test_batch_iterator_remaining() {
        let factory = DefaultEventFactory::<TestEvent>::new();
        let buffer = RingBuffer::new(8, factory).unwrap();

        unsafe {
            let iter = buffer.batch_iter_mut(2, 5);
            assert_eq!(iter.remaining(), 4); // 5 - 2 + 1 = 4
        }
    }

    #[test]
    fn test_ring_buffer_data_provider_trait() {
        let factory = DefaultEventFactory::<TestEvent>::new();
        let mut buffer = RingBuffer::new(8, factory).unwrap();

        // Set a value using mutable access
        {
            let event = buffer.get_mut(0);
            event.value = 456;
        }

        // Test DataProvider trait implementation
        let provider: &dyn DataProvider<TestEvent> = &buffer;
        let event = provider.get(0);
        assert_eq!(event.value, 456);
    }

    #[test]
    fn test_ring_buffer_wrapping_access() {
        let factory = DefaultEventFactory::<TestEvent>::new();
        let mut buffer = RingBuffer::new(4, factory).unwrap();

        // Test multiple wraps
        for i in 0..12 {
            let event = buffer.get_mut(i);
            event.value = i;
        }

        // Verify wrapping behavior - each slot should have the last value written to it
        // Since we wrote values 0-11 to a buffer of size 4, each slot gets overwritten 3 times
        // Slot 0: gets values 0, 4, 8  -> final value is 8
        // Slot 1: gets values 1, 5, 9  -> final value is 9
        // Slot 2: gets values 2, 6, 10 -> final value is 10
        // Slot 3: gets values 3, 7, 11 -> final value is 11
        for i in 0..4 {
            let expected_value = i + 8; // Last value written to each slot
            let event = buffer.get(i);
            assert_eq!(event.value, expected_value);
        }
    }

    #[test]
    #[cfg(feature = "shared-ring-buffer")]
    fn test_shared_ring_buffer_capacity_methods() {
        let factory = DefaultEventFactory::<TestEvent>::new();
        let shared_buffer = SharedRingBuffer::new(8, factory).unwrap();

        // Test has_available_capacity
        assert!(shared_buffer.has_available_capacity(4, 8));
        assert!(!shared_buffer.has_available_capacity(10, 8));

        // Test remaining_capacity
        let remaining = shared_buffer.remaining_capacity(0, 4);
        assert_eq!(remaining, 4);
    }

    #[test]
    #[cfg(feature = "shared-ring-buffer")]
    fn test_shared_ring_buffer_invalid_size() {
        let factory = DefaultEventFactory::<TestEvent>::new();
        let result = SharedRingBuffer::new(7, factory); // Not a power of 2
        assert!(result.is_err());
    }

    #[test]
    fn test_ring_buffer_negative_sequence() {
        let factory = DefaultEventFactory::<TestEvent>::new();
        let mut buffer = RingBuffer::new(8, factory).unwrap();

        // Test negative sequence (should still work due to wrapping)
        let event = buffer.get_mut(-1);
        event.value = 999;

        // Verify negative sequence access
        let event = buffer.get(-1);
        assert_eq!(event.value, 999);

        // Test that -1 wraps to the same position as buffer_size - 1
        let event2 = buffer.get(7); // 8 - 1 = 7
        assert_eq!(event2.value, 999);
    }

    #[test]
    fn test_ring_buffer_large_sequence() {
        let factory = DefaultEventFactory::<TestEvent>::new();
        let mut buffer = RingBuffer::new(4, factory).unwrap();

        // Test very large sequence numbers
        let large_seq = i64::MAX - 100;
        let event = buffer.get_mut(large_seq);
        event.value = 777;

        // Verify large sequence access
        let event = buffer.get(large_seq);
        assert_eq!(event.value, 777);
    }

    #[test]
    fn test_ring_buffer_edge_cases() {
        let factory = DefaultEventFactory::<TestEvent>::new();
        let buffer = RingBuffer::new(1, factory).unwrap(); // Minimum size

        assert_eq!(buffer.buffer_size(), 1);
        assert_eq!(buffer.size(), 1);

        // Test capacity calculations with size 1
        assert!(buffer.has_available_capacity(1, 1));
        assert!(!buffer.has_available_capacity(2, 1));

        let remaining = buffer.remaining_capacity(0, 0);
        assert_eq!(remaining, 1);

        let _free = buffer.free_slots(0, 0);
        assert_eq!(remaining, 1);
    }

    #[test]
    fn test_ring_buffer_zero_capacity_scenarios() {
        let factory = DefaultEventFactory::<TestEvent>::new();
        let buffer = RingBuffer::new(8, factory).unwrap();

        // Test zero required capacity
        assert!(buffer.has_available_capacity(0, 5));

        // Test zero available capacity
        assert!(!buffer.has_available_capacity(1, 0));

        // Test remaining capacity with equal sequences
        let remaining = buffer.remaining_capacity(5, 5);
        assert_eq!(remaining, 8);
    }

    #[test]
    fn test_ring_buffer_thread_safety_markers() {
        let factory = DefaultEventFactory::<TestEvent>::new();
        let buffer = RingBuffer::new(8, factory).unwrap();

        // Test that buffer can be sent between threads
        let handle = std::thread::spawn(move || {
            let event = buffer.get(0);
            event.value
        });

        assert_eq!(handle.join().unwrap(), 0);
    }
}
