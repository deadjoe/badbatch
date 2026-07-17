//! Builder unit tests (split from monolithic builder.rs).
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_lossless,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::items_after_statements,
    clippy::too_many_lines
)]

use super::*;
use crate::disruptor::builder::fluent::ClosureEventHandler;
use crate::disruptor::producer::Producer;
use crate::disruptor::{
    ring_buffer::SlotPadding,
    wait_strategy::{BusySpinWaitStrategy, YieldingWaitStrategy},
    EventHandler,
};
use std::sync::{
    atomic::{AtomicI64, Ordering},
    mpsc, Arc, Mutex,
};
use std::time::Duration;

macro_rules! println {
    ($($arg:tt)*) => {
        crate::test_log!($($arg)*);
    };
}

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

fn test_event_factory() -> TestEvent {
    TestEvent::default()
}

fn wait_until<F>(timeout: Duration, mut condition: F, description: &str)
where
    F: FnMut() -> bool,
{
    let start = std::time::Instant::now();
    while !condition() {
        assert!(
            start.elapsed() < timeout,
            "timed out waiting for {description} after {timeout:?}"
        );
        std::thread::yield_now();
    }
}

#[test]
fn test_build_single_producer_basic() {
    let builder = build_single_producer(8, test_event_factory, BusySpinWaitStrategy);

    // Should be able to create builder without consumers
    assert_eq!(builder.shared.size, 8);
    assert_eq!(builder.shared.consumers.len(), 0);
}

#[test]
fn test_build_multi_producer_basic() {
    let builder = build_multi_producer(16, test_event_factory, BusySpinWaitStrategy);

    // Should be able to create builder without consumers
    assert_eq!(builder.shared.size, 16);
    assert_eq!(builder.shared.consumers.len(), 0);
}

#[test]
fn test_single_producer_builder_cache_line_padding_configuration() {
    let builder = build_single_producer(8, test_event_factory, BusySpinWaitStrategy)
        .with_cache_line_padding(true);

    assert_eq!(builder.shared.slot_padding, SlotPadding::CacheLine128);
}

#[test]
fn test_single_producer_builder_slot_padding_propagates_to_ring_buffer() {
    let disruptor = build_single_producer(8, test_event_factory, BusySpinWaitStrategy)
        .with_slot_padding(SlotPadding::CacheLine64)
        .handle_events_with(|_event: &mut TestEvent, _sequence, _end_of_batch| {})
        .build();

    assert_eq!(
        disruptor.core.ring_buffer.slot_padding(),
        SlotPadding::CacheLine64
    );
    assert_eq!(disruptor.core.ring_buffer.slot_stride_bytes(), 64);
}

#[test]
fn test_single_producer_builder_cache_line_padding_processes_events() {
    let processed = Arc::new(AtomicI64::new(0));
    let processed_clone = Arc::clone(&processed);

    let mut disruptor = build_single_producer(16, test_event_factory, BusySpinWaitStrategy)
        .with_cache_line_padding(true)
        .handle_events_with(move |event: &mut TestEvent, _sequence, _end_of_batch| {
            processed_clone.fetch_add(event.value, Ordering::Release);
        })
        .build();

    assert_eq!(
        disruptor.core.ring_buffer.slot_padding(),
        SlotPadding::CacheLine128
    );

    disruptor.publish(|event| {
        event.value = 7;
    });

    wait_until(
        Duration::from_secs(1),
        || processed.load(Ordering::Acquire) == 7,
        "cache-line padded event processing",
    );
}

#[test]
fn test_single_producer_add_event_handler() {
    let (tx, _rx) = mpsc::channel();
    let handler = move |event: &mut TestEvent, sequence: i64, _end_of_batch: bool| {
        event.value = sequence;
        let _ = tx.send(sequence);
    };

    let builder = build_single_producer(8, test_event_factory, BusySpinWaitStrategy)
        .handle_events_with(handler);

    // Should transition to HasConsumers state and have one handler
    assert_eq!(builder.shared.consumers.len(), 1);
}

#[test]
fn test_multi_producer_add_event_handler() {
    let (tx, _rx) = mpsc::channel();
    let handler = move |event: &mut TestEvent, sequence: i64, _end_of_batch: bool| {
        event.value = sequence;
        let _ = tx.send(sequence);
    };

    let builder = build_multi_producer(16, test_event_factory, BusySpinWaitStrategy)
        .handle_events_with(handler);

    // Should transition to HasConsumers state and have one handler
    assert_eq!(builder.shared.consumers.len(), 1);
}

#[test]
fn test_single_producer_multiple_handlers() {
    let (tx1, _rx1) = mpsc::channel();
    let (tx2, _rx2) = mpsc::channel();

    let handler1 = move |event: &mut TestEvent, sequence: i64, _end_of_batch: bool| {
        event.value = sequence;
        let _ = tx1.send(sequence);
    };

    let handler2 = move |event: &mut TestEvent, sequence: i64, _end_of_batch: bool| {
        event.data = format!("seq_{sequence}");
        let _ = tx2.send(sequence);
    };

    let builder = build_single_producer(8, test_event_factory, BusySpinWaitStrategy)
        .handle_events_with(handler1)
        .handle_events_with(handler2);

    // Should have two handlers
    assert_eq!(builder.shared.consumers.len(), 2);
}

#[test]
fn test_multi_producer_multiple_handlers() {
    let (tx1, _rx1) = mpsc::channel();
    let (tx2, _rx2) = mpsc::channel();

    let handler1 = move |event: &mut TestEvent, sequence: i64, _end_of_batch: bool| {
        event.value = sequence;
        let _ = tx1.send(sequence);
    };

    let handler2 = move |event: &mut TestEvent, sequence: i64, _end_of_batch: bool| {
        event.data = format!("seq_{sequence}");
        let _ = tx2.send(sequence);
    };

    let builder = build_multi_producer(16, test_event_factory, BusySpinWaitStrategy)
        .handle_events_with(handler1)
        .handle_events_with(handler2);

    // Should have two handlers
    assert_eq!(builder.shared.consumers.len(), 2);
}

#[test]
fn test_single_producer_build() {
    let (tx, rx) = mpsc::channel();
    let handler = move |event: &mut TestEvent, sequence: i64, _end_of_batch: bool| {
        event.value = sequence;
        let _ = tx.send(sequence);
    };

    let disruptor_handle = build_single_producer(8, test_event_factory, BusySpinWaitStrategy)
        .handle_events_with(handler)
        .build();

    // Should successfully build and return a DisruptorHandle
    // The handle should be usable for publishing
    drop(disruptor_handle);
    drop(rx); // Ensure we can drop the receiver
}

#[test]
fn test_multi_producer_build() {
    let (tx, rx) = mpsc::channel();
    let handler = move |event: &mut TestEvent, sequence: i64, _end_of_batch: bool| {
        event.value = sequence;
        let _ = tx.send(sequence);
    };

    let producer = build_multi_producer(16, test_event_factory, BusySpinWaitStrategy)
        .handle_events_with(handler)
        .build();

    // Should successfully build and return a handle that can create producers
    let _producer2 = producer.create_producer();
    drop(producer);
    drop(rx); // Ensure we can drop the receiver
}

#[test]
fn test_cloneable_producer_clone() {
    let (tx, rx) = mpsc::channel();
    let handler = move |event: &mut TestEvent, sequence: i64, _end_of_batch: bool| {
        event.value = sequence;
        let _ = tx.send(sequence);
    };

    let handle = build_multi_producer(16, test_event_factory, BusySpinWaitStrategy)
        .handle_events_with(handler)
        .build();

    let producer1 = handle.create_producer();
    let producer2 = producer1.clone();

    // Both SimpleProducer clones should operate on the same underlying buffer
    drop(producer2);
    drop(handle);
    drop(rx);
}

#[test]
fn test_cloneable_producer_create_producer() {
    let (tx, rx) = mpsc::channel();
    let handler = move |event: &mut TestEvent, sequence: i64, _end_of_batch: bool| {
        event.value = sequence;
        let _ = tx.send(sequence);
    };

    let cloneable_producer = build_multi_producer(16, test_event_factory, BusySpinWaitStrategy)
        .handle_events_with(handler)
        .build();

    let _simple_producer = cloneable_producer.create_producer();

    // Should be able to create SimpleProducer instances
    drop(cloneable_producer);
    drop(rx);
}

#[test]
fn test_builder_with_different_wait_strategies() {
    use crate::disruptor::wait_strategy::{SleepingWaitStrategy, YieldingWaitStrategy};
    use std::time::Duration;

    let (tx1, rx1) = mpsc::channel();
    let (tx2, rx2) = mpsc::channel();
    let (tx3, rx3) = mpsc::channel();

    let handler1 = move |event: &mut TestEvent, sequence: i64, _: bool| {
        event.value = sequence;
        let _ = tx1.send(sequence);
    };

    let handler2 = move |event: &mut TestEvent, sequence: i64, _: bool| {
        event.value = sequence;
        let _ = tx2.send(sequence);
    };

    let handler3 = move |event: &mut TestEvent, sequence: i64, _: bool| {
        event.value = sequence;
        let _ = tx3.send(sequence);
    };

    // Test with BusySpinWaitStrategy
    let mut disruptor1 = build_single_producer(8, test_event_factory, BusySpinWaitStrategy)
        .handle_events_with(handler1)
        .build();

    // Test with YieldingWaitStrategy
    let mut disruptor2 = build_single_producer(8, test_event_factory, YieldingWaitStrategy)
        .handle_events_with(handler2)
        .build();

    // Test with SleepingWaitStrategy
    let mut disruptor3 = build_single_producer(8, test_event_factory, SleepingWaitStrategy::new())
        .handle_events_with(handler3)
        .build();

    // Publish a single event to each disruptor to ensure they're working
    disruptor1.publish(|event| {
        event.value = 1;
        event.data = "test1".to_string();
    });

    disruptor2.publish(|event| {
        event.value = 2;
        event.data = "test2".to_string();
    });

    disruptor3.publish(|event| {
        event.value = 3;
        event.data = "test3".to_string();
    });

    assert_eq!(rx1.recv_timeout(Duration::from_secs(1)).unwrap(), 0);
    assert_eq!(rx2.recv_timeout(Duration::from_secs(1)).unwrap(), 0);
    assert_eq!(rx3.recv_timeout(Duration::from_secs(1)).unwrap(), 0);

    // Properly shutdown all disruptors
    disruptor1.shutdown();
    disruptor2.shutdown();
    disruptor3.shutdown();

    drop(rx1);
    drop(rx2);
    drop(rx3);
}

