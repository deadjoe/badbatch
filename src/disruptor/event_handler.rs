//! Event Handler Implementation
//!
//! This module provides the EventHandler trait and related functionality for processing
//! events in the Disruptor pattern. This follows the exact interface design from the
//! original LMAX Disruptor EventHandler interface.

use crate::disruptor::{Result, Sequence};
use std::sync::Arc;

/// Handler for processing events from the Disruptor
///
/// This trait must be implemented by consumers to process events.
/// It follows the exact LMAX Disruptor EventHandler interface design.
///
/// # Type Parameters
/// * `T` - The event type that will be processed
pub trait EventHandler<T>: Send + Sync {
    /// Process an event
    ///
    /// This method is called for each event that needs to be processed.
    /// The implementation should process the event and return quickly to
    /// maintain high throughput.
    ///
    /// # Arguments
    /// * `event` - The event to process (mutable reference for in-place updates)
    /// * `sequence` - The sequence number of the event in the ring buffer
    /// * `end_of_batch` - True if this is the last event in the current batch
    ///
    /// # Returns
    /// Result indicating success or failure of event processing
    ///
    /// # Examples
    /// ```
    /// use badbatch::disruptor::{EventHandler, Result};
    ///
    /// #[derive(Default)]
    /// struct MyEvent {
    ///     data: i32,
    /// }
    ///
    /// struct MyEventHandler;
    ///
    /// impl EventHandler<MyEvent> for MyEventHandler {
    ///     fn on_event(&mut self, event: &mut MyEvent, sequence: i64, end_of_batch: bool) -> Result<()> {
    ///         // Process the event
    ///         println!("Processing event {} at sequence {}", event.data, sequence);
    ///         Ok(())
    ///     }
    /// }
    /// ```
    fn on_event(&mut self, event: &mut T, sequence: i64, end_of_batch: bool) -> Result<()>;

    /// Set the sequence callback for early release (optional)
    ///
    /// This allows the handler to signal completion before the end of a batch,
    /// enabling downstream consumers to start processing earlier. This is an
    /// advanced feature for optimizing latency in complex dependency graphs.
    ///
    /// # Arguments
    /// * `sequence_callback` - The sequence to update when processing is complete
    fn set_sequence_callback(&mut self, _sequence_callback: Arc<Sequence>) {
        // Default implementation does nothing - this is optional
    }
}

/// A simple event handler that can be created from a closure
///
/// This provides a convenient way to create event handlers without defining
/// a new struct and implementing the trait manually.
///
/// # Type Parameters
/// * `T` - The event type
/// * `F` - The closure type
pub struct ClosureEventHandler<T, F>
where
    F: FnMut(&mut T, i64, bool) -> Result<()> + Send + Sync,
{
    handler: F,
    sequence_callback: Option<Arc<Sequence>>,
    _phantom: std::marker::PhantomData<T>,
}

