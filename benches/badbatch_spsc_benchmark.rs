use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

// Assuming 'badbatch' is the crate name as per Cargo.toml
use badbatch::disruptor::{
    BusySpinWaitStrategy,
    DefaultEventFactory, // Added DefaultEventFactory for direct use
    Disruptor,
    DisruptorError,
    EventHandler,
    EventTranslator,
    ProducerType,
};

#[derive(Debug, Default, Clone, Copy)]
struct BenchmarkEvent {
    value: i64,
}

// Event Handler: processes events and updates the sink
struct BenchmarkEventHandler {
    sink: Arc<AtomicI64>,
}

impl EventHandler<BenchmarkEvent> for BenchmarkEventHandler {
    fn on_event(
        &mut self,
        event: &mut BenchmarkEvent,
        _sequence: i64,
        _end_of_batch: bool,
    ) -> Result<(), DisruptorError> {
        self.sink.store(event.value, Ordering::Release);
        Ok(())
    }
    // on_start, on_shutdown, on_timeout will use default implementations
}

// Event Translator: populates event data during publishing
struct BenchmarkEventTranslator {
    value_to_set: i64,
}

impl EventTranslator<BenchmarkEvent> for BenchmarkEventTranslator {
    fn translate_to(&self, event: &mut BenchmarkEvent, _sequence: i64) {
        event.value = criterion::black_box(self.value_to_set);
    }
}

fn spsc_throughput_benchmark(c: &mut Criterion) {
    const BUFFER_SIZE: usize = 1024 * 64; // Must be a power of 2
                                          // Using a single burst size for simplicity in this initial setup, can be expanded
    const BURST_SIZES: [u64; 2] = [100, 1_000]; // Number of events to publish per iteration

    let mut group = c.benchmark_group("BadBatch_SPSC_Throughput_BusySpin");

    for &burst_size_val in BURST_SIZES.iter() {
        group.throughput(Throughput::Elements(burst_size_val));

        // Event factory: uses DefaultEventFactory for BenchmarkEvent which implements Default
        let event_factory = DefaultEventFactory::<BenchmarkEvent>::new();

        // Sink to signal completion from event handler to producer thread
        let sink = Arc::new(AtomicI64::new(0));

        // Event handler instance
        let event_handler = BenchmarkEventHandler {
            sink: Arc::clone(&sink),
        };

        // Disruptor setup
        let mut disruptor = match Disruptor::new(
            event_factory,
            BUFFER_SIZE,
            ProducerType::Single,
            Box::new(BusySpinWaitStrategy::new()),
        ) {
            Ok(d) => d.handle_events_with(event_handler).build(),
            Err(e) => panic!("Failed to create disruptor: {:?}", e),
        };

        if let Err(e) = disruptor.start() {
            panic!("Failed to start disruptor: {:?}", e);
        }

        let benchmark_id = BenchmarkId::from_parameter(burst_size_val);
        group.bench_function(benchmark_id, |b| {
            b.iter_custom(|measurement_iters| {
                let mut total_duration = Duration::ZERO;
                for _i_measurement in 0..measurement_iters {
                    let start_time = Instant::now();
                    for i in 0..burst_size_val {
                        // The value must be unique for the sink to correctly identify the last event.
                        // For iter_custom, measurement_iters is usually small (e.g., 10 for precise measurements).
                        // If burst_size_val is large, i + 1 will be unique within a single measurement iteration.
                        // The sink is reset implicitly by the fact that we only check for the *last* value of the current burst.
                        let value_to_set = i as i64 + 1;
                        if let Err(e) =
                            disruptor.publish_event(BenchmarkEventTranslator { value_to_set })
                        {
                            // In a real benchmark, might log or handle, but for now, panic.
                            panic!("Failed to publish event: {:?}", e);
                        }
                    }

                    let last_expected_value = burst_size_val as i64;
                    while sink.load(Ordering::Acquire) != last_expected_value {
                        // Spin wait, consistent with BusySpinWaitStrategy and reference benchmark
                        std::hint::spin_loop();
                    }
                    total_duration += start_time.elapsed();
                }
                total_duration
            })
        });

        if let Err(e) = disruptor.shutdown() {
            // Log error or handle, but don't panic during benchmark cleanup if possible
            eprintln!("Error shutting down disruptor: {:?}", e);
        }
    }
    group.finish();
}

criterion_group!(benches, spsc_throughput_benchmark);
criterion_main!(benches);
