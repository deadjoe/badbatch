//! Analysis of API discrepancies between documentation and implementation
//!
//! This test file specifically checks for discrepancies between what's documented
//! in README.md and DESIGN.md versus what's actually implemented.

use badbatch::disruptor::*;

#[cfg(test)]
mod api_discrepancy_tests {
    use super::*;

    #[test]
    fn test_readme_modern_api_exact_pattern() {
        // README.md line 178 shows: build_single_producer(1024, || MyEvent::default(), BusySpin)
        // But BusySpin doesn't implement WaitStrategy. Let's check what the correct pattern should be.
        
        #[derive(Debug, Default)]
        struct MyEvent {
            value: i64,
        }

        // This is what the README shows but doesn't work:
        // let mut producer = build_single_producer(1024, || MyEvent::default(), simple_wait_strategy::BusySpin)
        
        // This is what actually works:
        let mut producer = build_single_producer(1024, MyEvent::default, BusySpinWaitStrategy)
            .handle_events_with(|event, _sequence, _end_of_batch| {
                println!("Processing event with value {}", event.value);
            })
            .build();

        // The Producer trait methods work as documented
        producer.publish(|event| {
            event.value = 42;
        });

        // Test if there's a way to use the simple wait strategies with build_single_producer
        // Using the adapter approach:
        let _producer2 = build_single_producer(
            1024, 
            MyEvent::default, 
            simple_wait_strategy::busy_spin()  // This should work as it returns SimpleWaitStrategyAdapter
        )
        .handle_events_with(|event, _sequence, _end_of_batch| {
            println!("Processing event with value {}", event.value);
        })
        .build();
    }

    #[test]
    fn test_elegant_consumer_api_exact_pattern() {
        // README.md lines 198-208 show ElegantConsumer usage
        use std::sync::Arc;
        
        #[derive(Debug, Default)]
        struct MyEvent {
            value: i64,
        }

        // The documented pattern works correctly:
        let factory = event_factory::ClosureEventFactory::new(MyEvent::default);
        let ring_buffer = Arc::new(RingBuffer::new(1024, factory).unwrap());

        let consumer = ElegantConsumer::with_affinity(
            ring_buffer,
            |event, _sequence, _end_of_batch| {
                println!("Processing: {}", event.value);
            },
            simple_wait_strategy::BusySpin,  // This works because ElegantConsumer uses SimpleWaitStrategy
            1, // Pin to CPU core 1
        ).unwrap();

        // Graceful shutdown
        consumer.shutdown().unwrap();
    }