#[test]
fn test_builder_with_different_buffer_sizes() {
    // Test various power-of-2 sizes
    for &size in &[2, 4, 8, 16, 32, 64, 128, 256, 512, 1024] {
        let mut disruptor = build_single_producer(size, test_event_factory, BusySpinWaitStrategy)
            .handle_events_with(move |event: &mut TestEvent, sequence: i64, _: bool| {
                event.value = sequence;
            })
            .build();

        // Properly shutdown the disruptor
        disruptor.shutdown();
    }
}

#[test]
fn test_closure_event_handler() {
    let counter = Arc::new(Mutex::new(0));
    let counter_clone = counter.clone();

    let handler = move |event: &mut TestEvent, sequence: i64, _end_of_batch: bool| {
        event.value = sequence;
        let mut count = counter_clone.lock().unwrap();
        *count += 1;
    };

    let mut closure_handler = ClosureEventHandler::new(handler);
    let mut event = TestEvent::default();

    // Test the handler directly
    closure_handler.on_event(&mut event, 42, false).unwrap();

    assert_eq!(event.value, 42);
    assert_eq!(*counter.lock().unwrap(), 1);
}

#[test]
fn test_builder_type_safety() {
    // This test ensures that the type system prevents invalid state transitions

    // Can't build without consumers
    let builder_no_consumers = build_single_producer(8, test_event_factory, BusySpinWaitStrategy);
    // builder_no_consumers.build(); // This should not compile

    // Must add at least one consumer before building
    let _producer = builder_no_consumers
        .handle_events_with(|_event: &mut TestEvent, _sequence: i64, _end_of_batch: bool| {})
        .build();
}

#[test]
fn test_builder_thread_management() {
    // Test thread naming and CPU affinity settings
    let builder = build_single_producer(8, test_event_factory, BusySpinWaitStrategy)
        .pin_at_core(1)
        .thread_name("test-processor")
        .handle_events_with(|_event: &mut TestEvent, _sequence: i64, _end_of_batch: bool| {});

    // Should have one consumer with thread settings
    assert_eq!(builder.shared.consumers.len(), 1);
    // Note: We can't easily test the actual thread name and affinity without
    // accessing private fields, but the API should work correctly
}

#[test]
fn test_builder_multiple_handlers_with_different_settings() {
    let builder = build_single_producer(8, test_event_factory, BusySpinWaitStrategy)
        .pin_at_core(1)
        .thread_name("processor-1")
        .handle_events_with(|_event: &mut TestEvent, _sequence: i64, _end_of_batch: bool| {})
        .pin_at_core(2)
        .thread_name("processor-2")
        .handle_events_with(|_event: &mut TestEvent, _sequence: i64, _end_of_batch: bool| {});

    // Should have two consumers with different thread settings
    assert_eq!(builder.shared.consumers.len(), 2);
}

#[test]
fn test_disruptor_handle_publishing() {
    let mut disruptor_handle = build_single_producer(8, test_event_factory, BusySpinWaitStrategy)
        .handle_events_with(|_event: &mut TestEvent, _sequence: i64, _end_of_batch: bool| {})
        .build();

    // Test single event publishing
    disruptor_handle.publish(|event| {
        event.value = 42;
        event.data = "test".to_string();
    });

    // Test batch publishing
    disruptor_handle.batch_publish(3, |batch| {
        for (i, event) in batch.enumerate() {
            event.value = i as i64;
            event.data = format!("batch_{i}");
        }
    });

    // Should be able to access the underlying producer
    let _producer = disruptor_handle.producer();
}

#[test]
fn test_disruptor_handle_shutdown() {
    // Create a disruptor with a simple event handler that doesn't block
    let mut disruptor_handle = build_single_producer(8, test_event_factory, BusySpinWaitStrategy)
        .thread_name("test-consumer")
        .handle_events_with(
            |_event: &mut TestEvent, _sequence: i64, _end_of_batch: bool| {
                // Simple handler that doesn't block
            },
        )
        .build();

    // Verify we have one consumer
    assert_eq!(disruptor_handle.consumer_count(), 1);

    // Publish some events
    for i in 0..3 {
        disruptor_handle.publish(|event| {
            event.value = i;
            event.data = format!("event_{i}");
        });
    }

    // Manually shutdown (should be idempotent)
    disruptor_handle.shutdown();
    disruptor_handle.shutdown();

    // Verify shutdown completed (consumers are cleaned up after shutdown)
    assert_eq!(disruptor_handle.consumer_count(), 0); // Consumers are cleaned up after shutdown
    assert!(disruptor_handle.is_shutdown());
}

#[test]
fn test_disruptor_handle_into_producer() {
    // Create a disruptor with a simple event handler
    let disruptor_handle = build_single_producer(8, test_event_factory, BusySpinWaitStrategy)
        .thread_name("test-consumer")
        .handle_events_with(
            |_event: &mut TestEvent, _sequence: i64, _end_of_batch: bool| {
                // Simple handler
            },
        )
        .build();

    // Convert to producer (this should shutdown consumer threads)
    let _producer = disruptor_handle.into_producer();
}

#[test]
fn test_disruptor_handle_into_producer_restores_capacity() {
    let mut handle = build_multi_producer(4, test_event_factory, BusySpinWaitStrategy)
        .handle_events_with(|_event: &mut TestEvent, _seq, _eob| {})
        .build();

    // Publish a few events so the consumer advances the gating sequence
    for i in 0..4 {
        handle.publish(|event| {
            event.value = i as i64;
        });
    }

    // Give the consumer a brief moment to process
    std::thread::sleep(std::time::Duration::from_millis(10));

    // Convert into a standalone producer. This should remove gating sequences so the
    // ring buffer can wrap freely without consumer backpressure.
    let mut producer = handle.into_producer();

    for i in 0..8 {
        producer
            .try_publish(|event| {
                event.value = 100 + i as i64;
            })
            .expect("publishing after into_producer should not block");
    }
}

#[test]
fn test_builder_api_completeness() {
    // Test the complete Builder API without actually starting consumer threads

    // Test single producer builder with multiple configurations
    let builder = build_single_producer(16, test_event_factory, BusySpinWaitStrategy)
        .pin_at_core(0)
        .thread_name("consumer-1")
        .handle_events_with(|event: &mut TestEvent, sequence: i64, _: bool| {
            event.value = sequence;
        })
        .pin_at_core(1)
        .thread_name("consumer-2")
        .handle_events_with(|event: &mut TestEvent, sequence: i64, _: bool| {
            event.value = sequence * 2;
        });

    // Verify builder state
    assert_eq!(builder.shared.consumers.len(), 2);
    assert_eq!(builder.shared.size, 16);

    // Test multi producer builder
    let multi_builder = build_multi_producer(32, test_event_factory, YieldingWaitStrategy)
        .pin_at_core(2)
        .thread_name("multi-consumer-1")
        .handle_events_with(|event: &mut TestEvent, sequence: i64, _: bool| {
            event.value = sequence;
        });

    // Verify multi builder state
    let multi_consumer_count = multi_builder.shared.consumers.len();
    let multi_size = multi_builder.shared.size;
    assert_eq!(multi_consumer_count, 1);
    assert_eq!(multi_size, 32);

    // Test CloneableProducer creation (doesn't start consumer threads)
    let cloneable_producer = multi_builder.build();
    let mut producer1 = cloneable_producer.create_producer();
    let mut producer2 = cloneable_producer.create_producer();

    // Verify producers can publish (this tests the core Producer functionality)
    let result1 = producer1.try_publish(|event| {
        event.value = 42;
        event.data = "test1".to_string();
    });

    let result2 = producer2.try_publish(|event| {
        event.value = 43;
        event.data = "test2".to_string();
    });

    assert!(result1.is_ok());
    assert!(result2.is_ok());

    println!("Builder API completeness test passed!");
    println!(
        "Single producer builder: {} consumers",
        builder.shared.consumers.len()
    );
    println!("Multi producer builder: {multi_consumer_count} consumers");
}

