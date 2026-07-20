use super::common::{test_event_factory, wait_until, TestEvent};
use crate::disruptor::{
    build_single_producer, BusySpinWaitStrategy, DisruptorError, EventHandler, FailurePhase, Result,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

struct StartFailureHandler {
    event_calls: Arc<AtomicUsize>,
}

impl EventHandler<TestEvent> for StartFailureHandler {
    fn on_event(
        &mut self,
        _event: &mut TestEvent,
        _sequence: i64,
        _end_of_batch: bool,
    ) -> Result<()> {
        self.event_calls.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn on_start(&mut self) -> Result<()> {
        Err(DisruptorError::ShutdownError(
            "startup dependency unavailable".to_string(),
        ))
    }
}

#[test]
fn on_start_failure_stops_before_event_loop_and_preserves_context() {
    let event_calls = Arc::new(AtomicUsize::new(0));
    let mut handle = build_single_producer(8, test_event_factory, BusySpinWaitStrategy)
        .thread_name("startup-check")
        .handle_events_with_handler(StartFailureHandler {
            event_calls: Arc::clone(&event_calls),
        })
        .build();

    wait_until(
        Duration::from_secs(1),
        || handle.is_poisoned(),
        "on_start failure to poison the pipeline",
    );

    assert!(matches!(
        handle.publish(|event| event.value = 1),
        Err(DisruptorError::Poisoned)
    ));
    assert_eq!(event_calls.load(Ordering::Relaxed), 0);

    let failure = handle.first_failure().expect("startup failure record");
    assert_eq!(failure.phase(), FailurePhase::HandlerStart);
    assert_eq!(failure.thread_name(), Some("startup-check"));
    assert_eq!(failure.stage_index(), Some(0));
    assert_eq!(failure.sequence(), None);
    assert!(failure.message().contains("startup dependency unavailable"));

    handle.halt();
}

struct EventAndShutdownFailure;

impl EventHandler<TestEvent> for EventAndShutdownFailure {
    fn on_event(
        &mut self,
        _event: &mut TestEvent,
        sequence: i64,
        _end_of_batch: bool,
    ) -> Result<()> {
        Err(DisruptorError::InvalidSequence(sequence))
    }

    fn on_shutdown(&mut self) -> Result<()> {
        Err(DisruptorError::ShutdownError(
            "secondary shutdown failure".to_string(),
        ))
    }
}

#[test]
fn event_failure_records_sequence_and_first_failure_wins() {
    let mut handle = build_single_producer(8, test_event_factory, BusySpinWaitStrategy)
        .thread_name("event-check")
        .handle_events_with_handler(EventAndShutdownFailure)
        .build();

    handle.publish(|event| event.value = 7).unwrap();
    wait_until(
        Duration::from_secs(1),
        || handle.is_poisoned(),
        "event error to poison the pipeline",
    );
    handle.halt();

    let failure = handle.first_failure().expect("event failure record");
    assert_eq!(failure.phase(), FailurePhase::EventProcessing);
    assert_eq!(failure.thread_name(), Some("event-check"));
    assert_eq!(failure.stage_index(), Some(0));
    assert_eq!(failure.sequence(), Some(0));
    assert_eq!(failure.message(), "Invalid sequence: 0");
}

struct ShutdownFailureHandler;

impl EventHandler<TestEvent> for ShutdownFailureHandler {
    fn on_event(
        &mut self,
        _event: &mut TestEvent,
        _sequence: i64,
        _end_of_batch: bool,
    ) -> Result<()> {
        Ok(())
    }

    fn on_shutdown(&mut self) -> Result<()> {
        Err(DisruptorError::ShutdownError(
            "shutdown flush failed".to_string(),
        ))
    }
}

#[test]
fn on_shutdown_failure_is_queryable_without_reclassifying_as_poison() {
    let mut handle = build_single_producer(8, test_event_factory, BusySpinWaitStrategy)
        .thread_name("shutdown-check")
        .handle_events_with_handler(ShutdownFailureHandler)
        .build();

    handle.halt();

    assert!(!handle.is_poisoned());
    let failure = handle.first_failure().expect("shutdown failure record");
    assert_eq!(failure.phase(), FailurePhase::HandlerShutdown);
    assert_eq!(failure.thread_name(), Some("shutdown-check"));
    assert_eq!(failure.stage_index(), Some(0));
}

#[test]
fn invalid_affinity_fails_closed_with_structured_startup_context() {
    let mut handle = build_single_producer(8, test_event_factory, BusySpinWaitStrategy)
        .thread_name("affinity-check")
        .pin_at_core(usize::MAX)
        .handle_events_with(|_event: &mut TestEvent, _sequence, _end_of_batch| {})
        .build();

    wait_until(
        Duration::from_secs(1),
        || handle.is_poisoned(),
        "invalid affinity to poison the pipeline",
    );

    let failure = handle.first_failure().expect("affinity failure record");
    assert_eq!(failure.phase(), FailurePhase::ThreadAffinity);
    assert_eq!(failure.thread_name(), Some("affinity-check"));
    assert_eq!(failure.stage_index(), Some(0));
    assert!(failure.message().contains("failed to pin to CPU core"));

    handle.halt();
}

#[test]
fn producer_panic_records_claim_context() {
    let mut handle = build_single_producer(8, test_event_factory, BusySpinWaitStrategy)
        .handle_events_with(|_event: &mut TestEvent, _sequence, _end_of_batch| {})
        .build();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = handle.publish(|_event| panic!("producer exploded"));
    }));
    assert!(result.is_err());
    assert!(handle.is_poisoned());

    let failure = handle.first_failure().expect("producer panic record");
    assert_eq!(failure.phase(), FailurePhase::ProducerPanic);
    assert_eq!(failure.sequence(), Some(0));
    assert!(failure.message().contains("after claim and before publish"));

    handle.halt();
}
