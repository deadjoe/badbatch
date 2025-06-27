//! Event Translator Implementation
//!
//! This module provides the EventTranslator traits and implementations for translating
//! data into events in the Disruptor pattern. This follows the exact interface design
//! from the original LMAX Disruptor EventTranslator family.

/// Base trait for translating data into events
///
/// This trait is used to populate events in the ring buffer with data.
/// It follows the exact LMAX Disruptor EventTranslator interface design.
///
/// # Type Parameters
/// * `T` - The event type to populate
///
/// # Examples
/// ```
/// use badbatch::disruptor::EventTranslator;
///
/// #[derive(Default)]
/// struct MyEvent {
///     value: i64,
///     sequence: i64,
/// }
///
/// struct MyEventTranslator {
///     data: i64,
/// }
///
/// impl EventTranslator<MyEvent> for MyEventTranslator {
///     fn translate_to(&self, event: &mut MyEvent, sequence: i64) {
///         event.value = self.data;
///         event.sequence = sequence;
///     }
/// }
/// ```
pub trait EventTranslator<T>: Send + Sync {
    /// Translate data into an event
    ///
    /// This method is called to populate an event with data. The event
    /// is already allocated in the ring buffer and just needs to be populated.
    ///
    /// # Arguments
    /// * `event` - The event to populate (mutable reference)
    /// * `sequence` - The sequence number of the event in the ring buffer
    fn translate_to(&self, event: &mut T, sequence: i64);
}

/// Event translator that takes one argument
///
/// This is used when you need to pass one piece of data to the translator.
///
/// # Type Parameters
/// * `T` - The event type to populate
/// * `A` - The type of the argument
pub trait EventTranslatorOneArg<T, A>: Send + Sync {
    /// Translate data into an event with one argument
    ///
    /// # Arguments
    /// * `event` - The event to populate (mutable reference)
    /// * `sequence` - The sequence number of the event in the ring buffer
    /// * `arg0` - The first argument
    fn translate_to(&self, event: &mut T, sequence: i64, arg0: A);
}

/// Event translator that takes two arguments
///
/// This is used when you need to pass two pieces of data to the translator.
///
/// # Type Parameters
/// * `T` - The event type to populate
/// * `A` - The type of the first argument
/// * `B` - The type of the second argument
pub trait EventTranslatorTwoArg<T, A, B>: Send + Sync {
    /// Translate data into an event with two arguments
    ///
    /// # Arguments
    /// * `event` - The event to populate (mutable reference)
    /// * `sequence` - The sequence number of the event in the ring buffer
    /// * `arg0` - The first argument
    /// * `arg1` - The second argument
    fn translate_to(&self, event: &mut T, sequence: i64, arg0: A, arg1: B);
}

/// Event translator that takes three arguments
///
/// This is used when you need to pass three pieces of data to the translator.
///
/// # Type Parameters
/// * `T` - The event type to populate
/// * `A` - The type of the first argument
/// * `B` - The type of the second argument
/// * `C` - The type of the third argument
pub trait EventTranslatorThreeArg<T, A, B, C>: Send + Sync {
    /// Translate data into an event with three arguments
    ///
    /// # Arguments
    /// * `event` - The event to populate (mutable reference)
    /// * `sequence` - The sequence number of the event in the ring buffer
    /// * `arg0` - The first argument
    /// * `arg1` - The second argument
    /// * `arg2` - The third argument
    fn translate_to(&self, event: &mut T, sequence: i64, arg0: A, arg1: B, arg2: C);
}

/// Closure-based event translator
///
/// This provides a convenient way to create event translators using closures.
///
/// # Type Parameters
/// * `T` - The event type
/// * `F` - The closure type
pub struct ClosureEventTranslator<T, F>
where
    F: Fn(&mut T, i64) + Send + Sync,
{
    translator_fn: F,
    _phantom: std::marker::PhantomData<T>,
}