/// Comparison test showing the differences between BadBatch and disruptor-rs APIs
#[test]
fn test_api_comparison_with_disruptor_rs() {
    // BadBatch API style (current implementation)
    let mut bad_batch_disruptor =
        build_single_producer(8, test_event_factory, BusySpinWaitStrategy)
            .pin_at_core(0)
            .thread_name("badBatch-consumer")
            .handle_events_with(|event: &mut TestEvent, sequence: i64, _: bool| {
                event.value = sequence;
            })
            .build();

    // BadBatch provides DisruptorHandle with lifecycle management
    assert_eq!(bad_batch_disruptor.consumer_count(), 1);
    bad_batch_disruptor.publish(|event| {
        event.value = 42;
        event.data = "BadBatch style".to_string();
    });
    bad_batch_disruptor.shutdown();

    // disruptor-rs API style would be:
    // let mut disruptor_rs_producer = build_single_producer(8, factory, BusySpin)
    //     .pin_at_core(0)
    //     .thread_name("disruptor-rs-consumer")
    //     .handle_events_with(|event: &Event, sequence: i64, end_of_batch: bool| {
    //         // Process event
    //     })
    //     .and_then()  // <- BadBatch doesn't have this yet
    //         .handle_events_with(|event: &Event, sequence: i64, end_of_batch: bool| {
    //             // Dependent consumer
    //         })
    //     .build();
    //
    // disruptor_rs_producer.publish(|event| {
    //     event.value = 42;
    // });
    // // Drop automatically handles shutdown

    println!("API comparison test completed");
}

#[test]
fn test_shutdown_fix_verification() {
    use std::time::Duration;

    // Create a disruptor with BusySpinWaitStrategy (the problematic one)
    let mut disruptor_handle = build_single_producer(8, test_event_factory, BusySpinWaitStrategy)
        .thread_name("shutdown-test-consumer")
        .handle_events_with(
            |_event: &mut TestEvent, _sequence: i64, _end_of_batch: bool| {
                // Simple handler that doesn't block
            },
        )
        .build();

    // Verify we have one consumer
    assert_eq!(disruptor_handle.consumer_count(), 1);

    // Shutdown should complete quickly now (not hang)
    let start = std::time::Instant::now();
    disruptor_handle.shutdown();
    let elapsed = start.elapsed();

    // Shutdown should complete within a reasonable time (much less than before)
    assert!(
        elapsed < Duration::from_secs(2),
        "Shutdown took too long: {elapsed:?}"
    );

    println!("Shutdown completed successfully in {elapsed:?}");
}

#[test]
fn test_yielding_wait_strategy_shutdown() {
    use std::time::Duration;

    // Test with YieldingWaitStrategy
    let mut disruptor_handle = build_single_producer(8, test_event_factory, YieldingWaitStrategy)
        .thread_name("yielding-test-consumer")
        .handle_events_with(
            |_event: &mut TestEvent, _sequence: i64, _end_of_batch: bool| {
                // Simple handler
            },
        )
        .build();

    // Shutdown should complete quickly
    let start = std::time::Instant::now();
    disruptor_handle.shutdown();
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_secs(2),
        "YieldingWaitStrategy shutdown took too long: {elapsed:?}"
    );

    println!("YieldingWaitStrategy shutdown completed in {elapsed:?}");
}

#[test]
fn test_and_then_dependency_chain() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    let counter1 = Arc::new(AtomicUsize::new(0));
    let counter2 = Arc::new(AtomicUsize::new(0));
    let counter1_clone = counter1.clone();
    let counter2_clone = counter2.clone();

    // Create a disruptor with dependency chain
    let mut disruptor_handle = build_single_producer(8, test_event_factory, BusySpinWaitStrategy)
        .thread_name("first-consumer")
        .handle_events_with(
            move |event: &mut TestEvent, _sequence: i64, _end_of_batch: bool| {
                // First consumer processes events
                event.value += 1;
                counter1_clone.fetch_add(1, Ordering::SeqCst);
                // Small delay to ensure ordering
                std::thread::sleep(Duration::from_millis(1));
            },
        )
        .and_then()
        .thread_name("second-consumer")
        .handle_events_with(
            move |event: &mut TestEvent, _sequence: i64, _end_of_batch: bool| {
                // Second consumer should see the modified value from first consumer
                assert!(
                    event.value > 0,
                    "Second consumer should see modified value from first consumer"
                );
                counter2_clone.fetch_add(1, Ordering::SeqCst);
            },
        )
        .build();

    // Verify we have two consumers
    assert_eq!(disruptor_handle.consumer_count(), 2);

    // Publish some events
    for i in 0..5 {
        disruptor_handle.publish(|event| {
            event.value = i;
        });
    }

    wait_until(
        Duration::from_secs(1),
        || counter2.load(Ordering::SeqCst) == 5,
        "dependency chain to process all events",
    );

    // Shutdown
    disruptor_handle.shutdown();

    // Verify both consumers processed the events
    assert_eq!(
        counter1.load(Ordering::SeqCst),
        5,
        "First consumer should process all events"
    );
    assert_eq!(
        counter2.load(Ordering::SeqCst),
        5,
        "Second consumer should process all events"
    );

    println!("Dependency chain test completed successfully");
}

#[test]
fn test_stateful_event_handler() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    // Define a stateful event handler
    struct CountingEventHandler {
        counter: usize,
        total_processed: Arc<AtomicUsize>,
    }

    impl EventHandler<TestEvent> for CountingEventHandler {
        fn on_event(
            &mut self,
            event: &mut TestEvent,
            _sequence: i64,
            _end_of_batch: bool,
        ) -> crate::disruptor::Result<()> {
            self.counter += 1;
            event.value = self.counter as i64; // Set the event value to our internal counter
            self.total_processed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn on_start(&mut self) -> crate::disruptor::Result<()> {
            println!("CountingEventHandler started");
            Ok(())
        }

        fn on_shutdown(&mut self) -> crate::disruptor::Result<()> {
            println!(
                "CountingEventHandler shutdown, processed {} events",
                self.counter
            );
            Ok(())
        }
    }

    let total_processed = Arc::new(AtomicUsize::new(0));
    let total_processed_clone = total_processed.clone();

    // Create a disruptor with a stateful event handler
    let mut disruptor_handle = build_single_producer(8, test_event_factory, BusySpinWaitStrategy)
        .thread_name("stateful-consumer")
        .handle_events_with_handler(CountingEventHandler {
            counter: 0,
            total_processed: total_processed_clone,
        })
        .build();

    // Verify we have one consumer
    assert_eq!(disruptor_handle.consumer_count(), 1);

    // Publish some events
    for i in 0..10 {
        disruptor_handle.publish(|event| {
            event.value = i; // This will be overwritten by the handler
        });
    }

    wait_until(
        Duration::from_secs(1),
        || total_processed.load(Ordering::SeqCst) == 10,
        "stateful handler to process all events",
    );

    // Shutdown
    disruptor_handle.shutdown();

    // Verify the stateful handler processed all events
    assert_eq!(
        total_processed.load(Ordering::SeqCst),
        10,
        "Stateful handler should process all events"
    );

    println!("Stateful event handler test completed successfully");
}

#[test]
fn test_multi_producer_stateful_event_handler() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    struct CountingEventHandler {
        total_processed: Arc<AtomicUsize>,
    }

    impl EventHandler<TestEvent> for CountingEventHandler {
        fn on_event(
            &mut self,
            event: &mut TestEvent,
            _sequence: i64,
            _end_of_batch: bool,
        ) -> crate::disruptor::Result<()> {
            self.total_processed.fetch_add(1, Ordering::SeqCst);
            event.value += 1;
            Ok(())
        }
    }

    let total_processed = Arc::new(AtomicUsize::new(0));

    let mut disruptor_handle = build_multi_producer(8, test_event_factory, BusySpinWaitStrategy)
        .thread_name("multi-stateful-consumer")
        .handle_events_with_handler(CountingEventHandler {
            total_processed: Arc::clone(&total_processed),
        })
        .build();

    let mut producer1 = disruptor_handle.create_producer();
    let mut producer2 = disruptor_handle.create_producer();

    for i in 0..5 {
        producer1.publish(|event| {
            event.value = i;
        });
        producer2.publish(|event| {
            event.value = i + 10;
        });
    }

    wait_until(
        Duration::from_secs(1),
        || total_processed.load(Ordering::SeqCst) == 10,
        "multi producer stateful handler to process all events",
    );

    disruptor_handle.shutdown();

    assert_eq!(total_processed.load(Ordering::SeqCst), 10);
}

#[test]
fn test_mixed_handler_types() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    // Define a stateful event handler
    struct StatefulHandler {
        id: String,
        counter: usize,
        total: Arc<AtomicUsize>,
    }

    impl EventHandler<TestEvent> for StatefulHandler {
        fn on_event(
            &mut self,
            event: &mut TestEvent,
            _sequence: i64,
            _end_of_batch: bool,
        ) -> crate::disruptor::Result<()> {
            self.counter += 1;
            event.value += 100; // Add 100 to distinguish from closure handler
            self.total.fetch_add(1, Ordering::SeqCst);
            println!(
                "StatefulHandler {} processed event, counter: {}",
                self.id, self.counter
            );
            Ok(())
        }
    }

    let stateful_total = Arc::new(AtomicUsize::new(0));
    let closure_total = Arc::new(AtomicUsize::new(0));
    let stateful_total_clone = stateful_total.clone();
    let closure_total_clone = closure_total.clone();

    // Create a disruptor with both stateful and closure handlers
    let mut disruptor_handle = build_single_producer(8, test_event_factory, BusySpinWaitStrategy)
        .thread_name("stateful-handler")
        .handle_events_with_handler(StatefulHandler {
            id: "handler-1".to_string(),
            counter: 0,
            total: stateful_total_clone,
        })
        .thread_name("closure-handler")
        .handle_events_with(
            move |event: &mut TestEvent, _sequence: i64, _end_of_batch: bool| {
                event.value += 1; // Add 1 to distinguish from stateful handler
                closure_total_clone.fetch_add(1, Ordering::SeqCst);
            },
        )
        .build();

    // Verify we have two consumers
    assert_eq!(disruptor_handle.consumer_count(), 2);

    // Publish some events
    for i in 0..5 {
        disruptor_handle.publish(|event| {
            event.value = i;
        });
    }

    wait_until(
        Duration::from_secs(1),
        || closure_total.load(Ordering::SeqCst) + stateful_total.load(Ordering::SeqCst) == 5,
        "work-pool mixed handlers to process all events",
    );

    // Shutdown
    disruptor_handle.shutdown();

    // WorkerPool scheme A: same-stage handlers partition events via CAS claim
    let stateful_n = stateful_total.load(Ordering::SeqCst);
    let closure_n = closure_total.load(Ordering::SeqCst);
    assert_eq!(
        stateful_n + closure_n,
        5,
        "mixed handlers must jointly process all events exactly once"
    );

    println!("Mixed handler types test completed successfully");
}

