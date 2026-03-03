//! MPSC stress test: 4 producers x 1 consumer x 1M messages.
//! Validates that the bitmap algorithm correctly handles high-throughput
//! multi-producer concurrent publishing with wraparound.

use badbatch::disruptor::{build_multi_producer, BusySpinWaitStrategy, Producer};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

#[test]
fn test_mpsc_stress_4_producers_1m_messages() {
    const MESSAGES_PER_PRODUCER: u64 = 250_000;
    const NUM_PRODUCERS: u64 = 4;
    const TOTAL_MESSAGES: u64 = MESSAGES_PER_PRODUCER * NUM_PRODUCERS;
    const BUFFER_SIZE: usize = 1024; // Power of 2, >= 64 for bitmap

    let sum = Arc::new(AtomicU64::new(0));
    let count = Arc::new(AtomicU64::new(0));
    let sum_clone = sum.clone();
    let count_clone = count.clone();

    let factory = || 0u64;
    let wait_strategy = BusySpinWaitStrategy::new();

    let mut handle = build_multi_producer(BUFFER_SIZE, factory, wait_strategy)
        .handle_events_with(move |event: &mut u64, _seq, _eob| {
            sum_clone.fetch_add(*event, Ordering::Relaxed);
            count_clone.fetch_add(1, Ordering::Relaxed);
        })
        .build();

    // Spawn 4 producer threads
    let mut producer_handles = Vec::new();
    for producer_id in 0..NUM_PRODUCERS {
        let mut producer = handle.create_producer();
        let h = std::thread::spawn(move || {
            let base = producer_id * MESSAGES_PER_PRODUCER;
            for i in 0..MESSAGES_PER_PRODUCER {
                producer.publish(|slot| {
                    *slot = base + i + 1; // Use 1-based values so sum > 0
                });
            }
        });
        producer_handles.push(h);
    }

    // Wait for all producers to finish
    for h in producer_handles {
        h.join().expect("producer thread panicked");
    }

    // Shutdown and wait for consumer
    handle.shutdown();

    let final_count = count.load(Ordering::Relaxed);
    let final_sum = sum.load(Ordering::Relaxed);

    assert_eq!(
        final_count, TOTAL_MESSAGES,
        "Expected {TOTAL_MESSAGES} messages but got {final_count}"
    );

    // Expected sum: for each producer p (0..4), sum of (base + i + 1) for i in 0..250_000
    let mut expected_sum: u64 = 0;
    for p in 0..NUM_PRODUCERS {
        let base = p * MESSAGES_PER_PRODUCER;
        // Sum of (base+1) + (base+2) + ... + (base+250_000)
        expected_sum +=
            MESSAGES_PER_PRODUCER * base + MESSAGES_PER_PRODUCER * (MESSAGES_PER_PRODUCER + 1) / 2;
    }

    assert_eq!(
        final_sum, expected_sum,
        "Sum mismatch: expected {expected_sum}, got {final_sum}"
    );
}
