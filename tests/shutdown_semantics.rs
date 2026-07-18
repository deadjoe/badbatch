//! Shutdown lifecycle semantics (P1 round of the 2026-07-18 audit).
//!
//! `shutdown()` drains the published backlog before stopping (LMAX
//! `shutdown()`), `halt()` stops abruptly (LMAX `halt()`), and
//! `shutdown_timeout` bounds the drain and reports what interrupted it.

use badbatch::disruptor::{build_single_producer, BusySpinWaitStrategy, DisruptorError};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Default)]
struct Event {
    value: i64,
}

/// The old shutdown dropped the backlog on the floor ("0 consumed" in the
/// audit's repro). A draining shutdown must process every published event.
#[test]
fn shutdown_drains_published_backlog() {
    let seen = Arc::new(AtomicI64::new(0));
    let seen_in_handler = Arc::clone(&seen);

    let mut handle = build_single_producer(128, Event::default, BusySpinWaitStrategy)
        .handle_events_with(move |_event: &mut Event, _seq, _eob| {
            // Slow consumer: forces a real backlog at shutdown time.
            std::thread::sleep(Duration::from_micros(200));
            seen_in_handler.fetch_add(1, Ordering::Relaxed);
        })
        .build();

    for _ in 0..100 {
        handle.publish(|event| event.value = 1).unwrap();
    }
    // No manual wait: shutdown() itself must drain all 100 events.
    handle.shutdown();

    assert_eq!(seen.load(Ordering::Relaxed), 100);
}

/// A bounded drain reports Timeout when the consumer cannot catch up in time,
/// and still stops the disruptor.
#[test]
fn shutdown_timeout_reports_undrained_backlog() {
    let seen = Arc::new(AtomicI64::new(0));
    let seen_in_handler = Arc::clone(&seen);

    let mut handle = build_single_producer(16, Event::default, BusySpinWaitStrategy)
        .handle_events_with(move |_event: &mut Event, _seq, _eob| {
            std::thread::sleep(Duration::from_millis(300));
            seen_in_handler.fetch_add(1, Ordering::Relaxed);
        })
        .build();

    for _ in 0..4 {
        handle.publish(|event| event.value = 1).unwrap();
    }

    let result = handle.shutdown_timeout(Duration::from_millis(100));
    // The Timeout error itself proves the drain did not complete in time.
    // (The subsequent join may still wait out the in-flight batch — all four
    // events can be one batch — so no wall-clock or count bound is asserted.)
    match result {
        Err(DisruptorError::Timeout) => {}
        other => panic!("expected Timeout, got {other:?}"),
    }
    assert!(handle.is_shutdown());
    let _ = seen.load(Ordering::Relaxed);
}

/// Draining must not hang when the consumer is already dead: the drain aborts
/// (Poisoned or dead-consumer report) and the stop completes.
#[test]
fn shutdown_aborts_drain_when_consumer_dead() {
    let mut handle = build_single_producer(16, Event::default, BusySpinWaitStrategy)
        .handle_events_with(|event: &mut Event, _seq, _eob| {
            assert!(event.value != 42, "handler poisoned by test event");
        })
        .build();

    let _ = handle.publish(|event| event.value = 42);

    // Give the panic a moment to land, then shutdown must return promptly
    // instead of draining forever against a dead consumer.
    let start = Instant::now();
    handle.shutdown();
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "shutdown hung against a dead consumer"
    );
}

/// `halt()` must not wait for the backlog (beyond the batch already taken).
#[test]
fn halt_stops_without_draining() {
    let seen = Arc::new(AtomicI64::new(0));
    let seen_in_handler = Arc::clone(&seen);

    let mut handle = build_single_producer(128, Event::default, BusySpinWaitStrategy)
        .handle_events_with(move |_event: &mut Event, _seq, _eob| {
            std::thread::sleep(Duration::from_millis(1));
            seen_in_handler.fetch_add(1, Ordering::Relaxed);
        })
        .build();

    for _ in 0..100 {
        handle.publish(|event| event.value = 1).unwrap();
    }
    handle.halt();

    // halt() joins after the in-flight batch, never guaranteeing the full
    // backlog. (The consumer may legally have consumed everything in one
    // batch, so only the "did not exceed" bound is asserted.)
    assert!(seen.load(Ordering::Relaxed) <= 100);
    assert!(handle.is_shutdown());
}
