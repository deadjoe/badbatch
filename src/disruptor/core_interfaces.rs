//! Core Interfaces for the Disruptor Pattern
//!
//! This module defines the fundamental interfaces that form the backbone
//! of the LMAX Disruptor pattern. These interfaces provide abstractions
//! for cursor management, sequencing operations, and data access.

use crate::disruptor::Result;

/// Provides access to a cursor value
///
/// This trait is equivalent to the Cursored interface in the original LMAX Disruptor.
/// It's used for dynamic add/remove of sequences and provides a way to access
/// the current cursor position.
pub trait Cursored {
    /// Get the current cursor value
    ///
    /// # Returns
    /// The current cursor value as a sequence number
    fn get_cursor(&self) -> i64;
}

/// Operations related to sequencing items in a ring buffer
///
/// This trait is equivalent to the Sequenced interface in the original LMAX Disruptor.
/// It defines the core operations for claiming sequences and publishing events.
pub trait Sequenced {
    /// Get the capacity of the data structure
    ///
    /// # Returns
    /// The size of the ring buffer
    fn get_buffer_size(&self) -> usize;

    /// Check if the buffer has capacity for additional sequences
    ///
    /// This is a concurrent method, so the response should only be taken
    /// as an indication of available capacity.
    ///
    /// # Arguments
    /// * `required_capacity` - The number of sequences needed
    ///
    /// # Returns
    /// True if the buffer has capacity, false otherwise
    fn has_available_capacity(&self, required_capacity: usize) -> bool;

    /// Get the remaining capacity for this sequencer
    ///
    /// # Returns
    /// The number of slots remaining
    fn remaining_capacity(&self) -> i64;

    /// Claim the next event in sequence for publishing
    ///
    /// # Returns
    /// The claimed sequence value
    ///
    /// # Errors
    /// Returns an error if there's insufficient capacity
    fn next(&self) -> Result<i64>;

    /// Claim the next n events in sequence for publishing
    ///
    /// This is for batch event producing. Using batch producing requires
    /// careful coordination:
    ///
    /// ```ignore
    /// let n = 10;
    /// let hi = sequencer.next_n(n)?;
    /// let lo = hi - (n - 1);
    /// for sequence in lo..=hi {
    ///     // Do work
    /// }
    /// sequencer.publish_range(lo, hi);
    /// ```
    ///
    /// # Arguments
    /// * `n` - The number of sequences to claim
    ///
    /// # Returns
    /// The highest claimed sequence value
    ///
    /// # Errors
    /// Returns an error if there's insufficient capacity
    fn next_n(&self, n: i64) -> Result<i64>;

    /// Attempt to claim the next event in sequence for publishing
    ///
    /// Will return the sequence number if there is at least one slot available.
    ///
    /// # Returns
    /// Some(sequence) if successful, None if insufficient capacity
    fn try_next(&self) -> Option<i64>;

    /// Attempt to claim the next n events in sequence for publishing
    ///
    /// Will return the highest numbered slot if there are at least n slots available.
    ///
    /// # Arguments
    /// * `n` - The number of sequences to claim
    ///
    /// # Returns
    /// Some(sequence) if successful, None if insufficient capacity
    fn try_next_n(&self, n: i64) -> Option<i64>;

    /// Publish a sequence
    ///
    /// Call when the event has been filled.
    ///
    /// # Arguments
    /// * `sequence` - The sequence to be published
    fn publish(&self, sequence: i64);

    /// Batch publish sequences
    ///
    /// Called when all of the events have been filled.
    ///
    /// # Arguments
    /// * `lo` - First sequence number to publish
    /// * `hi` - Last sequence number to publish
    fn publish_range(&self, lo: i64, hi: i64);
}

/// Provides data access abstraction
///
/// This trait is equivalent to the DataProvider interface in the original LMAX Disruptor.
/// It's typically used to decouple classes from RingBuffer to allow easier testing.
pub trait DataProvider<T> {
    /// Get the data item at the specified sequence
    ///
    /// # Arguments
    /// * `sequence` - The sequence at which to find the data
    ///
    /// # Returns
    /// The data item located at that sequence
    fn get(&self, sequence: i64) -> &T;
}

/// Event sink for publishing events
///
/// This trait provides methods for publishing events to the ring buffer
/// using various event translator patterns.
pub trait EventSink<T> {
    /// Publish an event using an event translator
    ///
    /// # Arguments
    /// * `translator` - Function that translates data into the event
    fn publish_event<F>(&self, translator: F)
    where
        F: FnOnce(&mut T, i64);

    /// Publish an event with one argument using an event translator
    ///
    /// # Arguments
    /// * `translator` - Function that translates data into the event
    /// * `arg` - Argument to pass to the translator
    fn publish_event_one_arg<A, F>(&self, translator: F, arg: A)
    where
        F: FnOnce(&mut T, i64, A);

    /// Publish an event with two arguments using an event translator
    ///
    /// # Arguments
    /// * `translator` - Function that translates data into the event
    /// * `arg0` - First argument to pass to the translator
    /// * `arg1` - Second argument to pass to the translator
    fn publish_event_two_arg<A, B, F>(&self, translator: F, arg0: A, arg1: B)
    where
        F: FnOnce(&mut T, i64, A, B);

    /// Publish an event with three arguments using an event translator
    ///
    /// # Arguments
    /// * `translator` - Function that translates data into the event
    /// * `arg0` - First argument to pass to the translator
    /// * `arg1` - Second argument to pass to the translator
    /// * `arg2` - Third argument to pass to the translator
    fn publish_event_three_arg<A, B, C, F>(&self, translator: F, arg0: A, arg1: B, arg2: C)
    where
        F: FnOnce(&mut T, i64, A, B, C);

    /// Try to publish an event (non-blocking)
    ///
    /// # Arguments
    /// * `translator` - Function that translates data into the event
    ///
    /// # Returns
    /// True if the event was published, false if there was insufficient capacity
    fn try_publish_event<F>(&self, translator: F) -> bool
    where
        F: FnOnce(&mut T, i64);
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test implementations for the traits
    struct TestCursored {
        cursor: i64,
    }

    impl Cursored for TestCursored {
        fn get_cursor(&self) -> i64 {
            self.cursor
        }
    }

    struct TestDataProvider {
        data: Vec<i32>,
    }

    impl DataProvider<i32> for TestDataProvider {
        fn get(&self, sequence: i64) -> &i32 {
            &self.data[sequence as usize % self.data.len()]
        }
    }

    #[test]
    fn test_cursored_trait() {
        let cursored = TestCursored { cursor: 42 };
        assert_eq!(cursored.get_cursor(), 42);
    }

    #[test]
    fn test_data_provider_trait() {
        let provider = TestDataProvider {
            data: vec![1, 2, 3, 4, 5],
        };

        assert_eq!(*provider.get(0), 1);
        assert_eq!(*provider.get(2), 3);
        assert_eq!(*provider.get(7), 3); // Wraps around
    }
}
