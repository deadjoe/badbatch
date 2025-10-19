//! Producer module inspired by disruptor-rs
//!
//! This module provides a simplified, user-friendly API for publishing events
//! into the Disruptor, following the patterns established by disruptor-rs.
//!
//! This implementation has been corrected to properly integrate with the Sequencer
//! component, following the LMAX Disruptor design principles.

use crate::disruptor::{ring_buffer::BatchIterMut, RingBuffer, Sequencer};
use std::convert::TryFrom;
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
    /// ```rust
    /// use badbatch::disruptor::{build_single_producer, BusySpinWaitStrategy};
    ///
    /// #[derive(Default)]
    /// struct MyEvent { price: f64 }
    ///
    /// let mut producer = build_single_producer(8, MyEvent::default, BusySpinWaitStrategy)
    ///     .handle_events_with(|_event, _sequence, _end_of_batch| {
    ///         // Handle event
    ///     })
    ///     .build();
    ///
    /// let sequence = producer.try_publish(|e| { e.price = 42.0; }).unwrap();
    /// # drop(producer); // Clean shutdown
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
    /// ```rust
    /// use badbatch::disruptor::{build_single_producer, BusySpinWaitStrategy};
    ///
    /// #[derive(Default)]
    /// struct MyEvent { price: f64 }
    ///
    /// let mut producer = build_single_producer(8, MyEvent::default, BusySpinWaitStrategy)
    ///     .handle_events_with(|_event, _sequence, _end_of_batch| {
    ///         // Handle event
    ///     })
    ///     .build();
    ///
    /// let sequence = producer.try_batch_publish(3, |iter| {
    ///     for e in iter {
    ///         e.price = 42.0;
    ///     }
    /// }).unwrap();
    /// # drop(producer); // Clean shutdown
    /// ```
    fn try_batch_publish<'a, F>(
        &'a mut self,
        n: usize,
        update: F,
    ) -> std::result::Result<i64, MissingFreeSlots>
    where
        E: 'a,
        F: FnOnce(BatchIterMut<'a, E>);

    /// Publish an Event into the Disruptor
    ///
    /// Spins until there is an available slot in case the ring buffer is full.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use badbatch::disruptor::{build_single_producer, BusySpinWaitStrategy};
    ///
    /// #[derive(Default)]
    /// struct MyEvent { price: f64 }
    ///
    /// let mut producer = build_single_producer(8, MyEvent::default, BusySpinWaitStrategy)
    ///     .handle_events_with(|_event, _sequence, _end_of_batch| {
    ///         // Handle event
    ///     })
    ///     .build();
    ///
    /// producer.publish(|e| { e.price = 42.0; });
    /// # drop(producer); // Clean shutdown
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
    /// ```rust
    /// use badbatch::disruptor::{build_single_producer, BusySpinWaitStrategy};
    ///
    /// #[derive(Default)]
    /// struct MyEvent { price: f64 }
    ///
    /// let mut producer = build_single_producer(8, MyEvent::default, BusySpinWaitStrategy)
    ///     .handle_events_with(|_event, _sequence, _end_of_batch| {
    ///         // Handle event
    ///     })
    ///     .build();
    ///
    /// producer.batch_publish(3, |iter| {
    ///     for e in iter {
    ///         e.price = 42.0;
    ///     }
    /// });
    /// # drop(producer); // Clean shutdown
    /// ```
    fn batch_publish<'a, F>(&'a mut self, n: usize, update: F)
    where
        E: 'a,
        F: FnOnce(BatchIterMut<'a, E>);
}

/// Simple producer implementation that properly integrates with Sequencer
///
/// This producer works with any Sequencer implementation (single or multi-producer)
/// and correctly follows the LMAX Disruptor pattern for sequence allocation and publishing.
#[derive(Debug, Clone)]
pub struct SimpleProducer<T>
where
    T: Send + Sync,
{
    ring_buffer: Arc<RingBuffer<T>>,
    sequencer: Arc<dyn Sequencer>,
}

