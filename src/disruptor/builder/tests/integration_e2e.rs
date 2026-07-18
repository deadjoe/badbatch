use super::super::*;
use super::common::{test_event_factory, wait_until, TestEvent};
use crate::disruptor::wait_strategy::{BusySpinWaitStrategy, YieldingWaitStrategy};
use crate::disruptor::EventHandler;
use std::sync::{atomic::Ordering, Arc};
use std::time::Duration;

macro_rules! println {
    ($($arg:tt)*) => {
        crate::test_log!($($arg)*);
    };
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

    let _ = disruptor.publish(|event| {
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
        let _ = disruptor.publish(|event| {
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
        let _ = disruptor.publish(|event| {
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
