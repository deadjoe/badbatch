//! Corrected README examples that actually compile and work
//!
//! This file contains the exact examples from README.md but with corrections
//! to make them actually work with the current codebase.

use badbatch::disruptor::{
    Disruptor, ProducerType, BlockingWaitStrategy, DefaultEventFactory,
    EventHandler, EventTranslator, Result,
};

// Corrected Traditional LMAX Disruptor API Example (from README lines 96-161)
#[test]
fn corrected_traditional_lmax_example() {
    use badbatch::disruptor::{
        Disruptor, ProducerType, BlockingWaitStrategy, DefaultEventFactory,
        EventHandler, EventTranslator, Result,
    };

    #[derive(Debug, Default)]
    struct MyEvent {
        value: i64,
        message: String,
    }

    // Event handler implementation
    struct MyEventHandler;

    impl EventHandler<MyEvent> for MyEventHandler {
        fn on_event(&mut self, event: &mut MyEvent, sequence: i64, end_of_batch: bool) -> Result<()> {
            println!("Processing event {} with value {} (end_of_batch: {})",
                     sequence, event.value, end_of_batch);
            Ok(())
        }
    }

    // Create and configure the Disruptor
    let factory = DefaultEventFactory::<MyEvent>::new();
    let mut disruptor = Disruptor::new(
        factory,
        1024, // Buffer size (must be power of 2)
        ProducerType::Single,
        Box::new(BlockingWaitStrategy::new()),
    ).unwrap()
    .handle_events_with(MyEventHandler)
    .build();

    // Start the Disruptor
    disruptor.start().unwrap();

    // Publish events using EventTranslator
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

    let translator = MyEventTranslator {
        value: 42,
        message: "Hello, World!".to_string(),
    };
    disruptor.publish_event(translator).unwrap();

    // Shutdown when done
    disruptor.shutdown().unwrap();
}

// Corrected Modern disruptor-rs Inspired API Example (from README lines 165-212)
#[test]
fn corrected_modern_disruptor_rs_example() {
    use badbatch::disruptor::{
        build_single_producer, Producer, ElegantConsumer, RingBuffer,
        BusySpinWaitStrategy, simple_wait_strategy::BusySpin, event_factory::ClosureEventFactory,
    };
    use std::sync::Arc;

    #[derive(Debug, Default)]
    struct MyEvent {
        value: i64,
    }

    // CORRECTION 1: Use BusySpinWaitStrategy instead of BusySpin for build_single_producer
    // The README showed BusySpin but the builder requires a WaitStrategy trait implementor
    let mut producer = build_single_producer(1024, || MyEvent::default(), BusySpinWaitStrategy)
        .handle_events_with(|event, sequence, end_of_batch| {
            println!("Processing event {} with value {} (batch_end: {})",
                     sequence, event.value, end_of_batch);
        })
        .build();

    // Publish events with closures (this works as documented)
    producer.publish(|event| {
        event.value = 42;
    });

    // Batch publishing (this works as documented)
    producer.batch_publish(5, |batch| {
        for (i, event) in batch.enumerate() {
            event.value = i as i64;
        }
    });

    // CORRECTION 2: ElegantConsumer works correctly with simple wait strategies
    // This part of the README was actually correct
    let factory = ClosureEventFactory::new(|| MyEvent::default());
    let ring_buffer = Arc::new(RingBuffer::new(1024, factory).unwrap());

    let consumer = ElegantConsumer::with_affinity(
        ring_buffer,
        |event, sequence, end_of_batch| {
            println!("Processing: {} at {}", event.value, sequence);
        },
        BusySpin, // This works correctly as ElegantConsumer uses SimpleWaitStrategy
        1, // Pin to CPU core 1
    ).unwrap();

    // Graceful shutdown
    consumer.shutdown().unwrap();
}

// Alternative modern API using SimpleWaitStrategyAdapter
#[test]
fn alternative_modern_api_with_adapter() {
    use badbatch::disruptor::{
        build_single_producer, simple_wait_strategy,
    };

    #[derive(Debug, Default)]
    struct MyEvent {
        value: i64,
    }

    // ALTERNATIVE: Use SimpleWaitStrategyAdapter to bridge the gap
    // This makes the simple wait strategies work with build_single_producer
    let mut producer = build_single_producer(
        1024, 
        || MyEvent::default(), 
        simple_wait_strategy::busy_spin()  // Returns SimpleWaitStrategyAdapter<BusySpin>
    )
    .handle_events_with(|event, sequence, end_of_batch| {
        println!("Processing event {} with value {} (batch_end: {})",
                 sequence, event.value, end_of_batch);
    })
    .build();

    // This API works exactly the same as the corrected version above
    producer.publish(|event| {
        event.value = 42;
    });
}

#[test]
fn test_all_wait_strategy_options() {
    // Show all the wait strategy options that work with build_single_producer
    use badbatch::disruptor::build_single_producer;
    
    #[derive(Debug, Default)]
    struct MyEvent {
        value: i64,
    }

    // Option 1: Traditional LMAX wait strategies
    let _producer1 = build_single_producer(1024, || MyEvent::default(), badbatch::disruptor::BusySpinWaitStrategy)
        .handle_events_with(|event, _seq, _end| { event.value = 1; })
        .build();

    let _producer2 = build_single_producer(1024, || MyEvent::default(), badbatch::disruptor::BlockingWaitStrategy::new())
        .handle_events_with(|event, _seq, _end| { event.value = 2; })
        .build();

    let _producer3 = build_single_producer(1024, || MyEvent::default(), badbatch::disruptor::YieldingWaitStrategy)
        .handle_events_with(|event, _seq, _end| { event.value = 3; })
        .build();

    let _producer4 = build_single_producer(1024, || MyEvent::default(), badbatch::disruptor::SleepingWaitStrategy::new())
        .handle_events_with(|event, _seq, _end| { event.value = 4; })
        .build();

    // Option 2: Simple wait strategy adapters (using helper functions)
    let _producer5 = build_single_producer(1024, || MyEvent::default(), badbatch::disruptor::simple_wait_strategy::busy_spin())
        .handle_events_with(|event, _seq, _end| { event.value = 5; })
        .build();

    let _producer6 = build_single_producer(1024, || MyEvent::default(), badbatch::disruptor::simple_wait_strategy::yielding())
        .handle_events_with(|event, _seq, _end| { event.value = 6; })
        .build();

    let _producer7 = build_single_producer(1024, || MyEvent::default(), badbatch::disruptor::simple_wait_strategy::sleeping())
        .handle_events_with(|event, _seq, _end| { event.value = 7; })
        .build();
}