    #[test]
    fn test_traditional_api_exact_pattern() {
        // README.md lines 127-157 show traditional LMAX API
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

        // Create and configure the Disruptor - this pattern works exactly as documented
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

        // EventTranslator pattern works as documented
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

    #[test] 
    fn test_import_patterns_from_readme() {
        // Test that all the imports shown in README.md actually exist and work

        // README.md lines 104-107 (Traditional LMAX API imports)
        use badbatch::disruptor::{
            Disruptor, ProducerType, BlockingWaitStrategy, DefaultEventFactory,
        };

        // README.md lines 166-169 (Modern disruptor-rs API imports)
        use badbatch::disruptor::{
            build_single_producer, RingBuffer,
            simple_wait_strategy::BusySpin, event_factory::ClosureEventFactory,
        };

        // All imports should be available
        let _: Option<Disruptor<i32>> = None;
        let _: Option<ProducerType> = None;
        let _: Option<BlockingWaitStrategy> = None;
        let _: Option<DefaultEventFactory<i32>> = None;
        let _: Option<RingBuffer<i32>> = None;
        let _: Option<BusySpin> = None;
        let _: Option<ClosureEventFactory<i32, fn() -> i32>> = None;

        // Function should exist
        let _factory = || 42i32;
        let _adapter = simple_wait_strategy::busy_spin();
        let _result = build_single_producer(8, _factory, _adapter);
    }

    #[test]
    fn test_discrepancy_wait_strategy_in_builder() {
        // ISSUE IDENTIFIED: README shows BusySpin but build_single_producer needs WaitStrategy
        // The correct pattern should use either:
        // 1. Traditional WaitStrategy types (BusySpinWaitStrategy, BlockingWaitStrategy, etc.)
        // 2. SimpleWaitStrategyAdapter created by helper functions
        
        #[derive(Debug, Default)]
        struct MyEvent {
            value: i64,
        }

        // Pattern 1: Traditional WaitStrategy (this works)
        let _producer1 = build_single_producer(1024, MyEvent::default, BusySpinWaitStrategy)
            .handle_events_with(|event, _seq, _end| {
                event.value = 42;
            })
            .build();

        // Pattern 2: Simple wait strategy adapter (this also works)
        let _producer2 = build_single_producer(1024, MyEvent::default, simple_wait_strategy::busy_spin())
            .handle_events_with(|event, _seq, _end| {
                event.value = 42;
            })
            .build();

        // Pattern 3: What the README shows (this does NOT work):
        // let _producer3 = build_single_producer(1024, || MyEvent::default(), BusySpin) // COMPILE ERROR
    }

    #[test]
    fn check_closure_vs_trait_event_handlers() {
        // The builder accepts closures, traditional Disruptor accepts EventHandler traits
        // Both patterns work as documented
        
        #[derive(Debug, Default)]
        struct MyEvent {
            value: i64,
        }

        // Closure pattern (modern API)
        let _builder_result = build_single_producer(1024, MyEvent::default, BusySpinWaitStrategy)
            .handle_events_with(|event, _sequence, _end_of_batch| {
                // Closure receives: &mut E, i64, bool
                event.value = 42;
            });

        // Trait pattern (traditional API)
        struct MyHandler;
        impl EventHandler<MyEvent> for MyHandler {
            fn on_event(&mut self, event: &mut MyEvent, sequence: i64, _end_of_batch: bool) -> badbatch::disruptor::Result<()> {
                // Method receives: &mut E, i64, bool -> Result<()>
                event.value = sequence;
                Ok(())
            }
        }

        let factory = DefaultEventFactory::<MyEvent>::new();
        let _disruptor_result = Disruptor::new(
            factory,
            1024,
            ProducerType::Single,
            Box::new(BusySpinWaitStrategy),
        ).unwrap()
        .handle_events_with(MyHandler);
    }
}

#[cfg(test)]
mod design_md_verification {
    use super::*;

    #[test]
    fn test_design_md_factory_patterns() {
        // DESIGN.md lines 414-427 show EventFactory patterns
        
        // DefaultEventFactory pattern
        let default_factory = DefaultEventFactory::<String>::new();
        let _event1 = default_factory.new_instance();

        // ClosureEventFactory pattern  
        let closure_factory = event_factory::ClosureEventFactory::new(|| "test".to_string());
        let _event2 = closure_factory.new_instance();
        
        // Both patterns work as documented
    }

    #[test]
    fn test_design_md_producer_trait() {
        // DESIGN.md lines 117-123 show Producer trait methods
        
        #[derive(Debug, Default)]
        struct TestEvent {
            value: i64,
        }

        let factory = DefaultEventFactory::<TestEvent>::new();
        let ring_buffer = std::sync::Arc::new(RingBuffer::new(1024, factory).unwrap());
        let sequencer = std::sync::Arc::new(SingleProducerSequencer::new(
            1024, 
            std::sync::Arc::new(BusySpinWaitStrategy)
        ));
        
        let mut producer = producer::SimpleProducer::new(ring_buffer, sequencer);
        
        // All documented methods exist and work
        let _result1 = producer.try_publish(|event| event.value = 42);
        let _result2 = producer.try_batch_publish(3, |batch| {
            for (i, event) in batch.enumerate() {
                event.value = i as i64;
            }
        });
        
        producer.publish(|event| event.value = 99);
        producer.batch_publish(2, |batch| {
            for event in batch {
                event.value = 100;
            }
        });
    }
}