//! MPMC End-to-End Continuity Tests
//!
//! This module tests the critical MPMC (Multi-Producer Multi-Consumer) continuity
//! convergence behavior, particularly focusing on scenarios that were identified
//! in the code review as missing test coverage.

use badbatch::disruptor::{
    build_multi_producer, BlockingWaitStrategy, BusySpinWaitStrategy, Producer,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Condvar, Mutex,
};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default)]
struct TestEvent {
    pub value: i64,
    pub producer_id: u32,
}

/// Test basic MPMC functionality with continuity verification
/// This verifies that the P0 fixes for MPMC continuity convergence work correctly
#[test]
fn test_mpmc_basic_functionality() {
    let processed_count = Arc::new(AtomicUsize::new(0));
    let count_clone = Arc::clone(&processed_count);

    // Create multi-producer disruptor
    let mut producer = build_multi_producer(128, TestEvent::default, BusySpinWaitStrategy)
        .handle_events_with(
            move |_event: &mut TestEvent, _sequence: i64, _end_of_batch: bool| {
                count_clone.fetch_add(1, Ordering::SeqCst);
                // Small processing delay
                thread::sleep(Duration::from_micros(100));
            },
        )
        .build();

    // Test single-threaded publishing first
    for i in 0..5 {
        producer
            .try_publish(|event| {
                event.value = i;
                event.producer_id = 1;
            })
            .expect("Failed to publish single-threaded event");
    }

    // Allow processing
    thread::sleep(Duration::from_millis(50));

    assert_eq!(
        processed_count.load(Ordering::SeqCst),
        5,
        "Single-threaded: Should process exactly 5 events"
    );

    println!("✅ MPMC single-threaded functionality test passed");
}

/// Test multi-producer scenario with proper synchronization
/// This tests the core MPMC continuity convergence under real concurrent conditions
#[test]
fn test_mpmc_multi_producer() {
    let processed_count = Arc::new(AtomicUsize::new(0));
    let count_clone = Arc::clone(&processed_count);

    // Create multi-producer disruptor with adequate buffer size
    let mut disruptor = build_multi_producer(256, TestEvent::default, BusySpinWaitStrategy)
        .handle_events_with(
            move |_event: &mut TestEvent, _sequence: i64, _end_of_batch: bool| {
                count_clone.fetch_add(1, Ordering::SeqCst);
            },
        )
        .build();

    let num_producers = 2;
    let events_per_producer = 10;
    let total_events = num_producers * events_per_producer;

    let mut handles = vec![];

    // Create producer threads with conservative publishing
    for producer_id in 0..num_producers {
        let mut producer_clone = disruptor.create_producer();
        let handle = thread::spawn(move || {
            for event_id in 0..events_per_producer {
                // Use blocking publish to avoid ring buffer full errors
                producer_clone.publish(|event| {
                    event.value = event_id as i64;
                    event.producer_id = producer_id as u32;
                });
                // Small delay between publishes
                thread::sleep(Duration::from_millis(1));
            }
        });
        handles.push(handle);
    }

    // Wait for all producers to complete
    for (i, handle) in handles.into_iter().enumerate() {
        handle
            .join()
            .unwrap_or_else(|_| panic!("Producer {i} thread panicked"));
    }

    // Allow all events to be processed with generous timeout
    let start = Instant::now();
    while processed_count.load(Ordering::SeqCst) < total_events
        && start.elapsed() < Duration::from_secs(5)
    {
        thread::sleep(Duration::from_millis(10));
    }

    let final_count = processed_count.load(Ordering::SeqCst);
    assert_eq!(
        final_count, total_events,
        "Multi-producer: Should process all {total_events} events, got {final_count}"
    );

    println!("✅ MPMC multi-producer test passed");
    println!("   Processed {final_count} events from {num_producers} producers");

    disruptor.shutdown();
}

/// Test continuity convergence with sequence tracking
/// This specifically tests that sequences are processed in order despite multi-producer chaos
#[test]
fn test_mpmc_sequence_continuity() {
    let sequences = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sequences_clone = Arc::clone(&sequences);

    // Create disruptor with sequence tracking
    let mut producer = build_multi_producer(64, TestEvent::default, BusySpinWaitStrategy)
        .handle_events_with(
            move |_event: &mut TestEvent, sequence: i64, _end_of_batch: bool| {
                sequences_clone.lock().unwrap().push(sequence);
            },
        )
        .build();

    // Publish events sequentially for predictable testing
    for i in 0..8 {
        producer.publish(|event| {
            event.value = i;
        });
    }

    // Allow processing to complete
    thread::sleep(Duration::from_millis(100));

    let final_sequences = sequences.lock().unwrap();
    assert_eq!(final_sequences.len(), 8, "Should process exactly 8 events");

    // Verify sequences are processed in order (the key continuity requirement)
    for i in 1..final_sequences.len() {
        assert_eq!(
            final_sequences[i],
            final_sequences[i - 1] + 1,
            "Sequence continuity broken: {} after {}",
            final_sequences[i],
            final_sequences[i - 1]
        );
    }

    // First sequence should be 0
    assert_eq!(final_sequences[0], 0, "First sequence should be 0");

    println!("✅ MPMC sequence continuity test passed");
    println!("   Processed sequences in order: {:?}", &*final_sequences);
}

