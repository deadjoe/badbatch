//! Try-publish error taxonomy (R1, 2026-07-19 audit).
//!
//! The non-blocking publish path must distinguish *transient* backpressure
//! (retry is meaningful) from *terminal* pipeline states (retrying can never
//! succeed). Before this audit both were reported as "ring full", which turned
//! a poisoned or shut-down pipeline into an infinite retry loop.

use badbatch::disruptor::{
    build_single_producer, open_single_producer_poller, BusySpinWaitStrategy, DefaultEventFactory,
    MissingFreeSlots, Producer, RingBufferFull, TryPublishError,
};
use std::panic::{catch_unwind, AssertUnwindSafe};

#[derive(Default)]
struct Event {
    value: i64,
}

/// A full ring yields the transient `Full` variant, and retrying after the
/// consumer drains succeeds — the transient story end to end.
#[test]
fn try_publish_on_full_ring_is_transient_and_recovers() {
    let (mut producer, mut poller, _shutdown) =
        open_single_producer_poller(4, DefaultEventFactory::<Event>::new(), BusySpinWaitStrategy)
            .unwrap();

    // Fill the ring (capacity 4).
    for i in 0..4 {
        producer.try_publish(|e| e.value = i).unwrap();
    }

    // The next non-blocking publish is rejected transiently.
    let err = producer.try_publish(|e| e.value = 99).unwrap_err();
    assert!(
        matches!(err, TryPublishError::Full(RingBufferFull)),
        "expected transient Full, got {err:?}"
    );
    assert!(err.is_transient());
    assert!(!err.is_terminal());

    // Drain one event; the retry now succeeds — transient means transient.
    // (EventBatch commits on drop only up to events actually taken, so the
    // batch must be iterated for the gating sequence to advance.)
    let mut drained = 0;
    if let Ok(mut batch) = poller.poll() {
        while batch.next_mut().is_some() {
            drained += 1;
        }
    }
    assert!(drained > 0, "poller should drain at least one event");
    producer.try_publish(|e| e.value = 99).unwrap();
}

/// Valid batch claims report the exact transient slot deficit. Invalid batch
/// sizes are non-retryable and never invoke the update closure.
#[test]
fn try_batch_publish_reports_deficit() {
    let (mut producer, _poller, _shutdown) =
        open_single_producer_poller(8, DefaultEventFactory::<Event>::new(), BusySpinWaitStrategy)
            .unwrap();

    // Partial fill: 6 of 8 used.
    producer
        .batch_publish(6, |iter| {
            for e in iter {
                e.value = 1;
            }
        })
        .unwrap();

    // 4 requested, 2 free → deficit of 2, transient.
    let err = producer.try_batch_publish(4, |_iter| {}).unwrap_err();
    match err {
        TryPublishError::MissingFreeSlots(MissingFreeSlots(deficit)) => {
            assert_eq!(deficit, 2, "deficit must be exact");
        }
        other => panic!("expected MissingFreeSlots, got {other:?}"),
    }
    assert!(err.is_transient());

    // Over-capacity and zero-size requests can never succeed unchanged, so
    // they must not enter a retry loop or invoke user code.
    for requested in [0, 1000, usize::MAX] {
        let err = producer
            .try_batch_publish(requested, |_iter| {
                panic!("invalid batch closure must not run")
            })
            .unwrap_err();
        assert_eq!(
            err,
            TryPublishError::InvalidBatchSize {
                requested,
                capacity: 8,
            }
        );
        assert!(!err.is_transient());
        assert!(!err.is_terminal());
    }
}

/// After a producer-side closure panic poisons the pipeline, both try paths
/// report the terminal `Poisoned` variant — never a transient one.
#[test]
fn try_paths_report_poisoned_as_terminal() {
    let (mut producer, _poller, _shutdown) =
        open_single_producer_poller(8, DefaultEventFactory::<Event>::new(), BusySpinWaitStrategy)
            .unwrap();

    let panicked = catch_unwind(AssertUnwindSafe(|| {
        let _ = producer.publish(|_e| panic!("update closure failure"));
    }));
    assert!(panicked.is_err(), "closure panic must propagate");

    let err = producer.try_publish(|e| e.value = 1).unwrap_err();
    assert!(
        matches!(err, TryPublishError::Poisoned),
        "expected Poisoned, got {err:?}"
    );
    assert!(err.is_terminal());

    let err = producer.try_batch_publish(2, |_iter| {}).unwrap_err();
    assert!(
        matches!(err, TryPublishError::Poisoned),
        "batch path must also report Poisoned, got {err:?}"
    );
    assert!(err.is_terminal());

    // A terminal pipeline state takes precedence over invalid request shape.
    let err = producer.try_batch_publish(0, |_iter| {}).unwrap_err();
    assert!(matches!(err, TryPublishError::Poisoned));
}

/// After halt, the handle-level try paths report the terminal `Shutdown`
/// variant (the sequencer is closed, not "full").
#[test]
fn try_paths_report_shutdown_as_terminal() {
    let mut handle = build_single_producer(8, Event::default, BusySpinWaitStrategy)
        .handle_events_with(|_e: &mut Event, _s, _eob| {})
        .build();

    handle.publish(|e| e.value = 1).unwrap();
    handle.halt();

    let err = handle.try_publish(|e| e.value = 2).unwrap_err();
    assert!(
        matches!(err, TryPublishError::Shutdown),
        "expected Shutdown, got {err:?}"
    );
    assert!(err.is_terminal());

    let err = handle.try_batch_publish(2, |_iter| {}).unwrap_err();
    assert!(
        matches!(err, TryPublishError::Shutdown),
        "batch path must also report Shutdown, got {err:?}"
    );
    assert!(err.is_terminal());

    let err = handle.try_batch_publish(0, |_iter| {}).unwrap_err();
    assert!(matches!(err, TryPublishError::Shutdown));
}
