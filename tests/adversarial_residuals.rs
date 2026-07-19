//! Adversarial residual regressions closed during the 2026-07-19 residual pass:
//! - `try_run_once` handler panic must poison producers (same as `run`)
//! - single-producer DSL nested publish must not deadlock (rejects reentrancy)

use badbatch::disruptor::{
    BatchEventProcessor, BusySpinWaitStrategy, DefaultEventFactory, DefaultExceptionHandler,
    Disruptor, DisruptorError, EventHandler, EventProcessor, EventTranslator,
    ProcessingSequenceBarrier, ProducerType, Result, RingBuffer, Sequencer, SequencerEnum,
    SingleProducerSequencer, TryPublishError,
};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Default)]
struct Ev {
    v: i64,
}

struct PanicHandler;
impl EventHandler<Ev> for PanicHandler {
    fn on_event(&mut self, _e: &mut Ev, _s: i64, _b: bool) -> Result<()> {
        panic!("handler boom");
    }
}

struct NoopTranslator;
impl EventTranslator<Ev> for NoopTranslator {
    fn translate_to(&self, _event: &mut Ev, _sequence: i64) {}
}

struct Reentrant {
    d: Arc<Disruptor<Ev, BusySpinWaitStrategy>>,
    nested: Arc<AtomicU8>,
}
impl EventTranslator<Ev> for Reentrant {
    fn translate_to(&self, event: &mut Ev, _sequence: i64) {
        event.v = 1;
        let nested = self.d.try_publish_event(NoopTranslator);
        match nested {
            Err(TryPublishError::ReentrantPublish) => {
                self.nested.store(1, Ordering::Release);
            }
            other => {
                self.nested.store(2, Ordering::Release);
                let _ = other;
            }
        }
    }
}

struct ReentrantBlocking {
    d: Arc<Disruptor<Ev, BusySpinWaitStrategy>>,
}
impl EventTranslator<Ev> for ReentrantBlocking {
    fn translate_to(&self, event: &mut Ev, _sequence: i64) {
        event.v = 2;
        let err = self
            .d
            .publish_event(NoopTranslator)
            .expect_err("nested blocking publish must fail");
        assert!(matches!(err, DisruptorError::ReentrantPublish));
    }
}

/// `run()` poisons on handler panic; `try_run_once` must honor the same contract.
#[test]
fn try_run_once_handler_panic_poisons_pipeline() {
    let ring = Arc::new(RingBuffer::new(8, DefaultEventFactory::<Ev>::new()).unwrap());
    let wait = Arc::new(BusySpinWaitStrategy);
    let sequencer = Arc::new(unsafe { SingleProducerSequencer::new(8, wait.clone()) });
    sequencer.add_gating_sequences(&[]);
    let barrier = Arc::new(ProcessingSequenceBarrier::new(
        sequencer.get_cursor(),
        wait,
        vec![],
        SequencerEnum::Single(sequencer.clone()),
    ));
    let processor = unsafe {
        BatchEventProcessor::new(
            ring,
            barrier,
            PanicHandler,
            Box::new(DefaultExceptionHandler::<Ev>::new()),
        )
    };

    let seq = sequencer.next().unwrap();
    sequencer.publish(seq);

    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| processor.try_run_once()));
    assert!(r.is_err(), "handler panic must propagate out of the probe");
    assert!(
        sequencer.is_poisoned(),
        "try_run_once panic must poison the pipeline"
    );
}

/// Nested publish from a translator must not deadlock; nested claim is rejected.
#[test]
fn dsl_reentrant_publish_does_not_deadlock() {
    let disruptor = Arc::new(
        Disruptor::new(
            DefaultEventFactory::<Ev>::new(),
            8,
            ProducerType::Single,
            BusySpinWaitStrategy,
        )
        .unwrap(),
    );

    let nested = Arc::new(AtomicU8::new(0));
    let d2 = disruptor.clone();
    let nested2 = Arc::clone(&nested);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let r = d2.publish_event(Reentrant {
            d: Arc::clone(&d2),
            nested: nested2,
        });
        let _ = tx.send(r);
    });

    let outer = rx
        .recv_timeout(Duration::from_secs(3))
        .expect("re-entrant publish deadlocked on single_publish_lock");
    assert!(
        outer.is_ok(),
        "outer publish should succeed after nested reject: {outer:?}"
    );
    assert_eq!(
        nested.load(Ordering::Acquire),
        1,
        "nested try_publish must report ReentrantPublish"
    );

    disruptor
        .publish_event(ReentrantBlocking {
            d: Arc::clone(&disruptor),
        })
        .unwrap();
}