impl<T, F> ClosureEventHandler<T, F>
where
    F: FnMut(&mut T, i64, bool) -> Result<()> + Send + Sync,
{
    /// Create a new closure-based event handler
    ///
    /// # Arguments
    /// * `handler` - The closure that will process events
    ///
    /// # Returns
    /// A new ClosureEventHandler instance
    pub fn new(handler: F) -> Self {
        Self {
            handler,
            sequence_callback: None,
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<T, F> EventHandler<T> for ClosureEventHandler<T, F>
where
    T: Send + Sync,
    F: FnMut(&mut T, i64, bool) -> Result<()> + Send + Sync,
{
    fn on_event(&mut self, event: &mut T, sequence: i64, end_of_batch: bool) -> Result<()> {
        (self.handler)(event, sequence, end_of_batch)
    }

    fn set_sequence_callback(&mut self, sequence_callback: Arc<Sequence>) {
        self.sequence_callback = Some(sequence_callback);
    }
}

/// A no-op event handler for testing and benchmarking
///
/// This handler does nothing with events, which is useful for measuring
/// the overhead of the Disruptor framework itself.
///
/// # Type Parameters
/// * `T` - The event type
pub struct NoOpEventHandler<T> {
    _phantom: std::marker::PhantomData<T>,
}

impl<T> NoOpEventHandler<T> {
    /// Create a new no-op event handler
    pub fn new() -> Self {
        Self {
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<T> Default for NoOpEventHandler<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> EventHandler<T> for NoOpEventHandler<T>
where
    T: Send + Sync,
{
    fn on_event(&mut self, _event: &mut T, _sequence: i64, _end_of_batch: bool) -> Result<()> {
        // Do nothing - this is a no-op handler
        Ok(())
    }
}

/// Event handler that clears events after processing
///
/// This is useful for preventing memory leaks when events contain heap-allocated
/// data. It wraps another event handler and clears the event after processing.
///
/// # Type Parameters
/// * `T` - The event type (must implement Default for clearing)
/// * `H` - The inner event handler type
pub struct ClearingEventHandler<T, H>
where
    T: Default,
    H: EventHandler<T>,
{
    inner_handler: H,
    _phantom: std::marker::PhantomData<T>,
}

impl<T, H> ClearingEventHandler<T, H>
where
    T: Default,
    H: EventHandler<T>,
{
    /// Create a new clearing event handler
    ///
    /// # Arguments
    /// * `inner_handler` - The handler that will process events before clearing
    ///
    /// # Returns
    /// A new ClearingEventHandler instance
    pub fn new(inner_handler: H) -> Self {
        Self {
            inner_handler,
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<T, H> EventHandler<T> for ClearingEventHandler<T, H>
where
    T: Default + Send + Sync,
    H: EventHandler<T>,
{
    fn on_event(&mut self, event: &mut T, sequence: i64, end_of_batch: bool) -> Result<()> {
        // Process the event first
        let result = self.inner_handler.on_event(event, sequence, end_of_batch);

        // Clear the event after processing (regardless of result)
        *event = T::default();

        result
    }

    fn set_sequence_callback(&mut self, sequence_callback: Arc<Sequence>) {
        self.inner_handler.set_sequence_callback(sequence_callback);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Default, Clone, PartialEq)]
    struct TestEvent {
        value: i64,
        processed: bool,
    }

    #[test]
    fn test_closure_event_handler() {
        let mut handler = ClosureEventHandler::new(|event: &mut TestEvent, sequence, _end_of_batch| {
            event.value = sequence;
            event.processed = true;
            Ok(())
        });

        let mut event = TestEvent::default();
        handler.on_event(&mut event, 42, false).unwrap();

        assert_eq!(event.value, 42);
        assert!(event.processed);
    }

    #[test]
    fn test_no_op_event_handler() {
        let mut handler = NoOpEventHandler::<TestEvent>::new();
        let mut event = TestEvent { value: 123, processed: false };

        handler.on_event(&mut event, 42, false).unwrap();

        // Event should remain unchanged
        assert_eq!(event.value, 123);
        assert!(!event.processed);
    }

    #[test]
    fn test_clearing_event_handler() {
        let inner_handler = ClosureEventHandler::new(|event: &mut TestEvent, sequence, _end_of_batch| {
            event.value = sequence;
            event.processed = true;
            Ok(())
        });

        let mut handler = ClearingEventHandler::new(inner_handler);
        let mut event = TestEvent { value: 999, processed: false };

        handler.on_event(&mut event, 42, false).unwrap();

        // Event should be cleared (default values)
        assert_eq!(event.value, 0);
        assert!(!event.processed);
    }

    #[test]
    fn test_sequence_callback() {
        let mut handler = ClosureEventHandler::new(|_event: &mut TestEvent, _sequence, _end_of_batch| {
            Ok(())
        });

        let sequence = Arc::new(Sequence::new(0));
        handler.set_sequence_callback(Arc::clone(&sequence));

        // Verify the callback was set (we can't directly test it without more complex setup)
        assert!(handler.sequence_callback.is_some());
    }
}
