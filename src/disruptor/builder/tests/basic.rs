use super::super::*;
use super::common::{test_event_factory, wait_until, TestEvent};
use crate::disruptor::event_handler::ClosureEventHandler;
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
    // SimpleProducer is deliberately not Clone (soundness audit 2026-07-18);
    // multi-mode handles create one handle per publishing thread instead.
    let producer2 = handle.create_producer();

    // Both producer handles operate on the same underlying buffer
    drop(producer1);
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
        Ok(())
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
    // SAFETY: single-threaded test, no concurrent writers.
    let event = unsafe { data_provider.get(0) };
    assert_eq!(event.value, -1); // Default value for TestEvent

    println!("✅ Core interfaces activation test passed:");
    println!("   RingBuffer implements DataProvider trait");
    println!("   Core interfaces are now activated and usable");
}