impl<T> SimpleProducer<T>
where
    T: Send + Sync,
{
    /// Create a new simple producer
    ///
    /// # Arguments
    /// * `ring_buffer` - The ring buffer to publish to
    /// * `sequencer` - The sequencer for coordinating sequence allocation
    ///
    /// # Returns
    /// A new SimpleProducer instance
    pub fn new(ring_buffer: Arc<RingBuffer<T>>, sequencer: Arc<dyn Sequencer>) -> Self {
        Self {
            ring_buffer,
            sequencer,
        }
    }

    /// Get the current cursor from the sequencer
    pub fn current_sequence(&self) -> i64 {
        self.sequencer.get_cursor().get()
    }

    // Note: wait_for_capacity is no longer needed as we use sequencer.next() which blocks
}

impl<T> Producer<T> for SimpleProducer<T>
where
    T: Send + Sync,
{
    fn try_publish<F>(&mut self, update: F) -> std::result::Result<i64, RingBufferFull>
    where
        F: FnOnce(&mut T),
    {
        // Try to claim the next sequence from the sequencer
        let Some(sequence) = self.sequencer.try_next() else {
            return Err(RingBufferFull);
        };

        // Get the event at the claimed sequence and update it
        // SAFETY: We have exclusive access to this sequence from the sequencer
        let event = unsafe { &mut *self.ring_buffer.get_mut_unchecked(sequence) };
        update(event);

        // Publish the sequence to make it available to consumers
        self.sequencer.publish(sequence);
        Ok(sequence)
    }

    fn try_batch_publish<'a, F>(
        &'a mut self,
        n: usize,
        update: F,
    ) -> std::result::Result<i64, MissingFreeSlots>
    where
        T: 'a,
        F: FnOnce(BatchIterMut<'a, T>),
    {
        // Try to claim n sequences from the sequencer
        let batch_size = i64::try_from(n).expect("batch size must fit in i64");
        let Some(end_sequence) = self.sequencer.try_next_n(batch_size) else {
            let missing = n; // We don't know exactly how many are missing
            return Err(MissingFreeSlots(
                u64::try_from(missing).expect("missing slots must fit in u64"),
            ));
        };

        let start_sequence = end_sequence - (batch_size - 1);

        // SAFETY: We have exclusive access to this sequence range from the sequencer
        let iter = unsafe {
            self.ring_buffer
                .batch_iter_mut(start_sequence, end_sequence)
        };
        update(iter);

        // Publish the entire range to make it available to consumers
        self.sequencer.publish_range(start_sequence, end_sequence);
        Ok(end_sequence)
    }

    fn publish<F>(&mut self, update: F)
    where
        F: FnOnce(&mut T),
    {
        // Claim the next sequence from the sequencer (blocking)
        let sequence = match self.sequencer.next() {
            Ok(seq) => seq,
            Err(e) => {
                crate::internal_error!("Failed to claim sequence: {e:?}");
                return;
            }
        };

        // Get the event at the claimed sequence and update it
        // SAFETY: We have exclusive access to this sequence from the sequencer
        let event = unsafe { &mut *self.ring_buffer.get_mut_unchecked(sequence) };
        update(event);

        // Publish the sequence to make it available to consumers
        self.sequencer.publish(sequence);
    }

    fn batch_publish<'a, F>(&'a mut self, n: usize, update: F)
    where
        T: 'a,
        F: FnOnce(BatchIterMut<'a, T>),
    {
        // Claim n sequences from the sequencer (blocking)
        let batch_size = i64::try_from(n).expect("batch size must fit in i64");
        let end_sequence = match self.sequencer.next_n(batch_size) {
            Ok(seq) => seq,
            Err(e) => {
                crate::internal_error!("Failed to claim batch sequences: {e:?}");
                return;
            }
        };
        let start_sequence = end_sequence - (batch_size - 1);

        // SAFETY: We have exclusive access to this sequence range from the sequencer
        let iter = unsafe {
            self.ring_buffer
                .batch_iter_mut(start_sequence, end_sequence)
        };
        update(iter);

        // Publish the entire range to make it available to consumers
        self.sequencer.publish_range(start_sequence, end_sequence);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disruptor::{
        event_factory::DefaultEventFactory,
        sequencer::{MultiProducerSequencer, SingleProducerSequencer},
        wait_strategy::BusySpinWaitStrategy,
    };
    use std::sync::Arc;

    #[derive(Debug, Clone, PartialEq)]
    struct TestEvent {
        value: i64,
        data: String,
    }

    impl Default for TestEvent {
        fn default() -> Self {
            Self {
                value: -1,
                data: String::new(),
            }
        }
    }

    fn create_test_producer() -> SimpleProducer<TestEvent> {
        let factory = DefaultEventFactory::<TestEvent>::new();
        let ring_buffer =
            Arc::new(RingBuffer::new(8, factory).expect("Failed to create ring buffer"));
        let sequencer = Arc::new(SingleProducerSequencer::new(
            8,
            Arc::new(BusySpinWaitStrategy),
        ));
        SimpleProducer::new(ring_buffer, sequencer)
    }

    fn create_multi_producer_test_producer() -> SimpleProducer<TestEvent> {
        let factory = DefaultEventFactory::<TestEvent>::new();
        let ring_buffer =
            Arc::new(RingBuffer::new(16, factory).expect("Failed to create ring buffer"));
        let sequencer = Arc::new(MultiProducerSequencer::new(
            16,
            Arc::new(BusySpinWaitStrategy),
        ));
        SimpleProducer::new(ring_buffer, sequencer)
    }

    #[test]
    fn test_simple_producer_creation() {
        let producer = create_test_producer();

        // Should start with sequence -1 (no events published yet)
        assert_eq!(producer.current_sequence(), -1);
    }

    #[test]
    fn test_try_publish_success() {
        let mut producer = create_test_producer();

        let result = producer.try_publish(|event| {
            event.value = 42;
            event.data = "test".to_string();
        });

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0); // First sequence should be 0
        assert_eq!(producer.current_sequence(), 0);
    }

    #[test]
    fn test_try_publish_multiple() {
        let mut producer = create_test_producer();

        // Publish multiple events
        for i in 0..5 {
            let result = producer.try_publish(|event| {
                event.value = i;
                event.data = format!("event_{i}");
            });

            assert!(result.is_ok());
            assert_eq!(result.unwrap(), i);
        }

        assert_eq!(producer.current_sequence(), 4);
    }

    #[test]
    fn test_publish_blocking() {
        let mut producer = create_test_producer();

        // Publish an event (should not block for first few events)
        producer.publish(|event| {
            event.value = 100;
            event.data = "blocking_test".to_string();
        });

        assert_eq!(producer.current_sequence(), 0);
    }

    #[test]
    fn test_try_batch_publish_success() {
        let mut producer = create_test_producer();

        let result = producer.try_batch_publish(3, |iter| {
            for (i, event) in iter.enumerate() {
                event.value = i64::try_from(i).expect("index fits in i64");
                event.data = format!("batch_{i}");
            }
        });

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 2); // End sequence should be 2 (0, 1, 2)
        assert_eq!(producer.current_sequence(), 2);
    }

    #[test]
    fn test_try_batch_publish_zero_size() {
        let mut producer = create_test_producer();

        let result = producer.try_batch_publish(0, |iter| {
            // Iterator should be empty for zero-size batch
            assert_eq!(iter.count(), 0);
        });

        // Zero-size batch behavior depends on sequencer implementation
        // Some sequencers may return an error for zero-size requests
        if result.is_err() {
            // This is acceptable behavior for zero-size batches
            assert!(matches!(result, Err(MissingFreeSlots(_))));
        } else {
            // If it succeeds, sequence should not advance
            assert_eq!(producer.current_sequence(), -1);
        }
    }

    #[test]
    fn test_batch_publish_blocking() {
        let mut producer = create_test_producer();

        // Publish a batch (should not block for first batch)
        producer.batch_publish(2, |iter| {
            for (i, event) in iter.enumerate() {
                event.value = i64::try_from(i + 10).expect("index fits in i64");
                event.data = format!("blocking_batch_{i}");
            }
        });

        assert_eq!(producer.current_sequence(), 1); // End sequence for batch of 2
    }

    #[test]
    fn test_producer_with_multi_producer_sequencer() {
        let mut producer = create_multi_producer_test_producer();

        // Should work the same way with multi-producer sequencer
        let result = producer.try_publish(|event| {
            event.value = 999;
            event.data = "multi_producer_test".to_string();
        });

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);
        assert_eq!(producer.current_sequence(), 0);
    }

    #[test]
    fn test_error_types() {
        // Test RingBufferFull error
        let ring_buffer_full = RingBufferFull;
        assert_eq!(ring_buffer_full.to_string(), "Ring Buffer is full");

        // Test MissingFreeSlots error
        let missing_slots = MissingFreeSlots(5);
        assert_eq!(
            missing_slots.to_string(),
            "Missing free slots in Ring Buffer: 5"
        );
    }

    #[test]
    fn test_producer_trait_methods() {
        let mut producer = create_test_producer();

        // Test all Producer trait methods

        // try_publish - publishes to sequence 0
        let result1 = Producer::try_publish(&mut producer, |event| {
            event.value = 1;
        });
        assert!(result1.is_ok());
        assert_eq!(result1.unwrap(), 0);

        // publish - publishes to sequence 1
        Producer::publish(&mut producer, |event| {
            event.value = 2;
        });
        assert_eq!(producer.current_sequence(), 1);

        // try_batch_publish - publishes 2 events to sequences 2, 3
        let result2 = Producer::try_batch_publish(&mut producer, 2, |iter| {
            for (i, event) in iter.enumerate() {
                event.value = i64::try_from(i + 10).expect("index fits in i64");
            }
        });
        assert!(result2.is_ok());
        assert_eq!(result2.unwrap(), 3); // End sequence of batch

        // batch_publish - publishes 1 event to sequence 4
        Producer::batch_publish(&mut producer, 1, |iter| {
            for event in iter {
                event.value = 99;
            }
        });

        // Should have published 5 events total (1 + 1 + 2 + 1) with final sequence 4
        assert_eq!(producer.current_sequence(), 4);
    }

    #[test]
    fn test_concurrent_access_safety() {
        // This test ensures that the producer can be safely used
        // even when the underlying structures are shared
        let factory = DefaultEventFactory::<TestEvent>::new();
        let ring_buffer =
            Arc::new(RingBuffer::new(64, factory).expect("Failed to create ring buffer"));
        let sequencer = Arc::new(MultiProducerSequencer::new(
            64,
            Arc::new(BusySpinWaitStrategy),
        ));

        let mut producer1 = SimpleProducer::new(ring_buffer.clone(), sequencer.clone());
        let mut producer2 = SimpleProducer::new(ring_buffer.clone(), sequencer.clone());

        // Both producers should be able to publish
        let result1 = producer1.try_publish(|event| event.value = 1);
        let result2 = producer2.try_publish(|event| event.value = 2);

        assert!(result1.is_ok());
        assert!(result2.is_ok());

        // Sequences should be different
        assert_ne!(result1.unwrap(), result2.unwrap());
    }

    #[test]
    fn test_large_batch_publish() {
        let factory = DefaultEventFactory::<TestEvent>::new();
        let ring_buffer =
            Arc::new(RingBuffer::new(1024, factory).expect("Failed to create ring buffer"));
        let sequencer = Arc::new(SingleProducerSequencer::new(
            1024,
            Arc::new(BusySpinWaitStrategy),
        ));
        let mut producer = SimpleProducer::new(ring_buffer, sequencer);

        // Test large batch
        let batch_size = 100;
        let result = producer.try_batch_publish(batch_size, |iter| {
            for (i, event) in iter.enumerate() {
                event.value = i64::try_from(i).expect("index fits in i64");
                event.data = format!("large_batch_{i}");
            }
        });

        assert!(result.is_ok());
        let expected = i64::try_from(batch_size - 1).expect("batch size fits in i64");
        assert_eq!(result.unwrap(), expected);
        assert_eq!(producer.current_sequence(), expected);
    }

    #[test]
    fn test_producer_sequence_consistency() {
        let mut producer = create_test_producer();
        let mut expected_sequence = 0i64;

        // Mix single and batch publishes
        for i in 0_i64..5 {
            if i % 2 == 0 {
                // Single publish
                let result = producer.try_publish(|event| {
                    event.value = i;
                });
                assert_eq!(result.unwrap(), expected_sequence);
                expected_sequence += 1;
            } else {
                // Batch publish of 2
                let result = producer.try_batch_publish(2, |iter| {
                    for (j, event) in iter.enumerate() {
                        event.value = i * 10 + i64::try_from(j).expect("index fits in i64");
                    }
                });
                expected_sequence += 1; // End sequence of batch
                assert_eq!(result.unwrap(), expected_sequence);
                expected_sequence += 1; // Prepare for next
            }
        }

        assert_eq!(producer.current_sequence(), expected_sequence - 1);
    }
}
