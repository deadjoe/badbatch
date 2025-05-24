//! Event Factory Implementation
//!
//! This module provides the EventFactory trait and implementations for creating events
//! in the Disruptor pattern. This follows the exact interface design from the original
//! LMAX Disruptor EventFactory for event pre-allocation.

/// Factory for creating events in the Disruptor
///
/// This trait is used to pre-allocate events in the ring buffer, following the
/// LMAX Disruptor pattern for zero-allocation event processing. The factory is
/// called once for each slot in the ring buffer during initialization.
///
/// # Type Parameters
/// * `T` - The event type to create
///
/// # Examples
/// ```
/// use badbatch::disruptor::EventFactory;
///
/// struct MyEvent {
///     data: i64,
/// }
///
/// struct MyEventFactory;
///
/// impl EventFactory<MyEvent> for MyEventFactory {
///     fn new_instance(&self) -> MyEvent {
///         MyEvent { data: 0 }
///     }
/// }
/// ```
pub trait EventFactory<T>: Send + Sync {
    /// Create a new event instance
    ///
    /// This method will be called once for each slot in the ring buffer during
    /// initialization to pre-allocate all events. The implementation should
    /// return a new instance of the event type in its initial state.
    ///
    /// # Returns
    /// A new event instance
    fn new_instance(&self) -> T;
}

/// Event factory that uses the Default trait
///
/// This is a convenient factory for events that implement Default.
/// It simply calls T::default() to create new instances.
///
/// # Type Parameters
/// * `T` - The event type (must implement Default)
pub struct DefaultEventFactory<T: Default> {
    _phantom: std::marker::PhantomData<T>,
}

impl<T: Default> DefaultEventFactory<T> {
    /// Create a new default event factory
    pub fn new() -> Self {
        Self {
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<T: Default> Default for DefaultEventFactory<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Default + Send + Sync> EventFactory<T> for DefaultEventFactory<T> {
    fn new_instance(&self) -> T {
        T::default()
    }
}

/// Event factory that uses a closure to create events
///
/// This provides a flexible way to create events using any closure
/// that returns the event type.
///
/// # Type Parameters
/// * `T` - The event type
/// * `F` - The closure type
pub struct ClosureEventFactory<T, F>
where
    F: Fn() -> T + Send + Sync,
{
    factory_fn: F,
    _phantom: std::marker::PhantomData<T>,
}

impl<T, F> ClosureEventFactory<T, F>
where
    F: Fn() -> T + Send + Sync,
{
    /// Create a new closure-based event factory
    ///
    /// # Arguments
    /// * `factory_fn` - The closure that creates new event instances
    ///
    /// # Returns
    /// A new ClosureEventFactory instance
    pub fn new(factory_fn: F) -> Self {
        Self {
            factory_fn,
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<T, F> EventFactory<T> for ClosureEventFactory<T, F>
where
    T: Send + Sync,
    F: Fn() -> T + Send + Sync,
{
    fn new_instance(&self) -> T {
        (self.factory_fn)()
    }
}

/// Event factory that clones a prototype event
///
/// This factory holds a prototype event and clones it for each new instance.
/// This is useful when you want all events to start with the same initial values.
///
/// # Type Parameters
/// * `T` - The event type (must implement Clone)
pub struct CloneEventFactory<T: Clone> {
    prototype: T,
}

impl<T: Clone> CloneEventFactory<T> {
    /// Create a new clone-based event factory
    ///
    /// # Arguments
    /// * `prototype` - The prototype event to clone
    ///
    /// # Returns
    /// A new CloneEventFactory instance
    pub fn new(prototype: T) -> Self {
        Self { prototype }
    }
}

impl<T: Clone + Send + Sync> EventFactory<T> for CloneEventFactory<T> {
    fn new_instance(&self) -> T {
        self.prototype.clone()
    }
}

/// Convenience function to create an event factory from a closure
///
/// This is a shorthand for creating a ClosureEventFactory.
///
/// # Arguments
/// * `factory_fn` - The closure that creates new event instances
///
/// # Returns
/// A new ClosureEventFactory instance
///
/// # Examples
/// ```
/// use badbatch::disruptor::event_factory;
///
/// let factory = event_factory(|| MyEvent { data: 42 });
/// ```
pub fn event_factory<T, F>(factory_fn: F) -> ClosureEventFactory<T, F>
where
    F: Fn() -> T + Send + Sync,
{
    ClosureEventFactory::new(factory_fn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Default, Clone, PartialEq)]
    struct TestEvent {
        value: i64,
        name: String,
    }

    impl TestEvent {
        fn new(value: i64, name: String) -> Self {
            Self { value, name }
        }
    }

    #[test]
    fn test_default_event_factory() {
        let factory = DefaultEventFactory::<TestEvent>::new();
        let event = factory.new_instance();
        assert_eq!(event, TestEvent::default());

        // Test multiple instances are independent
        let event1 = factory.new_instance();
        let event2 = factory.new_instance();
        assert_eq!(event1, event2);
    }

    #[test]
    fn test_closure_event_factory() {
        let factory = ClosureEventFactory::new(|| TestEvent::new(42, "test".to_string()));
        let event = factory.new_instance();
        assert_eq!(event.value, 42);
        assert_eq!(event.name, "test");

        // Test multiple instances
        let event1 = factory.new_instance();
        let event2 = factory.new_instance();
        assert_eq!(event1, event2);
    }

    #[test]
    fn test_clone_event_factory() {
        let prototype = TestEvent::new(100, "prototype".to_string());
        let factory = CloneEventFactory::new(prototype.clone());
        let event = factory.new_instance();
        assert_eq!(event, prototype);

        // Test multiple instances are independent
        let mut event1 = factory.new_instance();
        let event2 = factory.new_instance();

        event1.value = 999; // Modify one instance
        assert_ne!(event1, event2); // Should be different now
        assert_eq!(event2, prototype); // Original should be unchanged
    }

    #[test]
    fn test_event_factory_convenience_function() {
        let factory = event_factory(|| TestEvent::new(999, "convenience".to_string()));
        let event = factory.new_instance();
        assert_eq!(event.value, 999);
        assert_eq!(event.name, "convenience");
    }

    #[test]
    fn test_factory_thread_safety() {
        use std::sync::Arc;
        use std::thread;

        let factory = Arc::new(DefaultEventFactory::<TestEvent>::new());
        let mut handles = vec![];

        // Test that factory can be used from multiple threads
        for _ in 0..4 {
            let factory_clone = Arc::clone(&factory);
            let handle = thread::spawn(move || {
                for _ in 0..100 {
                    let _event = factory_clone.new_instance();
                }
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.join().unwrap();
        }
    }

    #[test]
    fn test_multiple_instances_are_independent() {
        let factory = ClosureEventFactory::new(|| TestEvent::new(0, "initial".to_string()));

        let mut event1 = factory.new_instance();
        let mut event2 = factory.new_instance();

        // Modify each instance differently
        event1.value = 1;
        event1.name = "first".to_string();

        event2.value = 2;
        event2.name = "second".to_string();

        // Verify they are independent
        assert_eq!(event1.value, 1);
        assert_eq!(event1.name, "first");
        assert_eq!(event2.value, 2);
        assert_eq!(event2.name, "second");
    }
}