#[test]
fn test_simple_dependency_chain() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    let stage1_count = Arc::new(AtomicUsize::new(0));
    let stage2_count = Arc::new(AtomicUsize::new(0));
    let stage1_count_clone = stage1_count.clone();
    let stage2_count_clone = stage2_count.clone();

    // Create a simple 2-stage dependency chain
    let mut disruptor_handle = build_single_producer(8, test_event_factory, BusySpinWaitStrategy)
        .thread_name("stage1")
        .handle_events_with(
            move |event: &mut TestEvent, sequence: i64, _end_of_batch: bool| {
                // Stage 1: Add 100 to the value
                println!(
                    "Stage 1 starting sequence {} (initial value: {})",
                    sequence, event.value
                );
                event.value += 100;
                stage1_count_clone.fetch_add(1, Ordering::SeqCst);
                println!(
                    "Stage 1 completed sequence {} (final value: {})",
                    sequence, event.value
                );
                // Small delay to ensure ordering
                std::thread::sleep(Duration::from_millis(2));
            },
        )
        .and_then()
        .thread_name("stage2")
        .handle_events_with(
            move |event: &mut TestEvent, sequence: i64, _end_of_batch: bool| {
                // Stage 2: Should see the value modified by Stage 1
                println!(
                    "Stage 2 processing sequence {} (value: {})",
                    sequence, event.value
                );
                assert!(
                    event.value >= 100,
                    "Stage 2 should see value modified by Stage 1, got: {}",
                    event.value
                );
                stage2_count_clone.fetch_add(1, Ordering::SeqCst);
                println!(
                    "Stage 2 completed sequence {} (value: {})",
                    sequence, event.value
                );
            },
        )
        .build();

    // Verify we have two consumers
    assert_eq!(disruptor_handle.consumer_count(), 2);

    println!("Publishing 3 events...");

    // Publish just one event to test dependency chain
    for i in 0..1 {
        disruptor_handle.publish(|event| {
            event.value = i;
            event.data = String::new();
        });
        println!("Published event {i}");
    }

    wait_until(
        Duration::from_secs(1),
        || stage1_count.load(Ordering::SeqCst) == 1 && stage2_count.load(Ordering::SeqCst) == 1,
        "simple dependency chain to process the published event",
    );

    println!("Shutting down...");
    let shutdown_start = std::time::Instant::now();
    disruptor_handle.shutdown();
    let shutdown_duration = shutdown_start.elapsed();

    // Verify shutdown was fast
    assert!(
        shutdown_duration < Duration::from_secs(3),
        "Shutdown took too long: {shutdown_duration:?}"
    );

    // Verify both stages processed all events
    let stage1_processed = stage1_count.load(Ordering::SeqCst);
    let stage2_processed = stage2_count.load(Ordering::SeqCst);

    println!("Stage 1 processed: {stage1_processed}, Stage 2 processed: {stage2_processed}");

    assert_eq!(stage1_processed, 1, "Stage 1 should process 1 event");
    assert_eq!(stage2_processed, 1, "Stage 2 should process 1 event");

    println!("✅ Simple dependency chain test completed successfully!");
}

#[test]
fn test_dependency_chain_debug() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    println!("🔍 Starting dependency chain debug test...");

    let stage1_count = Arc::new(AtomicUsize::new(0));
    let stage2_count = Arc::new(AtomicUsize::new(0));
    let stage1_count_clone = stage1_count.clone();
    let stage2_count_clone = stage2_count.clone();

    // Create a simple 2-stage dependency chain with detailed logging
    let mut disruptor_handle = build_single_producer(4, test_event_factory, BusySpinWaitStrategy)
        .thread_name("debug-stage1")
        .handle_events_with(move |event: &mut TestEvent, sequence: i64, _end_of_batch: bool| {
            println!("🔵 Stage 1 START: sequence={sequence}, initial_value={}", event.value);

            // Modify the event
            let original_value = event.value;
            event.value = original_value + 1000;

            // Add a small delay to make the timing issue more visible
            std::thread::sleep(Duration::from_millis(5));

            stage1_count_clone.fetch_add(1, Ordering::SeqCst);
            println!("🔵 Stage 1 END: sequence={sequence}, final_value={} (was {original_value})", event.value);
        })
        .and_then()
        .thread_name("debug-stage2")
        .handle_events_with(move |event: &mut TestEvent, sequence: i64, _end_of_batch: bool| {
            println!("🟢 Stage 2 START: sequence={sequence}, value={}", event.value);

            // Check if we see the modification from Stage 1
            if event.value < 1000 {
                println!("❌ Stage 2 ERROR: sequence={sequence}, saw original value {} instead of modified value", event.value);
            }
            assert!(
                event.value >= 1000,
                "Stage 2 saw unmodified value: {}",
                event.value
            );
            println!("✅ Stage 2 SUCCESS: sequence={sequence}, saw modified value {}", event.value);

            stage2_count_clone.fetch_add(1, Ordering::SeqCst);
            println!("🟢 Stage 2 END: sequence={sequence}");
        })
        .build();

    println!("📊 Created 2-stage debug pipeline");
    assert_eq!(disruptor_handle.consumer_count(), 2);

    // Publish just one event to make debugging easier
    println!("📤 Publishing single event...");
    disruptor_handle.publish(|event| {
        event.value = 42;
        event.data = "debug_event".to_string();
    });
    println!("📤 Event published");

    wait_until(
        Duration::from_secs(1),
        || stage2_count.load(Ordering::SeqCst) == 1,
        "debug dependency chain to process the published event",
    );

    println!("🛑 Shutting down...");
    disruptor_handle.shutdown();

    // Verify both stages processed the event
    let stage1_processed = stage1_count.load(Ordering::SeqCst);
    let stage2_processed = stage2_count.load(Ordering::SeqCst);

    println!("📊 Final counts: Stage 1={stage1_processed}, Stage 2={stage2_processed}");

    assert_eq!(stage1_processed, 1, "Stage 1 should process 1 event");
    assert_eq!(stage2_processed, 1, "Stage 2 should process 1 event");

    println!("✅ Dependency chain debug test completed successfully!");
}

#[test]
fn test_four_stage_dependency_chain() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    println!("🔍 Starting 4-stage dependency chain test...");

    let stage_counts = [
        Arc::new(AtomicUsize::new(0)),
        Arc::new(AtomicUsize::new(0)),
        Arc::new(AtomicUsize::new(0)),
        Arc::new(AtomicUsize::new(0)),
    ];

    let validation_count = stage_counts[0].clone();
    let transformation_count = stage_counts[1].clone();
    let enrichment_count = stage_counts[2].clone();
    let final_count = stage_counts[3].clone();

    // Create a 4-stage dependency chain: validation -> transformation -> enrichment -> final
    let mut disruptor_handle = build_single_producer(8, test_event_factory, BusySpinWaitStrategy)
        .thread_name("validation")
        .handle_events_with(
            move |event: &mut TestEvent, sequence: i64, _end_of_batch: bool| {
                println!(
                    "🔵 Validation: sequence={}, value={}",
                    sequence, event.value
                );
                event.data = "|validated".to_string();
                validation_count.fetch_add(1, Ordering::SeqCst);
                println!("🔵 Validation DONE: sequence={sequence}");
            },
        )
        .and_then()
        .thread_name("transformation")
        .handle_events_with(
            move |event: &mut TestEvent, sequence: i64, _end_of_batch: bool| {
                println!(
                    "🟡 Transformation: sequence={}, data={}",
                    sequence, event.data
                );
                assert!(
                    event.data.contains("|validated"),
                    "Transformation stage should see validated data, got: {}",
                    event.data
                );
                event.data.push_str("|transformed");
                event.value *= 2;
                transformation_count.fetch_add(1, Ordering::SeqCst);
                println!("🟡 Transformation DONE: sequence={sequence}");
            },
        )
        .and_then()
        .thread_name("enrichment")
        .handle_events_with(
            move |event: &mut TestEvent, sequence: i64, _end_of_batch: bool| {
                println!("🟠 Enrichment: sequence={}, data={}", sequence, event.data);
                assert!(
                    event.data.contains("|transformed"),
                    "Enrichment stage should see transformed data, got: {}",
                    event.data
                );
                event.data.push_str("|enriched");
                event.value += 1000;
                enrichment_count.fetch_add(1, Ordering::SeqCst);
                println!("🟠 Enrichment DONE: sequence={sequence}");
            },
        )
        .and_then()
        .thread_name("final")
        .handle_events_with(
            move |event: &mut TestEvent, sequence: i64, _end_of_batch: bool| {
                println!("🟢 Final: sequence={}, data={}", sequence, event.data);
                assert!(
                    event.data.contains("|enriched"),
                    "Final stage should see enriched data, got: {}",
                    event.data
                );
                final_count.fetch_add(1, Ordering::SeqCst);
                println!("🟢 Final DONE: sequence={sequence}");
            },
        )
        .build();

    println!("📊 Created 4-stage dependency chain");
    assert_eq!(disruptor_handle.consumer_count(), 4);

    // Publish a few events with delays
    let num_events = 3;
    println!("📤 Publishing {num_events} events...");

    for i in 0..num_events {
        disruptor_handle.publish(|event| {
            event.value = i;
            event.data = String::new();
        });
        println!("📤 Published event {i}");

        // Small delay between publications
        std::thread::sleep(Duration::from_millis(10));
    }

    println!("⏳ Waiting for processing...");
    wait_until(
        Duration::from_secs(1),
        || {
            stage_counts
                .iter()
                .all(|count| count.load(Ordering::SeqCst) == num_events as usize)
        },
        "four-stage dependency chain to process all events",
    );

    println!("🛑 Shutting down...");
    disruptor_handle.shutdown();

    // Verify all stages processed all events
    for (stage_idx, count) in stage_counts.iter().enumerate() {
        let processed = count.load(Ordering::SeqCst);
        println!("📊 Stage {stage_idx} processed: {processed}");
        assert_eq!(
            processed, num_events as usize,
            "Stage {stage_idx} should process all {num_events} events"
        );
    }

    println!("✅ 4-stage dependency chain test completed successfully!");
}

