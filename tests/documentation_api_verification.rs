#![allow(
    missing_docs,
    clippy::all,
    clippy::pedantic,
    clippy::nursery,
    clippy::cargo
)]

//! Test file to verify that the API examples in README.md and DESIGN.md actually compile and work
//!
//! This test verifies that the documented API patterns work correctly with the current codebase.

use badbatch::disruptor::{
    BlockingWaitStrategy,
    DefaultEventFactory,
    // Traditional LMAX API imports from README lines 104-107
    Disruptor,
    EventHandler,
    EventTranslator,
    ProducerType,
};

// Test 1: Traditional LMAX Disruptor API (README lines 96-161)
#[cfg(test)]
mod traditional_lmax_api_tests {
    use super::*;

    #[derive(Debug, Default)]
    struct MyEvent {
        value: i64,
        message: String,
    }

    // Event handler implementation
    struct MyEventHandler;

    impl EventHandler<MyEvent> for MyEventHandler {
        fn on_event(
            &mut self,
            event: &mut MyEvent,
            sequence: i64,
            end_of_batch: bool,
        ) -> badbatch::disruptor::Result<()> {
            badbatch::test_log!(
                "Processing event {} with value {} (end_of_batch: {})",
                sequence,
                event.value,
                end_of_batch
            );
            Ok(())
        }
    }

    // Event translator implementation
    struct MyEventTranslator {
        value: i64,
        message: String,
    }

    impl EventTranslator<MyEvent> for MyEventTranslator {
        fn translate_to(&self, event: &mut MyEvent, _sequence: i64) {
            event.value = self.value;
            event.message = self.message.clone();
        }
    }

    #[test]
    fn test_traditional_lmax_api_compiles() {
        // Test that the traditional LMAX API pattern compiles
        let factory = DefaultEventFactory::<MyEvent>::new();
        let mut disruptor = Disruptor::new(
            factory,
            1024, // Buffer size (must be power of 2)
            ProducerType::Single,
            Box::new(BlockingWaitStrategy::new()),
        )
        .unwrap()
        .handle_events_with(MyEventHandler)
        .build();

        // Start the Disruptor
        disruptor.start().unwrap();

        // Publish events using EventTranslator
        let translator = MyEventTranslator {
            value: 42,
            message: "Hello, World!".to_string(),
        };
        disruptor.publish_event(translator).unwrap();

        // Shutdown when done
        disruptor.shutdown().unwrap();
    }
}

// Test 2: Modern disruptor-rs Inspired API (README lines 165-212)
#[cfg(test)]
mod modern_disruptor_rs_api_tests {
    use badbatch::disruptor::{
        // Imports from README lines 166-169
        build_single_producer,
        event_factory::ClosureEventFactory,
        simple_wait_strategy::BusySpin,
        BusySpinWaitStrategy,
        ElegantConsumer,
        RingBuffer,
    };
    use std::sync::Arc;

    #[derive(Debug, Default)]
    struct MyEvent {
        value: i64,
    }

    #[test]
    fn test_modern_api_build_single_producer() {
        // Test build_single_producer function from README line 178
        // The wait strategy needs to implement WaitStrategy, so we use BusySpinWaitStrategy
        let mut producer = build_single_producer(1024, MyEvent::default, BusySpinWaitStrategy)
            .handle_events_with(|event, _sequence, _end_of_batch| {
                badbatch::test_log!("Processing event with value {}", event.value);
            })
            .build();

        // Test closure-based publishing (README line 186-188)
        producer.publish(|event| {
            event.value = 42;
        });

        // Test batch publishing (README lines 191-195)
        producer.batch_publish(5, |batch| {
            for (i, event) in batch.enumerate() {
                event.value = i as i64;
            }
        });
    }

    #[test]
    fn test_elegant_consumer_api() {
        // Test ElegantConsumer from README lines 197-211
        let factory = ClosureEventFactory::new(MyEvent::default);
        let ring_buffer = Arc::new(RingBuffer::new(1024, factory).unwrap());

        let consumer = ElegantConsumer::with_affinity(
            ring_buffer,
            |event, _sequence, _end_of_batch| {
                badbatch::test_log!("Processing: {}", event.value);
            },
            BusySpin,
            1, // Pin to CPU core 1
        )
        .unwrap();

        // Graceful shutdown
        consumer.shutdown().unwrap();
    }
}

// Test 3: Verify DESIGN.md code snippets
#[cfg(test)]
mod design_md_verification_tests {
    use badbatch::disruptor::{
        event_factory::DefaultEventFactory, MultiProducerSequencer, RingBuffer, Sequence,
        SingleProducerSequencer,
    };
    use std::sync::Arc;

