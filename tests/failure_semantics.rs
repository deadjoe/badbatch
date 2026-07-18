//! Failure-delivery semantics (P1 round of the 2026-07-18 audit).
//!
//! Publishing is fallible and failures are delivered to the caller:
//! - a panicking consumer poisons the pipeline; blocking publishes return
//!   `DisruptorError::Poisoned` instead of spinning forever,
//! - a panicking producer update closure poisons the pipeline instead of
//!   exposing a never-written slot on the next publish.

use badbatch::disruptor::{
    build_single_producer, open_single_producer_poller, BusySpinWaitStrategy, DefaultEventFactory,
    DisruptorError, Producer,
};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::{Duration, Instant};

#[derive(Debug, Default)]
struct Event {
    value: i64,
}

/// A consumer panic must surface to producers as `Poisoned` within bounded
/// time — previously the producer spun forever on the dead gating sequence.
#[test]
fn consumer_panic_poisons_producer() {
    let mut handle = build_single_producer(8, Event::default, BusySpinWaitStrategy)
        .handle_events_with(|event: &mut Event, _seq, _eob| {
            assert!(event.value != 42, "handler poisoned by test event");
        })
        .build();

    // First event trips the handler panic; the consumer thread dies and
    // poisons the pipeline.
    let _ = handle.publish(|event| event.value = 42);

    // Keep publishing: once the ring is full the blocking claim path would
    // previously spin forever. Now it must return Poisoned within the deadline.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut poisoned = false;
    while Instant::now() < deadline {
        match handle.publish(|event| event.value = 0) {
            Err(DisruptorError::Poisoned) => {
                poisoned = true;
                break;
            }
            Err(other) => panic!("unexpected error: {other:?}"),
            Ok(_) => {}
        }
    }
    assert!(poisoned, "producer never observed the poisoned pipeline");

    handle.shutdown();
}

/// A panicking update closure leaves a claimed-but-unpublished slot; the
/// pipeline must be poisoned so later publishes fail instead of exposing a
/// never-written event.
#[test]
fn producer_closure_panic_poisons_pipeline() {
    let (mut producer, _poller, _shutdown) =
        open_single_producer_poller(8, DefaultEventFactory::<Event>::new(), BusySpinWaitStrategy)
            .unwrap();

    let panicked = catch_unwind(AssertUnwindSafe(|| {
        let _ = producer.publish(|_event| panic!("update closure failure"));
    }));
    assert!(panicked.is_err(), "closure panic must propagate");

    match producer.publish(|event| event.value = 1) {
        Err(DisruptorError::Poisoned) => {}
        other => panic!("expected Poisoned, got {other:?}"),
    }
    assert!(
        producer.try_publish(|event| event.value = 1).is_err(),
        "try_publish must also fail on a poisoned pipeline"
    );
}

/// Batch claims larger than the ring fail loudly through the blocking path
/// too (P1: no silent `internal_error!` drops).
#[test]
fn blocking_batch_publish_rejects_over_capacity() {
    let (mut producer, _poller, _shutdown) =
        open_single_producer_poller(8, DefaultEventFactory::<Event>::new(), BusySpinWaitStrategy)
            .unwrap();

    let mut ran = false;
    let result = producer.batch_publish(9, |_iter| ran = true);
    match result {
        Err(DisruptorError::InvalidSequence(9)) => {}
        other => panic!("expected InvalidSequence(9), got {other:?}"),
    }
    assert!(!ran, "closure must not run for a rejected batch");
}
