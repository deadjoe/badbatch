//! Exception Handler Implementation
//!
//! This module provides exception handling for the Disruptor pattern.
//! Exception handlers are called when event processing fails, allowing
//! for custom error handling and recovery strategies.

use crate::disruptor::DisruptorError;
use std::fmt::Debug;

/// Handler for exceptions that occur during event processing
///
/// This trait allows custom handling of exceptions that occur during
/// event processing. It follows the exact design from the original
/// LMAX Disruptor ExceptionHandler interface.
///
/// # Type Parameters
/// * `T` - The event type being processed
pub trait ExceptionHandler<T>: Send + Sync {
    /// Handle an exception that occurred during event processing
    ///
    /// This method is called when an event handler throws an exception.
    /// The implementation can decide how to handle the error - log it,
    /// retry processing, or take other recovery actions.
    ///
    /// # Arguments
    /// * `error` - The error that occurred
    /// * `sequence` - The sequence number of the event that caused the error
    /// * `event` - The event that was being processed when the error occurred
    fn handle_event_exception(&self, error: DisruptorError, sequence: i64, event: &T);

    /// Handle an exception that occurred during the event processing loop startup
    ///
    /// This method is called when an exception occurs during the startup
    /// of an event processor.
    ///
    /// # Arguments
    /// * `error` - The error that occurred during startup
    fn handle_on_start_exception(&self, error: DisruptorError);

    /// Handle an exception that occurred during the event processing loop shutdown
    ///
    /// This method is called when an exception occurs during the shutdown
    /// of an event processor.
    ///
    /// # Arguments
    /// * `error` - The error that occurred during shutdown
    fn handle_on_shutdown_exception(&self, error: DisruptorError);
}

/// Default exception handler that logs errors
///
/// This is a simple exception handler that logs all exceptions.
/// It's suitable for development and testing, but production systems
/// may want to implement more sophisticated error handling.
///
/// # Type Parameters
/// * `T` - The event type being processed
#[derive(Debug, Default)]
pub struct DefaultExceptionHandler<T> {
    _phantom: std::marker::PhantomData<T>,
}