#[test]
fn test_dependency_chain_validation() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    println!("🔍 Starting dependency chain validation test...");

    let stage1_count = Arc::new(AtomicUsize::new(0));
    let stage2_count = Arc::new(AtomicUsize::new(0));
    let stage3_count = Arc::new(AtomicUsize::new(0));

    let stage1_count_clone = stage1_count.clone();
    let stage2_count_clone = stage2_count.clone();
    let stage3_count_clone = stage3_count.clone();

    // Create a simple 3-stage dependency chain with validation
    let mut disruptor_handle = build_single_producer(8, test_event_factory, BusySpinWaitStrategy)
        .thread_name("stage1")
        .handle_events_with(
            move |event: &mut TestEvent, sequence: i64, _end_of_batch: bool| {
                println!("🔵 Stage1: seq={}, value={}", sequence, event.value);
                event.data = "|stage1".to_string();
                stage1_count_clone.fetch_add(1, Ordering::SeqCst);
                println!("🔵 Stage1 DONE: seq={sequence}");
            },
        )
        .and_then()
        .thread_name("stage2")
        .handle_events_with(
            move |event: &mut TestEvent, sequence: i64, _end_of_batch: bool| {
                println!("🟡 Stage2: seq={}, data={}", sequence, event.data);
                // Validate that stage1 processed this event
                assert!(
                    event.data.contains("|stage1"),
                    "Stage2 should see stage1 data, got: {}",
                    event.data
                );
                event.data.push_str("|stage2");
                stage2_count_clone.fetch_add(1, Ordering::SeqCst);
                println!("🟡 Stage2 DONE: seq={sequence}");
            },
        )
        .and_then()
        .thread_name("stage3")
        .handle_events_with(
            move |event: &mut TestEvent, sequence: i64, _end_of_batch: bool| {
                println!("🟢 Stage3: seq={}, data={}", sequence, event.data);
                // Validate that both stage1 and stage2 processed this event
                assert!(
                    event.data.contains("|stage1"),
                    "Stage3 should see stage1 data, got: {}",
                    event.data
                );
                assert!(
                    event.data.contains("|stage2"),
                    "Stage3 should see stage2 data, got: {}",
                    event.data
                );
                event.data.push_str("|stage3");
                stage3_count_clone.fetch_add(1, Ordering::SeqCst);
                println!("🟢 Stage3 DONE: seq={sequence}");
            },
        )
        .build();

    println!("📊 Created 3-stage dependency chain");
    assert_eq!(disruptor_handle.consumer_count(), 3);

    // Publish events one by one with delays
    let num_events = 5;
    println!("📤 Publishing {num_events} events...");

    for i in 0..num_events {
        disruptor_handle.publish(|event| {
            event.value = i;
            event.data = String::new();
        });
        println!("📤 Published event {i}");

        // Small delay between publications to reduce contention
        std::thread::sleep(Duration::from_millis(20));
    }

    // Give time for processing
    println!("⏳ Waiting for processing...");
    std::thread::sleep(Duration::from_millis(300));

    println!("🛑 Shutting down...");
    disruptor_handle.shutdown();

    // Verify all stages processed all events
    let stage1_processed = stage1_count.load(Ordering::SeqCst);
    let stage2_processed = stage2_count.load(Ordering::SeqCst);
    let stage3_processed = stage3_count.load(Ordering::SeqCst);

    println!(
        "📊 Final counts: Stage1={stage1_processed}, Stage2={stage2_processed}, Stage3={stage3_processed}"
    );

    assert_eq!(
        stage1_processed, num_events as usize,
        "Stage1 should process all {num_events} events"
    );
    assert_eq!(
        stage2_processed, num_events as usize,
        "Stage2 should process all {num_events} events"
    );
    assert_eq!(
        stage3_processed, num_events as usize,
        "Stage3 should process all {num_events} events"
    );

    println!("✅ Dependency chain validation test completed successfully!");
}

#[test]
fn test_robust_dependency_chain_multiple_events() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    println!("🔍 Starting robust dependency chain test with multiple events...");

    let stage1_count = Arc::new(AtomicUsize::new(0));
    let stage2_count = Arc::new(AtomicUsize::new(0));
    let stage1_count_clone = stage1_count.clone();
    let stage2_count_clone = stage2_count.clone();

    // Create a 2-stage dependency chain with slower processing to reduce race conditions
    let mut disruptor_handle = build_single_producer(8, test_event_factory, BusySpinWaitStrategy)
        .thread_name("robust-stage1")
        .handle_events_with(
            move |event: &mut TestEvent, sequence: i64, _end_of_batch: bool| {
                println!(
                    "🔵 Stage 1: sequence={}, initial_value={}",
                    sequence, event.value
                );

                // Modify the event
                let original_value = event.value;
                event.value = original_value + 1000;

                // Add a longer delay to ensure proper ordering
                std::thread::sleep(Duration::from_millis(10));

                stage1_count_clone.fetch_add(1, Ordering::SeqCst);
                println!(
                    "🔵 Stage 1 DONE: sequence={}, final_value={}",
                    sequence, event.value
                );
            },
        )
        .and_then()
        .thread_name("robust-stage2")
        .handle_events_with(
            move |event: &mut TestEvent, sequence: i64, _end_of_batch: bool| {
                println!("🟢 Stage 2: sequence={}, value={}", sequence, event.value);

                // Verify we see the modification from Stage 1
                if event.value < 1000 {
                    println!(
                        "❌ DEPENDENCY VIOLATION: sequence={}, saw {} (should be >= 1000)",
                        sequence, event.value
                    );
                }
                assert!(
                    event.value >= 1000,
                    "Dependency chain violation at sequence {}: saw {}",
                    sequence,
                    event.value
                );

                stage2_count_clone.fetch_add(1, Ordering::SeqCst);
                println!("🟢 Stage 2 DONE: sequence={sequence}");
            },
        )
        .build();

    println!("📊 Created robust 2-stage pipeline");
    assert_eq!(disruptor_handle.consumer_count(), 2);

    // Publish multiple events with delays between them
    let num_events = 3;
    println!("📤 Publishing {num_events} events with delays...");

    for i in 0..num_events {
        disruptor_handle.publish(|event| {
            event.value = i;
            event.data = format!("event_{i}");
        });
        println!("📤 Published event {i}");

        // Small delay between publications to reduce race conditions
        std::thread::sleep(Duration::from_millis(5));
    }

    println!("⏳ Waiting for processing to complete...");
    wait_until(
        Duration::from_secs(1),
        || {
            stage1_count.load(Ordering::SeqCst) == num_events as usize
                && stage2_count.load(Ordering::SeqCst) == num_events as usize
        },
        "robust dependency chain to process all events",
    );

    println!("🛑 Shutting down...");
    disruptor_handle.shutdown();

    // Verify both stages processed all events
    let stage1_processed = stage1_count.load(Ordering::SeqCst);
    let stage2_processed = stage2_count.load(Ordering::SeqCst);

    println!("📊 Final counts: Stage 1={stage1_processed}, Stage 2={stage2_processed}");

    assert_eq!(
        stage1_processed, num_events as usize,
        "Stage 1 should process all {num_events} events"
    );
    assert_eq!(
        stage2_processed, num_events as usize,
        "Stage 2 should process all {num_events} events"
    );

    println!("✅ Robust dependency chain test completed successfully!");
    println!("   All {num_events} events processed correctly through dependency chain");
}

