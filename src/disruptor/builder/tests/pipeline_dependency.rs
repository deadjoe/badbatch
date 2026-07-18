use super::super::*;
use super::common::{test_event_factory, wait_until, TestEvent};
use crate::disruptor::producer::Producer;
use crate::disruptor::wait_strategy::{BusySpinWaitStrategy, YieldingWaitStrategy};
use crate::disruptor::EventHandler;
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
        let _ = disruptor_handle.publish(|event| {
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
        let _ = disruptor_handle.publish(|event| {
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
        let _ = producer1.publish(|event| {
            event.value = i;
        });
        let _ = producer2.publish(|event| {
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
        let _ = disruptor_handle.publish(|event| {
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
        let _ = disruptor_handle.publish(|event| {
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
    let _ = disruptor_handle.publish(|event| {
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
        let _ = disruptor_handle.publish(|event| {
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
        let _ = disruptor_handle.publish(|event| {
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
        let _ = disruptor_handle.publish(|event| {
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