impl<T> DefaultExceptionHandler<T> {
    /// Create a new default exception handler
    pub fn new() -> Self {
        Self {
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<T> ExceptionHandler<T> for DefaultExceptionHandler<T>
where
    T: Debug + Send + Sync,
{
    fn handle_event_exception(&self, error: DisruptorError, sequence: i64, event: &T) {
        eprintln!("Exception processing event at sequence {sequence}: {error:?}. Event: {event:?}");
    }

    fn handle_on_start_exception(&self, error: DisruptorError) {
        eprintln!("Exception during event processor startup: {error:?}");
    }

    fn handle_on_shutdown_exception(&self, error: DisruptorError) {
        eprintln!("Exception during event processor shutdown: {error:?}");
    }
}

/// Exception handler that ignores all exceptions
///
/// This handler silently ignores all exceptions. Use with caution,
/// as it can make debugging difficult. It's mainly useful for
/// performance testing where you want to measure overhead without
/// error handling.
///
/// # Type Parameters
/// * `T` - The event type being processed
#[derive(Debug, Default)]
pub struct IgnoreExceptionHandler<T> {
    _phantom: std::marker::PhantomData<T>,
}

impl<T> IgnoreExceptionHandler<T> {
    /// Create a new ignore exception handler
    pub fn new() -> Self {
        Self {
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<T> ExceptionHandler<T> for IgnoreExceptionHandler<T>
where
    T: Send + Sync,
{
    fn handle_event_exception(&self, _error: DisruptorError, _sequence: i64, _event: &T) {
        // Ignore the exception
    }

    fn handle_on_start_exception(&self, _error: DisruptorError) {
        // Ignore the exception
    }

    fn handle_on_shutdown_exception(&self, _error: DisruptorError) {
        // Ignore the exception
    }
}

/// Exception handler that panics on any exception
///
/// This handler panics when any exception occurs. It's useful for
/// development and testing when you want to fail fast on any error.
///
/// # Type Parameters
/// * `T` - The event type being processed
#[derive(Debug, Default)]
pub struct PanicExceptionHandler<T> {
    _phantom: std::marker::PhantomData<T>,
}

impl<T> PanicExceptionHandler<T> {
    /// Create a new panic exception handler
    pub fn new() -> Self {
        Self {
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<T> ExceptionHandler<T> for PanicExceptionHandler<T>
where
    T: Debug + Send + Sync,
{
    fn handle_event_exception(&self, error: DisruptorError, sequence: i64, event: &T) {
        panic!("Exception processing event at sequence {sequence}: {error:?}. Event: {event:?}");
    }

    fn handle_on_start_exception(&self, error: DisruptorError) {
        panic!("Exception during event processor startup: {error:?}");
    }

    fn handle_on_shutdown_exception(&self, error: DisruptorError) {
        panic!("Exception during event processor shutdown: {error:?}");
    }
}

/// Closure-based exception handler
///
/// This provides a flexible way to create exception handlers using closures.
///
/// # Type Parameters
/// * `T` - The event type
/// * `F` - The closure type for event exceptions
/// * `S` - The closure type for startup exceptions
/// * `H` - The closure type for shutdown exceptions
pub struct ClosureExceptionHandler<T, F, S, H>
where
    F: Fn(DisruptorError, i64, &T) + Send + Sync,
    S: Fn(DisruptorError) + Send + Sync,
    H: Fn(DisruptorError) + Send + Sync,
{
    event_handler: F,
    start_handler: S,
    shutdown_handler: H,
    _phantom: std::marker::PhantomData<T>,
}

impl<T, F, S, H> ClosureExceptionHandler<T, F, S, H>
where
    F: Fn(DisruptorError, i64, &T) + Send + Sync,
    S: Fn(DisruptorError) + Send + Sync,
    H: Fn(DisruptorError) + Send + Sync,
{
    /// Create a new closure-based exception handler
    ///
    /// # Arguments
    /// * `event_handler` - Closure for handling event processing exceptions
    /// * `start_handler` - Closure for handling startup exceptions
    /// * `shutdown_handler` - Closure for handling shutdown exceptions
    pub fn new(event_handler: F, start_handler: S, shutdown_handler: H) -> Self {
        Self {
            event_handler,
            start_handler,
            shutdown_handler,
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<T, F, S, H> ExceptionHandler<T> for ClosureExceptionHandler<T, F, S, H>
where
    T: Send + Sync,
    F: Fn(DisruptorError, i64, &T) + Send + Sync,
    S: Fn(DisruptorError) + Send + Sync,
    H: Fn(DisruptorError) + Send + Sync,
{
    fn handle_event_exception(&self, error: DisruptorError, sequence: i64, event: &T) {
        (self.event_handler)(error, sequence, event);
    }

    fn handle_on_start_exception(&self, error: DisruptorError) {
        (self.start_handler)(error);
    }

    fn handle_on_shutdown_exception(&self, error: DisruptorError) {
        (self.shutdown_handler)(error);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone)]
    #[allow(dead_code)]
    struct TestEvent {
        value: i64,
    }

    #[test]
    fn test_default_exception_handler() {
        let handler = DefaultExceptionHandler::<TestEvent>::new();
        let event = TestEvent { value: 42 };

        // These should not panic, just log to stderr
        handler.handle_event_exception(DisruptorError::BufferFull, 1, &event);
        handler.handle_on_start_exception(DisruptorError::Shutdown);
        handler.handle_on_shutdown_exception(DisruptorError::Timeout);
    }

    #[test]
    fn test_ignore_exception_handler() {
        let handler = IgnoreExceptionHandler::<TestEvent>::new();
        let event = TestEvent { value: 42 };

        // These should do nothing
        handler.handle_event_exception(DisruptorError::BufferFull, 1, &event);
        handler.handle_on_start_exception(DisruptorError::Shutdown);
        handler.handle_on_shutdown_exception(DisruptorError::Timeout);
    }

    #[test]
    fn test_closure_exception_handler() {
        let _event_called = false;
        let _start_called = false;
        let _shutdown_called = false;

        let handler = ClosureExceptionHandler::new(
            |_error, _sequence, _event| {
                // In a real test, we'd use a shared state mechanism
                // For now, just verify the closure can be called
            },
            |_error| {
                // Start handler
            },
            |_error| {
                // Shutdown handler
            },
        );

        let event = TestEvent { value: 42 };
        handler.handle_event_exception(DisruptorError::BufferFull, 1, &event);
        handler.handle_on_start_exception(DisruptorError::Shutdown);
        handler.handle_on_shutdown_exception(DisruptorError::Timeout);
    }
}
