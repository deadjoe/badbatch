//! Soundness regression tests for the 2026-07-18 cross-audit findings.
//!
//! Runtime-observable pieces of the four P0 fixes live here. The type-level
//! guarantees are pinned by `compile_fail` doctests in the library:
//! - `SimpleProducer` is not `Clone` (producer.rs)
//! - single-mode `DisruptorHandle` has no `create_producer` (builder/handle.rs)

use badbatch::disruptor::{
    build_multi_producer, build_single_producer, open_single_producer_poller, BusySpinWaitStrategy,
    DefaultEventFactory, Producer,
};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::thread;

#[derive(Debug, Default)]
struct Event {
    value: i64,
}

/// P0-4: an over-capacity batch claim used to succeed with no gating sequences
/// (empty-gating minimum was `i64::MAX`), handing out aliased `&mut` to the same
/// ring slot within one batch. It must now fail without running the closure.
#[test]
fn try_batch_publish_rejects_over_capacity() {
    let (mut producer, _poller, _shutdown) =
        open_single_producer_poller(8, DefaultEventFactory::<Event>::new(), BusySpinWaitStrategy)
            .unwrap();

    let mut closure_ran = false;
    let result = producer.try_batch_publish(9, |_iter| {
        closure_ran = true;
    });
    assert!(result.is_err(), "claim larger than the buffer must fail");
    assert!(
        !closure_ran,
        "update closure must not run on a failed claim"
    );

    let result = producer.try_batch_publish(1000, |_iter| {});
    assert!(result.is_err());

    // A full-buffer batch stays legal and yields exactly buffer_size slots.
    let mut yielded = 0;
    let result = producer.try_batch_publish(8, |iter| {
        for event in iter {
            event.value = 1;
            yielded += 1;
        }
    });
    assert_eq!(result.unwrap(), 7);
    assert_eq!(yielded, 8);
}

/// P0-1 capability smoke: the single-mode handle still publishes normally
/// through its one owned producer handle.
#[test]
fn single_mode_handle_publishes() {
    let seen = Arc::new(AtomicI64::new(0));
    let seen_in_handler = Arc::clone(&seen);

    let mut handle = build_single_producer(64, Event::default, BusySpinWaitStrategy)
        .handle_events_with(move |event: &mut Event, _seq, _eob| {
            seen_in_handler.fetch_add(event.value, Ordering::Relaxed);
        })
        .build();

    for _ in 0..10 {
        handle.publish(|event| event.value = 1).unwrap();
    }
    // shutdown() drains the published backlog (P1 round) — no manual wait.
    handle.shutdown();

    assert_eq!(seen.load(Ordering::Relaxed), 10);
}

/// P0-1 capability smoke: multi mode hands out one producer per thread via
/// `create_producer` (the only remaining way to get extra handles).
#[test]
fn multi_mode_one_producer_per_thread() {
    let seen = Arc::new(AtomicI64::new(0));
    let seen_in_handler = Arc::clone(&seen);

    let mut handle = build_multi_producer(64, Event::default, BusySpinWaitStrategy)
        .handle_events_with(move |event: &mut Event, _seq, _eob| {
            seen_in_handler.fetch_add(event.value, Ordering::Relaxed);
        })
        .build();

    let threads: Vec<_> = (0..2)
        .map(|_| {
            let mut producer = handle.create_producer();
            thread::spawn(move || {
                for _ in 0..100 {
                    producer.publish(|event| event.value = 1).unwrap();
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    // shutdown() drains the published backlog (P1 round) — no manual wait.
    handle.shutdown();

    assert_eq!(seen.load(Ordering::Relaxed), 200);
}