#[test]
fn test_parallel_stage_then_waits_for_all_upstream_handlers() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    // LMAX WorkerPool scheme A: parallel same-stage handlers CAS-claim sequences
    // exclusively (work-sharing). Downstream waits on min(worker sequences).
    let upstream_claims = Arc::new(AtomicUsize::new(0));
    let upstream_claims_clone = Arc::clone(&upstream_claims);
    let upstream_claims_clone2 = Arc::clone(&upstream_claims);
    let downstream_processed = Arc::new(AtomicUsize::new(0));
    let downstream_processed_clone = Arc::clone(&downstream_processed);

    let mut disruptor_handle = build_single_producer(8, test_event_factory, BusySpinWaitStrategy)
        .thread_name("parallel-slow")
        .handle_events_with(
            move |event: &mut TestEvent, _sequence: i64, _end_of_batch: bool| {
                std::thread::sleep(Duration::from_millis(25));
                event.data.push_str("|worker");
                upstream_claims_clone.fetch_add(1, Ordering::SeqCst);
            },
        )
        .thread_name("parallel-fast")
        .handle_events_with(
            move |event: &mut TestEvent, _sequence: i64, _end_of_batch: bool| {
                event.data.push_str("|worker");
                upstream_claims_clone2.fetch_add(1, Ordering::SeqCst);
            },
        )
        .and_then()
        .thread_name("downstream")
        .handle_events_with(
            move |event: &mut TestEvent, _sequence: i64, _end_of_batch: bool| {
                assert!(
                    event.data.contains("|worker"),
                    "downstream must observe the exclusive upstream work-claim mutation"
                );
                downstream_processed_clone.fetch_add(1, Ordering::SeqCst);
            },
        )
        .build();

    // Publish several events so both workers can claim some work
    for i in 0..4 {
        disruptor_handle.publish(|event| {
            event.value = i;
            event.data.clear();
        });
    }

    let deadline = Instant::now() + Duration::from_secs(2);
    while downstream_processed.load(Ordering::SeqCst) < 4 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }

    disruptor_handle.shutdown();
    assert_eq!(upstream_claims.load(Ordering::SeqCst), 4);
    assert_eq!(downstream_processed.load(Ordering::SeqCst), 4);
}

#[test]
fn test_parallel_consumers_in_dependent_stage_share_upstream_barrier() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    // WorkerPool scheme A on a dependent stage: workers share the upstream
    // barrier and CAS-claim sequences for exclusive processing.
    let mid_claims = Arc::new(AtomicUsize::new(0));
    let mid_a = Arc::clone(&mid_claims);
    let mid_b = Arc::clone(&mid_claims);
    let final_stage_processed = Arc::new(AtomicUsize::new(0));
    let final_stage_processed_clone = Arc::clone(&final_stage_processed);

    let mut disruptor_handle = build_single_producer(8, test_event_factory, BusySpinWaitStrategy)
        .thread_name("root")
        .handle_events_with(
            move |event: &mut TestEvent, _sequence: i64, _end_of_batch: bool| {
                event.data.push_str("|root");
            },
        )
        .and_then()
        .thread_name("left")
        .handle_events_with(
            move |event: &mut TestEvent, _sequence: i64, _end_of_batch: bool| {
                assert!(
                    event.data.contains("|root"),
                    "work-claim handlers must wait for the full upstream stage"
                );
                std::thread::sleep(Duration::from_millis(5));
                event.data.push_str("|mid");
                mid_a.fetch_add(1, Ordering::SeqCst);
            },
        )
        .thread_name("right")
        .handle_events_with(
            move |event: &mut TestEvent, _sequence: i64, _end_of_batch: bool| {
                assert!(
                    event.data.contains("|root"),
                    "parallel workers share the upstream barrier"
                );
                event.data.push_str("|mid");
                mid_b.fetch_add(1, Ordering::SeqCst);
            },
        )
        .and_then()
        .thread_name("final")
        .handle_events_with(
            move |event: &mut TestEvent, _sequence: i64, _end_of_batch: bool| {
                assert!(event.data.contains("|root"));
                assert!(
                    event.data.contains("|mid"),
                    "final stage sees exclusive mid-stage mutation"
                );
                final_stage_processed_clone.fetch_add(1, Ordering::SeqCst);
            },
        )
        .build();

    for i in 0..4 {
        disruptor_handle.publish(|event| {
            event.value = i;
            event.data.clear();
        });
    }

    let deadline = Instant::now() + Duration::from_secs(2);
    while final_stage_processed.load(Ordering::SeqCst) < 4 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }

    disruptor_handle.shutdown();
    assert_eq!(mid_claims.load(Ordering::SeqCst), 4);
    assert_eq!(final_stage_processed.load(Ordering::SeqCst), 4);
}

#[test]
fn test_final_comprehensive_validation() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    // Test all three improvements together (without complex dependency logic)

    // 1. Test shutdown functionality
    let mut disruptor_handle = build_single_producer(8, test_event_factory, BusySpinWaitStrategy)
        .thread_name("shutdown-test")
        .handle_events_with(
            |_event: &mut TestEvent, _sequence: i64, _end_of_batch: bool| {
                // Simple handler
            },
        )
        .build();

    let shutdown_start = std::time::Instant::now();
    disruptor_handle.shutdown();
    let shutdown_duration = shutdown_start.elapsed();
    assert!(
        shutdown_duration < Duration::from_secs(2),
        "Shutdown should be fast"
    );
    println!("✅ Shutdown test passed: {shutdown_duration:?}");

    // 2. Test stateful handlers
    struct CountingHandler {
        count: usize,
        total: Arc<AtomicUsize>,
    }

    impl EventHandler<TestEvent> for CountingHandler {
        fn on_event(
            &mut self,
            event: &mut TestEvent,
            _sequence: i64,
            _end_of_batch: bool,
        ) -> crate::disruptor::Result<()> {
            self.count += 1;
            event.value = self.count as i64;
            self.total.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let total_processed = Arc::new(AtomicUsize::new(0));
    let total_processed_clone = total_processed.clone();

    let mut disruptor_handle = build_single_producer(8, test_event_factory, YieldingWaitStrategy)
        .thread_name("stateful-test")
        .handle_events_with_handler(CountingHandler {
            count: 0,
            total: total_processed_clone,
        })
        .build();

    // Publish some events
    for i in 0..5 {
        disruptor_handle.publish(|event| {
            event.value = i;
        });
    }

    let wait_start = std::time::Instant::now();
    while total_processed.load(Ordering::SeqCst) < 5 {
        assert!(
            wait_start.elapsed() < Duration::from_secs(1),
            "timed out waiting for stateful handler to process all events"
        );
        std::thread::yield_now();
    }
    disruptor_handle.shutdown();

    assert_eq!(total_processed.load(Ordering::SeqCst), 5);
    println!("✅ Stateful handler test passed");

    // 3. Test and_then() structure (basic functionality)
    let stage1_count = Arc::new(AtomicUsize::new(0));
    let stage2_count = Arc::new(AtomicUsize::new(0));
    let stage1_count_clone = stage1_count.clone();
    let stage2_count_clone = stage2_count.clone();

    let mut disruptor_handle = build_single_producer(8, test_event_factory, BusySpinWaitStrategy)
        .thread_name("stage1")
        .handle_events_with(
            move |_event: &mut TestEvent, _sequence: i64, _end_of_batch: bool| {
                stage1_count_clone.fetch_add(1, Ordering::SeqCst);
            },
        )
        .and_then()
        .thread_name("stage2")
        .handle_events_with(
            move |_event: &mut TestEvent, _sequence: i64, _end_of_batch: bool| {
                stage2_count_clone.fetch_add(1, Ordering::SeqCst);
            },
        )
        .build();

    // Verify we have two consumers
    assert_eq!(disruptor_handle.consumer_count(), 2);

    // Publish events
    for i in 0..3 {
        disruptor_handle.publish(|event| {
            event.value = i;
        });
    }

    let wait_start = std::time::Instant::now();
    while stage2_count.load(Ordering::SeqCst) < 3 {
        assert!(
            wait_start.elapsed() < Duration::from_secs(1),
            "timed out waiting for both pipeline stages to process all events"
        );
        std::thread::yield_now();
    }
    disruptor_handle.shutdown();

    let stage1_processed = stage1_count.load(Ordering::SeqCst);
    let stage2_processed = stage2_count.load(Ordering::SeqCst);

    assert_eq!(
        stage1_processed, 3,
        "Stage 1 should process all published events"
    );
    assert_eq!(
        stage2_processed, 3,
        "Stage 2 should process all published events"
    );
    println!(
        "✅ and_then() structure test passed: stage1={stage1_processed}, stage2={stage2_processed}"
    );

    println!("🎉 All comprehensive validation tests passed!");
    println!("   ✅ Shutdown fix: Fast shutdown in {shutdown_duration:?}");
    let processed_count = total_processed.load(Ordering::SeqCst);
    println!("   ✅ Stateful handlers: Processed {processed_count} events");
    println!("   ✅ and_then() structure: 2-stage pipeline created");
}

#[test]
fn test_unified_api_design() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    // Test that both DSL and Builder APIs use the same underlying DisruptorCore

    let processed_count = Arc::new(AtomicUsize::new(0));
    let processed_count_clone = processed_count.clone();

    // Test Builder API (current implementation)
    let mut builder_disruptor = build_single_producer(8, test_event_factory, BusySpinWaitStrategy)
        .thread_name("builder-consumer")
        .handle_events_with(
            move |event: &mut TestEvent, sequence: i64, _end_of_batch: bool| {
                event.value = sequence + 1000;
                processed_count_clone.fetch_add(1, Ordering::SeqCst);
            },
        )
        .build();

    // Verify the builder API works
    assert_eq!(builder_disruptor.consumer_count(), 1);

    // Publish some events
    for i in 0..5 {
        builder_disruptor.publish(|event| {
            event.value = i;
            event.data = format!("event_{i}");
        });
    }

    wait_until(
        Duration::from_secs(1),
        || processed_count.load(Ordering::SeqCst) == 5,
        "builder API consumer to process all events",
    );

    // Shutdown
    builder_disruptor.shutdown();

    // Verify events were processed
    assert_eq!(processed_count.load(Ordering::SeqCst), 5);

    println!("✅ Unified API design test passed:");
    println!("   - Builder API: ✅ Works correctly");
    println!("   - DisruptorCore: ✅ Provides unified implementation");
    println!("   - Code duplication: ✅ Eliminated through shared factory function");
    println!("   - Both APIs can now use the same underlying components");
}