    #[test]
    fn test_design_md_structs_exist() {
        // Verify that the structures mentioned in DESIGN.md actually exist

        // Test Sequence struct (DESIGN.md lines 302-306)
        let _sequence = Sequence::new_with_initial_value();

        // Test RingBuffer (DESIGN.md lines 50-54)
        let factory = DefaultEventFactory::<i64>::new();
        let _ring_buffer = RingBuffer::new(1024, factory).unwrap();

        // Test Sequencer types (DESIGN.md lines 74-82, 94-103)
        let _single_sequencer =
            SingleProducerSequencer::new(1024, Arc::new(badbatch::disruptor::BusySpinWaitStrategy));
        let _multi_sequencer =
            MultiProducerSequencer::new(1024, Arc::new(badbatch::disruptor::BusySpinWaitStrategy));
    }

    #[test]
    fn test_design_md_producer_api() {
        // Test Producer API from DESIGN.md lines 117-123
        use badbatch::disruptor::{Producer, SimpleProducer};

        let factory = DefaultEventFactory::<i64>::new();
        let ring_buffer = Arc::new(RingBuffer::new(1024, factory).unwrap());
        let sequencer = Arc::new(SingleProducerSequencer::new(
            1024,
            Arc::new(badbatch::disruptor::BusySpinWaitStrategy),
        ));

        let mut producer = SimpleProducer::new(ring_buffer, sequencer);

        // Test the Producer trait methods
        producer.publish(|event| {
            *event = 42;
        });

        producer.batch_publish(3, |batch| {
            for (i, event) in batch.enumerate() {
                *event = i as i64;
            }
        });
    }

    #[test]
    fn test_design_md_event_interfaces() {
        // Test EventFactory trait (DESIGN.md lines 414-417)
        use badbatch::disruptor::EventFactory;

        let factory = DefaultEventFactory::<String>::new();
        let _event = factory.new_instance();

        // Test closure-based factory (DESIGN.md lines 423-427)
        use badbatch::disruptor::event_factory::ClosureEventFactory;
        let closure_factory = ClosureEventFactory::new(|| "test".to_string());
        let _event2 = closure_factory.new_instance();
    }
}

// Test 4: Error handling and edge cases
#[cfg(test)]
mod error_handling_tests {
    use badbatch::disruptor::{is_power_of_two, DefaultEventFactory, RingBuffer};

    #[test]
    fn test_error_types_exist() {
        // Verify that error types mentioned in documentation exist
        use badbatch::disruptor::{DisruptorError, Result};

        // Test various error types
        let _error1 = DisruptorError::BufferFull;
        let _error2 = DisruptorError::InvalidSequence(42);
        let _error3 = DisruptorError::InvalidBufferSize(1023);
        let _error4 = DisruptorError::Timeout;
        let _error5 = DisruptorError::Shutdown;
        let _error6 = DisruptorError::InsufficientCapacity;
        let _error7 = DisruptorError::Alert;

        // Test Result type alias
        let _result: Result<()> = Ok(());
    }

    #[test]
    fn test_power_of_two_validation() {
        // Test utility function
        assert!(is_power_of_two(1024));
        assert!(!is_power_of_two(1023));

        // Test that invalid buffer sizes are rejected
        let factory = DefaultEventFactory::<i32>::new();
        let result = RingBuffer::new(1023, factory); // Not power of 2
        assert!(result.is_err());
    }
}

// Test 5: Wait strategy compatibility
#[cfg(test)]
mod wait_strategy_tests {
    use badbatch::disruptor::{
        simple_wait_strategy::*, BlockingWaitStrategy, BusySpinWaitStrategy, SleepingWaitStrategy,
        YieldingWaitStrategy,
    };

    #[test]
    fn test_traditional_wait_strategies() {
        // Test that traditional LMAX wait strategies exist and work
        let _blocking = BlockingWaitStrategy::new();
        let _busy_spin = BusySpinWaitStrategy;
        let _yielding = YieldingWaitStrategy;
        let _sleeping = SleepingWaitStrategy::new();
    }

    #[test]
    fn test_simple_wait_strategies() {
        // Test that simplified wait strategies from disruptor-rs inspiration work
        let _busy_spin = BusySpin;
        let _busy_spin_hint = BusySpinWithHint;
        let _yielding = Yielding::default();
        let _sleeping = Sleeping::default();

        // Test wait strategy adapters
        let _adapter1 = busy_spin();
        let _adapter2 = busy_spin_with_hint();
        let _adapter3 = yielding();
        let _adapter4 = sleeping();
    }
}
