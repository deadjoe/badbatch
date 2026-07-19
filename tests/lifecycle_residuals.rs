//! Residual lifecycle fixes after the 2026-07-18 audit P0–P2 rounds:
//! - publish after halt/shutdown fails (sequencer closed)
//! - DSL `Disruptor::shutdown` drains like the Builder path

use badbatch::disruptor::event_translator::ClosureEventTranslator;
use badbatch::disruptor::{
    build_multi_producer, build_single_producer, BusySpinWaitStrategy, DefaultEventFactory,
    Disruptor, DisruptorError, EventHandler, Producer, ProducerType, Result as DisruptorResult,
    TryPublishError,
};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Default)]
struct Event {
    value: i64,
}

/// After `halt()`, further blocking publishes must fail with `Shutdown` (not spin
/// forever writing into a dead pipeline).
#[test]
fn publish_after_halt_returns_shutdown() {
    let mut handle = build_single_producer(16, Event::default, BusySpinWaitStrategy)
        .handle_events_with(|_e: &mut Event, _s, _eob| {})
        .build();

    handle.publish(|e| e.value = 1).unwrap();
    handle.halt();

    match handle.publish(|e| e.value = 2) {
        Err(DisruptorError::Shutdown) => {}
        other => panic!("expected Shutdown after halt, got {other:?}"),
    }
    // The try path must distinguish the terminal state too (R1, 2026-07-19
    // audit): after halt the answer is Shutdown, not "ring full".
    match handle.try_publish(|e| e.value = 3) {
        Err(TryPublishError::Shutdown) => {}
        other => panic!("expected TryPublishError::Shutdown after halt, got {other:?}"),
    }
}

/// After draining `shutdown()`, publish is likewise rejected.
#[test]
fn publish_after_shutdown_returns_shutdown() {
    let mut handle = build_single_producer(16, Event::default, BusySpinWaitStrategy)
        .handle_events_with(|_e: &mut Event, _s, _eob| {})
        .build();

    for _ in 0..5 {
        handle.publish(|e| e.value = 1).unwrap();
    }
    handle.shutdown();

    match handle.publish(|e| e.value = 9) {
        Err(DisruptorError::Shutdown) => {}
        other => panic!("expected Shutdown after shutdown, got {other:?}"),
    }
}

struct CountingHandler {
    seen: Arc<AtomicI64>,
}

impl EventHandler<Event> for CountingHandler {
    fn on_event(
        &mut self,
        event: &mut Event,
        _sequence: i64,
        _end_of_batch: bool,
    ) -> DisruptorResult<()> {
        // Slow enough that a non-draining halt would leave backlog.
        std::thread::sleep(Duration::from_micros(200));
        self.seen.fetch_add(event.value, Ordering::Relaxed);
        Ok(())
    }
}

/// DSL [`Disruptor::shutdown`] must drain the published backlog (was abrupt halt-only).
#[test]
fn dsl_shutdown_drains_published_backlog() {
    let seen = Arc::new(AtomicI64::new(0));
    let factory = DefaultEventFactory::<Event>::new();
    let mut disruptor = Disruptor::new(factory, 64, ProducerType::Single, BusySpinWaitStrategy)
        .unwrap()
        .handle_events_with(CountingHandler {
            seen: Arc::clone(&seen),
        })
        .build();

    disruptor.start().unwrap();

    for _ in 0..20 {
        disruptor
            .publish_event(ClosureEventTranslator::new(|e: &mut Event, _seq| {
                e.value = 1;
            }))
            .unwrap();
    }

    disruptor.shutdown().unwrap();
    assert_eq!(seen.load(Ordering::Relaxed), 20);

    // Claim path closed: further publishes fail.
    let result = disruptor.publish_event(ClosureEventTranslator::new(|e: &mut Event, _seq| {
        e.value = 1;
    }));
    assert!(
        matches!(result, Err(DisruptorError::Shutdown)),
        "expected Shutdown, got {result:?}"
    );
    assert!(disruptor.is_claim_closed());
    assert!(!disruptor.is_poisoned());
}

/// Handle lifecycle queries reflect claim-path and poison state.
#[test]
fn handle_lifecycle_queries() {
    let mut handle = build_single_producer(16, Event::default, BusySpinWaitStrategy)
        .handle_events_with(|_e: &mut Event, _s, _eob| {})
        .build();

    assert!(!handle.is_poisoned());
    assert!(!handle.is_claim_closed());
    handle.halt();
    assert!(handle.is_claim_closed());
    assert!(handle.is_shutdown());

    // Multi-mode CloneableProducer is the supported public mint path.
    let mut multi = build_multi_producer(16, Event::default, BusySpinWaitStrategy)
        .handle_events_with(|_e: &mut Event, _s, _eob| {})
        .build();
    let shared = multi.cloneable_producer();
    let mut p1 = shared.create_producer();
    let mut p2 = shared.clone().create_producer();
    p1.publish(|e| e.value = 1).unwrap();
    p2.publish(|e| e.value = 2).unwrap();
    multi.shutdown();
    assert!(multi.is_claim_closed());
}

/// DSL `try_publish_event` reports terminal Shutdown distinctly from Full.
#[test]
fn dsl_try_publish_reports_shutdown_terminal() {
    let factory = DefaultEventFactory::<Event>::new();
    let mut disruptor = Disruptor::new(factory, 8, ProducerType::Single, BusySpinWaitStrategy)
        .unwrap()
        .handle_events_with(CountingHandler {
            seen: Arc::new(AtomicI64::new(0)),
        })
        .build();
    disruptor.start().unwrap();
    disruptor.halt().unwrap();

    match disruptor.try_publish_event(ClosureEventTranslator::new(|e: &mut Event, _| {
        e.value = 1;
    })) {
        Err(TryPublishError::Shutdown) => {}
        other => panic!("expected TryPublishError::Shutdown, got {other:?}"),
    }
}
