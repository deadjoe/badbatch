use super::super::*;
use super::common::{test_event_factory, wait_until, TestEvent};
use crate::disruptor::producer::Producer;
use crate::disruptor::wait_strategy::{BusySpinWaitStrategy, YieldingWaitStrategy};
use std::sync::{
    atomic::{AtomicI64, Ordering},
    Arc,
};
use std::time::Duration;

macro_rules! println {
    ($($arg:tt)*) => {
        crate::test_log!($($arg)*);
    };
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
