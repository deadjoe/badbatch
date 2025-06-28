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
        println!("REPRO: Handler processed producer_id={}, value={}, sequence={}", 
                event.producer_id, event.value, sequence);
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
            println!("REPRO: Producer {} waiting at barrier", producer_id);
            start_barrier.wait();
            println!("REPRO: Producer {} starting burst of {} events", producer_id, burst_size);

            for i in 1..=burst_size {
                if stop_flag_clone.load(Ordering::Acquire) {
                    println!("REPRO: Producer {} stopping due to stop flag", producer_id);
                    return false;
                }

                println!("REPRO: Producer {} publishing event {}", producer_id, i);
                let result = disruptor.publish_event(ClosureEventTranslator::new(
                    move |event: &mut ReproEvent, seq: i64| {
                        event.producer_id = producer_id;
                        event.value = i as i64;
                        event.sequence = seq;
                    },
                ));

                match result {
                    Ok(_) => println!("REPRO: Producer {} successfully published event {}", producer_id, i),
                    Err(e) => {
                        println!("REPRO: Producer {} failed to publish event {}: {:?}", producer_id, i, e);
                        return false;
                    }
                }
            }
            println!("REPRO: Producer {} finished all {} events", producer_id, burst_size);
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
            match handle.join() {
                Ok(success) => success,
                Err(_) => false,
            }
        } else {
            false
        }
    }
}

#[test]
fn test_mpsc_exact_repro() {
    println!("=== MPSC Exact Reproduction Test ===");

    // Exact same config as MPSC benchmark
    const PRODUCER_COUNT: usize = 3;
    const BUFFER_SIZE: usize = 2048; // Same as benchmark!
    const BURST_SIZE: u64 = 10;
    const EXPECTED_EVENTS: i64 = (BURST_SIZE * PRODUCER_COUNT as u64) as i64;

    let factory = DefaultEventFactory::<ReproEvent>::new();
    let handler = ReproHandler::new();
    let counter = handler.get_count();

    println!("REPRO: Creating disruptor with buffer size {}", BUFFER_SIZE);
    let mut disruptor = Disruptor::new(
        factory,
        BUFFER_SIZE, // This triggers bitmap path!
        ProducerType::Multi,
        Box::new(BusySpinWaitStrategy::new()),
    )
    .unwrap()
    .handle_events_with(handler)
    .build();

    println!("REPRO: Starting disruptor");
    disruptor.start().unwrap();
    let disruptor_arc = Arc::new(disruptor);

    println!("REPRO: Creating {} producers with barrier synchronization", PRODUCER_COUNT);
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

    println!("REPRO: All producers created, waiting for completion...");

    // Wait for completion with timeout
    let start_time = Instant::now();
    let timeout = Duration::from_secs(10);
    let mut last_count = 0;
    let mut stall_count = 0;

    loop {
        let current_count = counter.load(Ordering::Acquire);
        if current_count >= EXPECTED_EVENTS {
            println!("REPRO: ✓ All {} events processed successfully!", current_count);
            break;
        }

        if start_time.elapsed() > timeout {
            println!("REPRO: ✗ Test timed out - {} events processed, expected {}", current_count, EXPECTED_EVENTS);
            break;
        }

        // Check for stalls
        if current_count == last_count {
            stall_count += 1;
            if stall_count > 1000 {
                println!("REPRO: WARNING - Test appears stalled at {} events (expected {})", current_count, EXPECTED_EVENTS);
                stall_count = 0;
            }
        } else {
            stall_count = 0;
        }
        last_count = current_count;

        thread::sleep(Duration::from_millis(1));
    }

    // Stop all producers
    println!("REPRO: Stopping all producers");
    let mut all_success = true;
    for (i, producer) in producers.iter_mut().enumerate() {
        let success = producer.stop_and_join();
        if !success {
            println!("REPRO: Producer {} failed", i);
            all_success = false;
        }
    }

    let final_count = counter.load(Ordering::Acquire);
    println!("REPRO: Final result - {} events processed (expected {})", final_count, EXPECTED_EVENTS);

    // Cleanup
    if let Ok(mut disruptor) = Arc::try_unwrap(disruptor_arc) {
        disruptor.shutdown().unwrap();
    }

    // The test should pass if we processed all expected events
    assert_eq!(final_count, EXPECTED_EVENTS, "Should process all {} events", EXPECTED_EVENTS);
    assert!(all_success, "All producers should complete successfully");
}