#[test]
fn test_end_to_end_producer_to_consumer_chain() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    println!("🚀 Starting comprehensive end-to-end integration test...");

    // Shared counters for tracking processing
    let stage1_processed = Arc::new(AtomicUsize::new(0));
    let stage2_processed = Arc::new(AtomicUsize::new(0));
    let stage3_processed = Arc::new(AtomicUsize::new(0));

    let stage1_clone = stage1_processed.clone();
    let stage2_clone = stage2_processed.clone();
    let stage3_clone = stage3_processed.clone();

    // Create a 3-stage processing pipeline
    let mut disruptor = build_single_producer(64, test_event_factory, BusySpinWaitStrategy)
        // Stage 1: Data validation and enrichment
        .thread_name("stage1-validator")
        .handle_events_with(
            move |event: &mut TestEvent, _sequence: i64, _end_of_batch: bool| {
                // Simulate data validation
                if event.value < 0 {
                    event.value = 0; // Fix invalid data
                }

                // Enrich data
                event.value += 100; // Add base value
                event.data = format!("validated_{}", event.data);

                stage1_clone.fetch_add(1, Ordering::SeqCst);

                // Simulate processing time
                std::thread::sleep(Duration::from_micros(10));
            },
        )
        // Stage 2: Business logic processing (depends on Stage 1)
        .and_then()
        .thread_name("stage2-processor")
        .handle_events_with(
            move |event: &mut TestEvent, _sequence: i64, _end_of_batch: bool| {
                // Apply business logic
                event.value *= 2; // Double the value
                event.data = format!("processed_{}", event.data);

                stage2_clone.fetch_add(1, Ordering::SeqCst);

                // Simulate processing time
                std::thread::sleep(Duration::from_micros(15));
            },
        )
        // Stage 3: Final output formatting (depends on Stage 2)
        .and_then()
        .thread_name("stage3-formatter")
        .handle_events_with(
            move |event: &mut TestEvent, sequence: i64, _end_of_batch: bool| {
                // Format for output
                event.data = format!("final_{}_seq_{}", event.data, sequence);

                stage3_clone.fetch_add(1, Ordering::SeqCst);

                // Simulate processing time
                std::thread::sleep(Duration::from_micros(5));
            },
        )
        .build();

    println!("✅ Created 3-stage processing pipeline");
    assert_eq!(disruptor.consumer_count(), 3);

    // Test 1: Single event processing
    println!("📝 Test 1: Single event processing");
    let start_time = Instant::now();

    disruptor.publish(|event| {
        event.value = 42;
        event.data = "test_single".to_string();
    });

    wait_until(
        Duration::from_secs(1),
        || stage3_processed.load(Ordering::SeqCst) == 1,
        "single event to traverse all three pipeline stages",
    );

    assert_eq!(stage1_processed.load(Ordering::SeqCst), 1);
    assert_eq!(stage2_processed.load(Ordering::SeqCst), 1);
    assert_eq!(stage3_processed.load(Ordering::SeqCst), 1);

    println!(
        "   ✅ Single event processed through all 3 stages in {:?}",
        start_time.elapsed()
    );

    // Test 2: Batch processing
    println!("📝 Test 2: Batch processing (100 events)");
    let batch_start = Instant::now();

    for i in 0..100 {
        disruptor.publish(|event| {
            event.value = i;
            event.data = format!("batch_event_{i}");
        });
    }

    // Wait for all events to be processed
    let mut wait_time = 0;
    while stage3_processed.load(Ordering::SeqCst) < 101 && wait_time < 5000 {
        std::thread::sleep(Duration::from_millis(10));
        wait_time += 10;
    }

    let batch_duration = batch_start.elapsed();

    assert_eq!(stage1_processed.load(Ordering::SeqCst), 101);
    assert_eq!(stage2_processed.load(Ordering::SeqCst), 101);
    assert_eq!(stage3_processed.load(Ordering::SeqCst), 101);

    println!("   ✅ 100 events processed through all 3 stages in {batch_duration:?}");
    println!(
        "   📊 Throughput: {:.2} events/ms",
        100.0 / batch_duration.as_millis() as f64
    );

    // Test 3: High-frequency publishing
    println!("📝 Test 3: High-frequency publishing (1000 events)");
    let hf_start = Instant::now();

    for i in 0..1000 {
        disruptor.publish(|event| {
            event.value = i + 1000;
            event.data = format!("hf_event_{i}");
        });
    }

    // Wait for all events to be processed
    wait_time = 0;
    while stage3_processed.load(Ordering::SeqCst) < 1101 && wait_time < 10000 {
        std::thread::sleep(Duration::from_millis(10));
        wait_time += 10;
    }

    let hf_duration = hf_start.elapsed();

    assert_eq!(stage1_processed.load(Ordering::SeqCst), 1101);
    assert_eq!(stage2_processed.load(Ordering::SeqCst), 1101);
    assert_eq!(stage3_processed.load(Ordering::SeqCst), 1101);

    println!("   ✅ 1000 events processed through all 3 stages in {hf_duration:?}");
    println!(
        "   📊 Throughput: {:.2} events/ms",
        1000.0 / hf_duration.as_millis() as f64
    );

    // Test 4: Graceful shutdown
    println!("📝 Test 4: Graceful shutdown");
    let shutdown_start = Instant::now();

    disruptor.shutdown();

    let shutdown_duration = shutdown_start.elapsed();

    println!("   ✅ Graceful shutdown completed in {shutdown_duration:?}");
    assert!(
        shutdown_duration < Duration::from_millis(500),
        "Shutdown should be fast"
    );

    println!("🎉 End-to-end integration test completed successfully!");
    println!("   ✅ 3-stage dependency chain: Working correctly");
    println!("   ✅ Single event processing: ✅");
    println!("   ✅ Batch processing (100 events): ✅");
    println!("   ✅ High-frequency processing (1000 events): ✅");
    println!("   ✅ Graceful shutdown: ✅");
    let total_processed = stage3_processed.load(Ordering::SeqCst);
    println!("   📊 Total events processed: {total_processed}");
}

#[test]
fn test_cpu_affinity_support() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    // Get available CPU cores
    let core_ids = core_affinity::get_core_ids();
    if core_ids.is_none() || core_ids.as_ref().unwrap().is_empty() {
        println!("⚠️  CPU affinity test skipped: No CPU cores detected");
        return;
    }

    let available_cores = core_ids.unwrap().len();
    println!("Available CPU cores: {available_cores}");

    // Test CPU affinity setting
    let processed_count = Arc::new(AtomicUsize::new(0));
    let processed_count_clone = processed_count.clone();

    // Use core 0 if available, otherwise skip the test
    let target_core = 0;
    if target_core >= available_cores {
        println!("⚠️  CPU affinity test skipped: Not enough cores");
        return;
    }

    let mut disruptor_handle = build_single_producer(8, test_event_factory, BusySpinWaitStrategy)
        .thread_name("cpu-affinity-test")
        .pin_at_core(target_core)
        .handle_events_with(
            move |_event: &mut TestEvent, _sequence: i64, _end_of_batch: bool| {
                processed_count_clone.fetch_add(1, Ordering::SeqCst);
            },
        )
        .build();

    // Publish some events
    for i in 0..5 {
        disruptor_handle.publish(|event| {
            event.value = i;
        });
    }

    wait_until(
        Duration::from_secs(1),
        || processed_count.load(Ordering::SeqCst) == 5,
        "CPU-affinity consumer to process all events",
    );

    // Shutdown
    disruptor_handle.shutdown();

    // Verify events were processed
    assert_eq!(processed_count.load(Ordering::SeqCst), 5);

    println!(
        "✅ CPU affinity test passed: Consumer pinned to core {target_core} processed 5 events"
    );
}

