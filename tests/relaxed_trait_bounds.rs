#![allow(missing_docs)]

use badbatch::disruptor::{build_single_producer, BusySpinWaitStrategy, EventHandler, Result};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Default)]
struct EventWithoutDebug {
    value: usize,
}

// `Cell` is `Send` but not `Sync`, so this handler exercises the exact relaxed
// bound rather than merely omitting an explicit `Sync` implementation.
struct SendOnlyHandler {
    local_count: Cell<usize>,
    total_count: Arc<AtomicUsize>,
}

impl SendOnlyHandler {
    fn new(total_count: Arc<AtomicUsize>) -> Self {
        Self {
            local_count: Cell::new(0),
            total_count,
        }
    }
}

impl EventHandler<EventWithoutDebug> for SendOnlyHandler {
    fn on_event(
        &mut self,
        event: &mut EventWithoutDebug,
        _sequence: i64,
        _end_of_batch: bool,
    ) -> Result<()> {
        self.local_count.set(self.local_count.get() + 1);
        event.value += 1;
        self.total_count.fetch_add(1, Ordering::Release);
        Ok(())
    }
}

fn wait_for_count(total_count: &AtomicUsize, expected: usize) {
    let deadline = Instant::now() + Duration::from_secs(1);
    while total_count.load(Ordering::Acquire) != expected {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for send-only handler"
        );
        std::thread::yield_now();
    }
}

#[test]
fn builder_accepts_send_not_sync_handler_and_non_debug_event() {
    fn assert_send<T: Send>() {}
    assert_send::<SendOnlyHandler>();

    let total_count = Arc::new(AtomicUsize::new(0));
    let mut handle = build_single_producer(8, EventWithoutDebug::default, BusySpinWaitStrategy)
        .handle_events_with_handler(SendOnlyHandler::new(Arc::clone(&total_count)))
        .build();

    for value in 0..4 {
        handle.publish(|event| event.value = value).unwrap();
    }
    wait_for_count(&total_count, 4);
    handle.shutdown();
}

#[cfg(feature = "lmax-dsl")]
#[test]
fn classic_dsl_accepts_send_not_sync_handler_and_non_debug_event() {
    use badbatch::disruptor::event_translator::ClosureEventTranslator;
    use badbatch::disruptor::{BlockingWaitStrategy, DefaultEventFactory, Disruptor, ProducerType};

    let total_count = Arc::new(AtomicUsize::new(0));
    let factory = DefaultEventFactory::<EventWithoutDebug>::new();
    let mut disruptor = Disruptor::new(
        factory,
        8,
        ProducerType::Single,
        BlockingWaitStrategy::new(),
    )
    .unwrap()
    .handle_events_with(SendOnlyHandler::new(Arc::clone(&total_count)))
    .build();

    disruptor.start().unwrap();
    for value in 0..4 {
        disruptor
            .publish_event(ClosureEventTranslator::new(
                move |event: &mut EventWithoutDebug, _sequence| {
                    event.value = value;
                },
            ))
            .unwrap();
    }
    wait_for_count(&total_count, 4);
    disruptor.shutdown().unwrap();
}
