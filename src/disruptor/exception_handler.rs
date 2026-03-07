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
        crate::internal_error!(
            "Exception processing event at sequence {sequence}: {error:?}. Event: {event:?}"
        );
    }

    fn handle_on_start_exception(&self, error: DisruptorError) {
        crate::internal_error!("Exception during event processor startup: {error:?}");
    }

    fn handle_on_shutdown_exception(&self, error: DisruptorError) {
        crate::internal_error!("Exception during event processor shutdown: {error:?}");
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
    use std::sync::{Arc, Mutex};

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
        #[derive(Debug, Default)]
        struct Recorded {
            event: Mutex<Vec<(String, i64, i64)>>,
            start: Mutex<Vec<String>>,
            shutdown: Mutex<Vec<String>>,
        }

        let calls = Arc::new(Recorded::default());
        let event_calls = calls.clone();
        let start_calls = calls.clone();
        let shutdown_calls = calls.clone();
        let handler = ClosureExceptionHandler::new(
            move |error, sequence, event: &TestEvent| {
                event_calls
                    .event
                    .lock()
                    .unwrap()
                    .push((format!("{error:?}"), sequence, event.value));
            },
            move |error| {
                start_calls
                    .start
                    .lock()
                    .unwrap()
                    .push(format!("{error:?}"));
            },
            move |error| {
                shutdown_calls
                    .shutdown
                    .lock()
                    .unwrap()
                    .push(format!("{error:?}"));
            },
        );

        let event = TestEvent { value: 42 };
        handler.handle_event_exception(DisruptorError::BufferFull, 1, &event);
        handler.handle_on_start_exception(DisruptorError::Shutdown);
        handler.handle_on_shutdown_exception(DisruptorError::Timeout);

        assert_eq!(
            *calls.event.lock().unwrap(),
            vec![("BufferFull".to_string(), 1, 42)]
        );
        assert_eq!(*calls.start.lock().unwrap(), vec!["Shutdown".to_string()]);
        assert_eq!(
            *calls.shutdown.lock().unwrap(),
            vec!["Timeout".to_string()]
        );
    }

    #[test]
    #[should_panic(expected = "Exception processing event at sequence 7")]
    fn test_panic_exception_handler_panics_on_event_exception() {
        let handler = PanicExceptionHandler::<TestEvent>::new();
        let event = TestEvent { value: 99 };
        handler.handle_event_exception(DisruptorError::BufferFull, 7, &event);
    }

    #[test]
    #[should_panic(expected = "Exception during event processor startup")]
    fn test_panic_exception_handler_panics_on_start_exception() {
        let handler = PanicExceptionHandler::<TestEvent>::new();
        handler.handle_on_start_exception(DisruptorError::Shutdown);
    }

    #[test]
    #[should_panic(expected = "Exception during event processor shutdown")]
    fn test_panic_exception_handler_panics_on_shutdown_exception() {
        let handler = PanicExceptionHandler::<TestEvent>::new();
        handler.handle_on_shutdown_exception(DisruptorError::Timeout);
    }
}