impl<T, F> ClosureEventTranslator<T, F>
where
    F: Fn(&mut T, i64) + Send + Sync,
{
    /// Create a new closure-based event translator
    ///
    /// # Arguments
    /// * `translator_fn` - The closure that will populate events
    ///
    /// # Returns
    /// A new ClosureEventTranslator instance
    pub fn new(translator_fn: F) -> Self {
        Self {
            translator_fn,
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<T, F> EventTranslator<T> for ClosureEventTranslator<T, F>
where
    T: Send + Sync,
    F: Fn(&mut T, i64) + Send + Sync,
{
    fn translate_to(&self, event: &mut T, sequence: i64) {
        (self.translator_fn)(event, sequence);
    }
}

/// Closure-based event translator with one argument
///
/// # Type Parameters
/// * `T` - The event type
/// * `A` - The argument type
/// * `F` - The closure type
pub struct ClosureEventTranslatorOneArg<T, A, F>
where
    F: Fn(&mut T, i64, A) + Send + Sync,
{
    translator_fn: F,
    _phantom: std::marker::PhantomData<(T, A)>,
}

impl<T, A, F> ClosureEventTranslatorOneArg<T, A, F>
where
    F: Fn(&mut T, i64, A) + Send + Sync,
{
    /// Create a new closure-based event translator with one argument
    pub fn new(translator_fn: F) -> Self {
        Self {
            translator_fn,
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<T, A, F> EventTranslatorOneArg<T, A> for ClosureEventTranslatorOneArg<T, A, F>
where
    T: Send + Sync,
    A: Send + Sync,
    F: Fn(&mut T, i64, A) + Send + Sync,
{
    fn translate_to(&self, event: &mut T, sequence: i64, arg0: A) {
        (self.translator_fn)(event, sequence, arg0);
    }
}

/// Closure-based event translator with two arguments
///
/// # Type Parameters
/// * `T` - The event type
/// * `A` - The first argument type
/// * `B` - The second argument type
/// * `F` - The closure type
pub struct ClosureEventTranslatorTwoArg<T, A, B, F>
where
    F: Fn(&mut T, i64, A, B) + Send + Sync,
{
    translator_fn: F,
    _phantom: std::marker::PhantomData<(T, A, B)>,
}

impl<T, A, B, F> ClosureEventTranslatorTwoArg<T, A, B, F>
where
    F: Fn(&mut T, i64, A, B) + Send + Sync,
{
    /// Create a new closure-based event translator with two arguments
    pub fn new(translator_fn: F) -> Self {
        Self {
            translator_fn,
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<T, A, B, F> EventTranslatorTwoArg<T, A, B> for ClosureEventTranslatorTwoArg<T, A, B, F>
where
    T: Send + Sync,
    A: Send + Sync,
    B: Send + Sync,
    F: Fn(&mut T, i64, A, B) + Send + Sync,
{
    fn translate_to(&self, event: &mut T, sequence: i64, arg0: A, arg1: B) {
        (self.translator_fn)(event, sequence, arg0, arg1);
    }
}

/// Convenience functions for creating event translators
///
/// Create an event translator from a closure
pub fn event_translator<T, F>(translator_fn: F) -> ClosureEventTranslator<T, F>
where
    F: Fn(&mut T, i64) + Send + Sync,
{
    ClosureEventTranslator::new(translator_fn)
}

/// Create an event translator with one argument from a closure
pub fn event_translator_one_arg<T, A, F>(translator_fn: F) -> ClosureEventTranslatorOneArg<T, A, F>
where
    F: Fn(&mut T, i64, A) + Send + Sync,
{
    ClosureEventTranslatorOneArg::new(translator_fn)
}

/// Create an event translator with two arguments from a closure
pub fn event_translator_two_arg<T, A, B, F>(
    translator_fn: F,
) -> ClosureEventTranslatorTwoArg<T, A, B, F>
where
    F: Fn(&mut T, i64, A, B) + Send + Sync,
{
    ClosureEventTranslatorTwoArg::new(translator_fn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Default, Clone, PartialEq)]
    struct TestEvent {
        value: i64,
        name: String,
        count: u32,
    }

    #[test]
    fn test_closure_event_translator() {
        let translator = ClosureEventTranslator::new(|event: &mut TestEvent, sequence| {
            event.value = sequence;
            event.name = format!("event_{sequence}");
        });

        let mut event = TestEvent::default();
        translator.translate_to(&mut event, 42);

        assert_eq!(event.value, 42);
        assert_eq!(event.name, "event_42");
    }

    #[test]
    fn test_closure_event_translator_one_arg() {
        let translator =
            ClosureEventTranslatorOneArg::new(|event: &mut TestEvent, sequence, name: String| {
                event.value = sequence;
                event.name = name;
            });

        let mut event = TestEvent::default();
        translator.translate_to(&mut event, 42, "test_name".to_string());

        assert_eq!(event.value, 42);
        assert_eq!(event.name, "test_name");
    }

    #[test]
    fn test_closure_event_translator_two_arg() {
        let translator = ClosureEventTranslatorTwoArg::new(
            |event: &mut TestEvent, sequence, name: String, count: u32| {
                event.value = sequence;
                event.name = name;
                event.count = count;
            },
        );

        let mut event = TestEvent::default();
        translator.translate_to(&mut event, 42, "test_name".to_string(), 100);

        assert_eq!(event.value, 42);
        assert_eq!(event.name, "test_name");
        assert_eq!(event.count, 100);
    }

    #[test]
    fn test_convenience_functions() {
        let translator = event_translator(|event: &mut TestEvent, sequence| {
            event.value = sequence * 2;
        });

        let mut event = TestEvent::default();
        translator.translate_to(&mut event, 21);
        assert_eq!(event.value, 42);

        let translator_one =
            event_translator_one_arg(|event: &mut TestEvent, sequence, multiplier: i64| {
                event.value = sequence * multiplier;
            });

        let mut event = TestEvent::default();
        translator_one.translate_to(&mut event, 21, 3);
        assert_eq!(event.value, 63);
    }
}