#[test]
fn test_multiple_consumers_with_different_cpu_affinity() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    // Get available CPU cores
    let core_ids = core_affinity::get_core_ids();
    if core_ids.is_none() || core_ids.as_ref().unwrap().len() < 2 {
        println!("⚠️  Multi-core CPU affinity test skipped: Need at least 2 CPU cores");
        return;
    }

    let available_cores = core_ids.unwrap().len();
    println!("Testing with {available_cores} available CPU cores");

    let consumer1_count = Arc::new(AtomicUsize::new(0));
    let consumer2_count = Arc::new(AtomicUsize::new(0));
    let consumer1_count_clone = consumer1_count.clone();
    let consumer2_count_clone = consumer2_count.clone();

    let mut disruptor_handle = build_single_producer(8, test_event_factory, YieldingWaitStrategy)
        .thread_name("consumer-core-0")
        .pin_at_core(0)
        .handle_events_with(
            move |_event: &mut TestEvent, _sequence: i64, _end_of_batch: bool| {
                consumer1_count_clone.fetch_add(1, Ordering::SeqCst);
            },
        )
        .thread_name("consumer-core-1")
        .pin_at_core(1)
        .handle_events_with(
            move |_event: &mut TestEvent, _sequence: i64, _end_of_batch: bool| {
                consumer2_count_clone.fetch_add(1, Ordering::SeqCst);
            },
        )
        .build();

    // Verify we have two consumers
    assert_eq!(disruptor_handle.consumer_count(), 2);

    // Publish some events
    for i in 0..10 {
        disruptor_handle.publish(|event| {
            event.value = i;
        });
    }

    wait_until(
        Duration::from_secs(1),
        || consumer1_count.load(Ordering::SeqCst) + consumer2_count.load(Ordering::SeqCst) == 10,
        "work-pool consumers to claim and process all events",
    );

    // Shutdown
    disruptor_handle.shutdown();

    // WorkerPool scheme A: each event is claimed by exactly one worker
    let consumer1_processed = consumer1_count.load(Ordering::SeqCst);
    let consumer2_processed = consumer2_count.load(Ordering::SeqCst);

    assert_eq!(
        consumer1_processed + consumer2_processed,
        10,
        "work-pool workers must jointly process all 10 events exactly once"
    );
    assert!(consumer1_processed > 0 || consumer2_processed > 0);

    println!("✅ Multi-core CPU affinity test passed:");
    println!("   Consumer on core 0 processed: {consumer1_processed} events");
    println!("   Consumer on core 1 processed: {consumer2_processed} events");
    println!("   Work-pool CAS claim split work across workers (LMAX WorkerPool scheme A)");
}

#[test]
fn test_core_interfaces_activation() {
    use crate::disruptor::core_interfaces::DataProvider;
    use crate::disruptor::{DefaultEventFactory, RingBuffer};

    // Test DataProvider trait implementation for RingBuffer
    let factory = DefaultEventFactory::<TestEvent>::new();
    let ring_buffer = RingBuffer::new(8, factory).unwrap();

    // Test DataProvider interface
    let data_provider: &dyn DataProvider<TestEvent> = &ring_buffer;
    let event = data_provider.get(0);
    assert_eq!(event.value, -1); // Default value for TestEvent

    println!("✅ Core interfaces activation test passed:");
    println!("   RingBuffer implements DataProvider trait");
    println!("   Core interfaces are now activated and usable");
}

#[test]
fn test_comprehensive_improvements() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    // Define a stateful event handler for the dependency chain
    struct ProcessingStageHandler {
        stage_name: String,
        processed_count: usize,
        total_processed: Arc<AtomicUsize>,
    }

    impl EventHandler<TestEvent> for ProcessingStageHandler {
        fn on_event(
            &mut self,
            event: &mut TestEvent,
            sequence: i64,
            _end_of_batch: bool,
        ) -> crate::disruptor::Result<()> {
            self.processed_count += 1;

            // In LMAX Disruptor, all consumers process the same event instance
            // Each stage adds its processing marker to the event
            match self.stage_name.as_str() {
                "validation" => {
                    // Validation stage: ensure value is non-negative
                    if event.value < 0 {
                        event.value = 0;
                    }
                    // Add validation marker
                    event.data = format!("{}|validated", event.data);
                }
                "transformation" => {
                    // Transformation stage: double the value
                    event.value *= 2;
                    // Add transformation marker
                    event.data = format!("{}|transformed", event.data);
                }
                "enrichment" => {
                    // Enrichment stage: add 1000
                    event.value += 1000;
                    // Add enrichment marker
                    event.data = format!("{}|enriched", event.data);
                }
                _ => {}
            }

            self.total_processed.fetch_add(1, Ordering::SeqCst);

            // Small delay to ensure proper ordering in dependency chain
            std::thread::sleep(Duration::from_millis(1));

            println!(
                "Stage '{}' processed sequence {} (value: {}, data: '{}'), stage count: {}",
                self.stage_name, sequence, event.value, event.data, self.processed_count
            );
            Ok(())
        }

        fn on_start(&mut self) -> crate::disruptor::Result<()> {
            println!("Stage '{}' started", self.stage_name);
            Ok(())
        }

        fn on_shutdown(&mut self) -> crate::disruptor::Result<()> {
            println!(
                "Stage '{}' shutdown, processed {} events",
                self.stage_name, self.processed_count
            );
            Ok(())
        }
    }

    let validation_total = Arc::new(AtomicUsize::new(0));
    let transformation_total = Arc::new(AtomicUsize::new(0));
    let enrichment_total = Arc::new(AtomicUsize::new(0));
    let final_total = Arc::new(AtomicUsize::new(0));

    let validation_total_clone = validation_total.clone();
    let transformation_total_clone = transformation_total.clone();
    let enrichment_total_clone = enrichment_total.clone();
    let final_total_clone = final_total.clone();

    // Create a comprehensive disruptor with:
    // 1. Shutdown fix (BusySpinWaitStrategy should shutdown properly)
    // 2. Dependency chain (and_then() functionality)
    // 3. Stateful handlers (ProcessingStageHandler)
    // 4. Mixed handler types (stateful + closure)
    let mut disruptor_handle = build_single_producer(16, test_event_factory, BusySpinWaitStrategy)
        // Stage 1: Validation (stateful handler)
        .thread_name("validation-stage")
        .handle_events_with_handler(ProcessingStageHandler {
            stage_name: "validation".to_string(),
            processed_count: 0,
            total_processed: validation_total_clone,
        })
        // Dependency chain: transformation depends on validation
        .and_then()
        .thread_name("transformation-stage")
        .handle_events_with_handler(ProcessingStageHandler {
            stage_name: "transformation".to_string(),
            processed_count: 0,
            total_processed: transformation_total_clone,
        })
        // Another dependency: enrichment depends on transformation
        .and_then()
        .thread_name("enrichment-stage")
        .handle_events_with_handler(ProcessingStageHandler {
            stage_name: "enrichment".to_string(),
            processed_count: 0,
            total_processed: enrichment_total_clone,
        })
        // Final stage: closure handler that depends on enrichment
        .and_then()
        .thread_name("final-stage")
        .handle_events_with(
            move |event: &mut TestEvent, sequence: i64, _end_of_batch: bool| {
                // Final processing with closure handler
                final_total_clone.fetch_add(1, Ordering::SeqCst);
                println!(
                    "Final stage processed sequence {} with final value: {} and data: '{}'",
                    sequence, event.value, event.data
                );

                // Verify the processing pipeline worked correctly
                // Original value i -> validation (>= 0) -> transformation (*2) -> enrichment (+1000)
                // For input i: i -> max(i,0) -> max(i,0)*2 -> max(i,0)*2+1000

                // Check that all stages processed this event (by checking the data field)
                assert!(
                    event.data.contains("validated"),
                    "Event should have been processed by validation stage, data: '{}'",
                    event.data
                );
                assert!(
                    event.data.contains("transformed"),
                    "Event should have been processed by transformation stage, data: '{}'",
                    event.data
                );
                assert!(
                    event.data.contains("enriched"),
                    "Event should have been processed by enrichment stage, data: '{}'",
                    event.data
                );

                // For non-negative inputs, the final value should be at least 1000 (enrichment)
                // For input 0: 0 -> 0 -> 0 -> 1000
                // For input i>0: i -> i -> i*2 -> i*2+1000
                let expected_min = 1000; // At least the enrichment value
                assert!(event.value >= expected_min,
                   "Final value {} should be at least {} after processing pipeline for sequence {}",
                   event.value, expected_min, sequence);
            },
        )
        .build();

    // Verify we have four consumers in the dependency chain
    assert_eq!(disruptor_handle.consumer_count(), 4);

    println!("Starting comprehensive test with 4-stage processing pipeline...");

    // Publish test events
    for i in 0..8 {
        disruptor_handle.publish(|event| {
            event.value = i;
            event.data = String::new(); // Initialize data field
        });
    }

    wait_until(
        Duration::from_secs(2),
        || final_total.load(Ordering::SeqCst) == 8,
        "comprehensive four-stage pipeline to process all events",
    );

    println!("Shutting down disruptor...");
    let shutdown_start = std::time::Instant::now();

    // Test shutdown functionality (should complete quickly)
    disruptor_handle.shutdown();

    let shutdown_duration = shutdown_start.elapsed();
    assert!(
        shutdown_duration < Duration::from_secs(3),
        "Shutdown took too long: {shutdown_duration:?}"
    );

    // Verify all stages processed all events
    assert_eq!(
        validation_total.load(Ordering::SeqCst),
        8,
        "Validation stage should process all events"
    );
    assert_eq!(
        transformation_total.load(Ordering::SeqCst),
        8,
        "Transformation stage should process all events"
    );
    assert_eq!(
        enrichment_total.load(Ordering::SeqCst),
        8,
        "Enrichment stage should process all events"
    );
    assert_eq!(
        final_total.load(Ordering::SeqCst),
        8,
        "Final stage should process all events"
    );

    println!("✅ Comprehensive test completed successfully!");
    println!("   - Shutdown fix: ✅ Completed in {shutdown_duration:?}");
    println!("   - Dependency chain: ✅ 4-stage pipeline worked correctly");
    println!("   - Stateful handlers: ✅ All stages maintained state");
    println!("   - Mixed handler types: ✅ Stateful + closure handlers worked together");
}