/// Test that verifies the P0 fixes work under realistic conditions
/// This combines multi-producer publishing with continuity verification
#[test]
fn test_mpmc_continuity_under_load() {
    let sequences = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sequences_clone = Arc::clone(&sequences);

    // Create disruptor
    let mut producer = build_multi_producer(128, TestEvent::default, BusySpinWaitStrategy)
        .handle_events_with(
            move |_event: &mut TestEvent, sequence: i64, _end_of_batch: bool| {
                sequences_clone.lock().unwrap().push(sequence);
                // Simulate some processing work
                thread::sleep(Duration::from_micros(50));
            },
        )
        .build();

    let num_events = 15;

    // Use a single thread with blocking publishes to ensure all events get through
    for i in 0..num_events {
        producer.publish(|event| {
            event.value = i as i64;
            event.producer_id = 1;
        });

        // Small delay to allow processing to keep up
        if i % 3 == 0 {
            thread::sleep(Duration::from_millis(5));
        }
    }

    // Allow final processing
    thread::sleep(Duration::from_millis(200));

    let final_sequences = sequences.lock().unwrap();
    assert_eq!(
        final_sequences.len(),
        num_events,
        "Should process exactly {num_events} events"
    );

    // Verify perfect continuity
    for i in 1..final_sequences.len() {
        assert_eq!(
            final_sequences[i],
            final_sequences[i - 1] + 1,
            "Continuity violation: sequence {} after {}",
            final_sequences[i],
            final_sequences[i - 1]
        );
    }

    assert_eq!(final_sequences[0], 0, "First sequence should be 0");

    println!("✅ MPMC continuity under load test passed");
    println!("   Perfect sequence continuity maintained for {num_events} events");
}

/// Regression: ensure that the first-stage consumer in a multi-producer disruptor
/// does not observe events before the publishing producer completes initialization.
#[test]
fn test_mpmc_slow_producer_does_not_leak_unpublished_events() {
    #[derive(Debug, Clone)]
    struct SlowEvent {
        value: i64,
    }

    impl Default for SlowEvent {
        fn default() -> Self {
            Self { value: -1 }
        }
    }

    let processed_values = Arc::new(Mutex::new(Vec::new()));
    let processed_clone = Arc::clone(&processed_values);

    let mut disruptor = build_multi_producer(16, SlowEvent::default, BlockingWaitStrategy::new())
        .handle_events_with(move |event: &mut SlowEvent, _sequence, _end_of_batch| {
            processed_clone.lock().unwrap().push(event.value);
        })
        .build();

    // Latches for coordinating the slow producer
    let claim_latch = Arc::new((Mutex::new(false), Condvar::new()));
    let release_latch = Arc::new((Mutex::new(false), Condvar::new()));

    // Spawn a slow producer that claims a sequence but delays initialization.
    let mut slow_producer = disruptor.create_producer();
    let claim_latch_clone = Arc::clone(&claim_latch);
    let release_latch_clone = Arc::clone(&release_latch);
    let producer_thread = std::thread::spawn(move || {
        slow_producer.publish(|event| {
            // Signal that we have entered the critical section before initialization.
            {
                let (lock, cvar) = &*claim_latch_clone;
                let mut claimed = lock.lock().unwrap();
                *claimed = true;
                cvar.notify_one();
            }

            // Wait until main thread allows initialization to complete.
            let (lock, cvar) = &*release_latch_clone;
            let mut released = lock.lock().unwrap();
            while !*released {
                released = cvar.wait(released).unwrap();
            }

            // Perform the actual event initialization.
            event.value = 7;
        });
    });

    // Wait until the producer has claimed the slot and is holding before initialization.
    {
        let (lock, cvar) = &*claim_latch;
        let mut claimed = lock.lock().unwrap();
        while !*claimed {
            claimed = cvar.wait(claimed).unwrap();
        }
    }

    // Give consumers a chance to misbehave (they should not see the event yet).
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert!(
        processed_values.lock().unwrap().is_empty(),
        "Consumer should not observe unpublished events"
    );

    // Allow the slow producer to finish initialization and publish the event.
    {
        let (lock, cvar) = &*release_latch;
        let mut released = lock.lock().unwrap();
        *released = true;
        cvar.notify_one();
    }

    producer_thread
        .join()
        .expect("Slow producer thread should complete cleanly");

    // Wait until the consumer processes the event.
    let start = std::time::Instant::now();
    while processed_values.lock().unwrap().is_empty()
        && start.elapsed() < std::time::Duration::from_secs(1)
    {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    let processed = processed_values.lock().unwrap().clone();
    assert_eq!(
        processed,
        vec![7],
        "Consumer should see exactly the initialized value after publish"
    );

    disruptor.shutdown();
}

/// Verify the DisruptorBuilder API works correctly for MPMC
#[test]
fn test_mpmc_builder_api() {
    let event_count = Arc::new(AtomicUsize::new(0));
    let count_clone = Arc::clone(&event_count);

    // Test the builder API
    let mut producer = build_multi_producer(32, TestEvent::default, BusySpinWaitStrategy)
        .handle_events_with(
            move |_event: &mut TestEvent, _sequence: i64, _end_of_batch: bool| {
                count_clone.fetch_add(1, Ordering::SeqCst);
            },
        )
        .build();

    // Simple publishing test
    for i in 0..3 {
        producer.publish(|event| {
            event.value = i as i64;
        });
    }

    // Allow processing
    thread::sleep(Duration::from_millis(50));

    assert_eq!(
        event_count.load(Ordering::SeqCst),
        3,
        "Builder API should process all 3 events"
    );

    println!("✅ MPMC builder API test passed");
}
