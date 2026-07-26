use super::super::*;
use super::common::{test_event_factory, wait_until, TestEvent};
use crate::disruptor::producer::Producer;
use crate::disruptor::wait_strategy::{BusySpinWaitStrategy, YieldingWaitStrategy};
use crate::disruptor::Sequencer;
use std::sync::{
    atomic::{AtomicI64, Ordering},
    Arc, Mutex,
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
    let _ = disruptor_handle.publish(|event| {
        event.value = 42;
        event.data = "test".to_string();
    });

    // Test batch publishing
    let _ = disruptor_handle.batch_publish(3, |batch| {
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
        let _ = disruptor_handle.publish(|event| {
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
fn test_into_producer_unblocks_backpressured_producer_after_gating_removal() {
    use std::sync::mpsc::RecvTimeoutError;

    let (handler_entered_tx, handler_entered_rx) = std::sync::mpsc::channel();
    let (release_handler_tx, release_handler_rx) = std::sync::mpsc::channel();
    let release_handler_rx = Arc::new(Mutex::new(release_handler_rx));
    let release_in_handler = Arc::clone(&release_handler_rx);

    let mut handle = build_multi_producer(4, test_event_factory, BusySpinWaitStrategy)
        .handle_events_with(move |_event: &mut TestEvent, _seq, _eob| {
            handler_entered_tx.send(()).unwrap();
            release_in_handler.lock().unwrap().recv().unwrap();
        })
        .build();
    let shutdown = Arc::clone(&handle.core.shutdown_flag);
    let sequencer = handle.core.sequencer.clone();
    let mut blocked_producer = handle.create_producer();

    // Publish the first event alone so the consumer's current batch ends at
    // sequence 0, then hold it inside that handler.
    handle
        .publish(|event| {
            event.value = 0;
        })
        .unwrap();
    handler_entered_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("consumer must enter the first handler");
    // Fill the remaining slots while the consumer is held, leaving its gating
    // sequence at -1 and the producer cursor at 3.
    for value in 1..4 {
        handle.publish(|event| event.value = value).unwrap();
    }

    // A second full-buffer batch needs the consumer to reach sequence 3 and
    // therefore blocks on the registered gating snapshot.
    let (claim_started_tx, claim_started_rx) = std::sync::mpsc::channel();
    let (claim_result_tx, claim_result_rx) = std::sync::mpsc::channel();
    let publisher = std::thread::spawn(move || {
        claim_started_tx.send(()).unwrap();
        let result = blocked_producer.batch_publish(4, |events| {
            for event in events {
                event.value = 99;
            }
        });
        claim_result_tx.send(result).unwrap();
    });
    claim_started_rx.recv().unwrap();
    match claim_result_rx.recv_timeout(Duration::from_millis(20)) {
        Err(RecvTimeoutError::Timeout) => {}
        unexpected => {
            shutdown.store(true, Ordering::Release);
            release_handler_tx.send(()).unwrap();
            let _standalone = handle.into_producer();
            publisher.join().unwrap();
            panic!("producer was not backpressured: {unexpected:?}");
        }
    }

    // Move lifecycle ownership to another thread. Wait until into_producer has
    // set the stop flag before releasing the first handler, so the consumer
    // cannot drain the rest of the batch and unblock the claim by normal
    // progress. The only sufficient release is gating removal after join.
    let into_producer = std::thread::spawn(move || handle.into_producer());
    wait_until(
        Duration::from_secs(1),
        || shutdown.load(Ordering::Acquire),
        "into_producer shutdown flag",
    );
    release_handler_tx.send(()).unwrap();
    let mut standalone = into_producer.join().unwrap();

    let claimed = match claim_result_rx.recv_timeout(Duration::from_secs(1)) {
        Ok(result) => result.expect("gating removal must unblock the claim"),
        Err(error) => {
            // Keep a failing regression from stranding a spinning publisher.
            sequencer.close();
            publisher.join().unwrap();
            panic!("backpressured producer did not observe gating removal: {error}");
        }
    };
    assert_eq!(claimed, 7);
    publisher.join().unwrap();

    // The producer returned by into_producer remains open and can continue
    // wrapping freely after all consumer gating has been removed.
    for expected in 8..16 {
        assert_eq!(
            standalone
                .try_publish(|event| event.value = expected)
                .unwrap(),
            expected
        );
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
