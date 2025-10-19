#![allow(
    missing_docs,
    clippy::all,
    clippy::pedantic,
    clippy::nursery,
    clippy::cargo
)]

// Exact reproduction of MPSC benchmark issue
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

use badbatch::disruptor::{
    event_translator::ClosureEventTranslator, BusySpinWaitStrategy, DefaultEventFactory, Disruptor,
    EventHandler, ProducerType, Result as DisruptorResult,
};

#[derive(Debug, Default, Clone, Copy)]
struct ReproEvent {
    producer_id: usize,
    value: i64,
    sequence: i64,
}

struct ReproHandler {
    count: Arc<AtomicI64>,
}

impl ReproHandler {
    fn new() -> Self {
        Self {
            count: Arc::new(AtomicI64::new(0)),
        }
    }

    fn get_count(&self) -> Arc<AtomicI64> {
        self.count.clone()
    }
}

impl EventHandler<ReproEvent> for ReproHandler {
    fn on_event(
        &mut self,
        event: &mut ReproEvent,
        sequence: i64,
        _end_of_batch: bool,
    ) -> DisruptorResult<()> {
        badbatch::test_log!(
            "REPRO: Handler processed producer_id={}, value={}, sequence={}",
            event.producer_id,
            event.value,
            sequence
        );
        self.count.fetch_add(1, Ordering::Release);
        Ok(())
    }
}

/// Producer thread that mimics the MPSC benchmark exactly
struct ReproProducer {
    join_handle: Option<thread::JoinHandle<bool>>,
    stop_flag: Arc<AtomicBool>,
}

impl ReproProducer {
    fn new(
        producer_id: usize,
        burst_size: u64,
        disruptor: Arc<Disruptor<ReproEvent>>,
        start_barrier: Arc<Barrier>,
        stop_flag: Arc<AtomicBool>,
    ) -> Self {
        let stop_flag_clone = stop_flag.clone();

        let join_handle = thread::spawn(move || {
            badbatch::test_log!("REPRO: Producer {producer_id} waiting at barrier");
            start_barrier.wait();
            badbatch::test_log!(
                "REPRO: Producer {producer_id} starting burst of {burst_size} events"
            );

            for i in 1..=burst_size {
                if stop_flag_clone.load(Ordering::Acquire) {
                    badbatch::test_log!("REPRO: Producer {producer_id} stopping due to stop flag");
                    return false;
                }

                badbatch::test_log!("REPRO: Producer {producer_id} publishing event {i}");
                let result = disruptor.publish_event(ClosureEventTranslator::new(
                    move |event: &mut ReproEvent, seq: i64| {
                        event.producer_id = producer_id;
                        event.value = i as i64;
                        event.sequence = seq;
                    },
                ));

                match result {
                    Ok(_) => {
                        badbatch::test_log!(
                            "REPRO: Producer {producer_id} successfully published event {i}"
                        )
                    }
                    Err(e) => {
                        badbatch::test_log!(
                            "REPRO: Producer {producer_id} failed to publish event {i}: {e:?}"
                        );
                        return false;
                    }
                }
            }
            badbatch::test_log!("REPRO: Producer {producer_id} finished all {burst_size} events");
            true
        });

        Self {
            join_handle: Some(join_handle),
            stop_flag,
        }
    }

    fn stop_and_join(&mut self) -> bool {
        self.stop_flag.store(true, Ordering::Release);
        if let Some(handle) = self.join_handle.take() {
            handle.join().unwrap_or_default()
        } else {
            false
        }
    }
}

#[test]
fn test_mpsc_exact_repro() {
    badbatch::test_log!("=== MPSC Exact Reproduction Test ===");

    // Exact same config as MPSC benchmark
    const PRODUCER_COUNT: usize = 3;
    const BUFFER_SIZE: usize = 2048; // Same as benchmark!
    const BURST_SIZE: u64 = 10;
    const EXPECTED_EVENTS: i64 = (BURST_SIZE * PRODUCER_COUNT as u64) as i64;

    let factory = DefaultEventFactory::<ReproEvent>::new();
    let handler = ReproHandler::new();
    let counter = handler.get_count();

    badbatch::test_log!("REPRO: Creating disruptor with buffer size {BUFFER_SIZE}");
    let mut disruptor = Disruptor::new(
        factory,
        BUFFER_SIZE, // This triggers bitmap path!
        ProducerType::Multi,
        Box::new(BusySpinWaitStrategy::new()),
    )
    .unwrap()
    .handle_events_with(handler)
    .build();

    badbatch::test_log!("REPRO: Starting disruptor");
    disruptor.start().unwrap();
    let disruptor_arc = Arc::new(disruptor);

    badbatch::test_log!("REPRO: Creating {PRODUCER_COUNT} producers with barrier synchronization");
    let start_barrier = Arc::new(Barrier::new(PRODUCER_COUNT));
    let stop_flag = Arc::new(AtomicBool::new(false));

    let mut producers: Vec<ReproProducer> = (0..PRODUCER_COUNT)
        .map(|producer_id| {
            ReproProducer::new(
                producer_id,
                BURST_SIZE,
                disruptor_arc.clone(),
                start_barrier.clone(),
                stop_flag.clone(),
            )
        })
        .collect();

    badbatch::test_log!("REPRO: All producers created, waiting for completion...");

    // Wait for completion with timeout
    let start_time = Instant::now();
    let timeout = Duration::from_secs(10);
    let mut last_count = 0;
    let mut stall_count = 0;

    loop {
        let current_count = counter.load(Ordering::Acquire);
        if current_count >= EXPECTED_EVENTS {
            badbatch::test_log!("REPRO: ✓ All {current_count} events processed successfully!");
            break;
        }

        if start_time.elapsed() > timeout {
            badbatch::test_log!("REPRO: ✗ Test timed out - {current_count} events processed, expected {EXPECTED_EVENTS}");
            break;
        }

        // Check for stalls
        if current_count == last_count {
            stall_count += 1;
            if stall_count > 1000 {
                badbatch::test_log!("REPRO: WARNING - Test appears stalled at {current_count} events (expected {EXPECTED_EVENTS})");
                stall_count = 0;
            }
        } else {
            stall_count = 0;
        }
        last_count = current_count;

        thread::sleep(Duration::from_millis(1));
    }

    // Stop all producers
    badbatch::test_log!("REPRO: Stopping all producers");
    let mut all_success = true;
    for (i, producer) in producers.iter_mut().enumerate() {
        let success = producer.stop_and_join();
        if !success {
            badbatch::test_log!("REPRO: Producer {i} failed");
            all_success = false;
        }
    }

    let final_count = counter.load(Ordering::Acquire);
    badbatch::test_log!(
        "REPRO: Final result - {final_count} events processed (expected {EXPECTED_EVENTS})"
    );

    // Cleanup
    if let Ok(mut disruptor) = Arc::try_unwrap(disruptor_arc) {
        disruptor.shutdown().unwrap();
    }

    // The test should pass if we processed all expected events
    assert_eq!(
        final_count, EXPECTED_EVENTS,
        "Should process all {EXPECTED_EVENTS} events"
    );
    assert!(all_success, "All producers should complete successfully");
}
