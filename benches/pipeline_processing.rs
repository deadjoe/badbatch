//! Pipeline Processing Benchmarks
//!
//! This benchmark suite tests the performance of complex processing pipelines
//! using the BadBatch Disruptor with multiple dependent event handlers.

use criterion::measurement::WallTime;
use criterion::{
    black_box, criterion_group, criterion_main, BenchmarkGroup, BenchmarkId, Criterion, Throughput,
};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use badbatch::disruptor::{
    event_translator::ClosureEventTranslator, BusySpinWaitStrategy, DefaultEventFactory, Disruptor,
    EventHandler, ProducerType, Result as DisruptorResult, YieldingWaitStrategy,
};

// Benchmark configuration constants
const BUFFER_SIZE: usize = 2048;
const BURST_SIZES: [u64; 3] = [50, 200, 1000];

#[derive(Debug, Default, Clone)]
struct PipelineEvent {
    id: i64,
    stage1_result: i64,
    stage2_result: i64,
    stage3_result: i64,
    final_result: i64,
}

/// First stage: Input validation and normalization
struct Stage1Handler {
    processed_count: Arc<AtomicI64>,
}

impl Stage1Handler {
    fn new() -> Self {
        Self {
            processed_count: Arc::new(AtomicI64::new(0)),
        }
    }

    fn get_processed_count(&self) -> Arc<AtomicI64> {
        self.processed_count.clone()
    }
}

impl EventHandler<PipelineEvent> for Stage1Handler {
    fn on_event(
        &mut self,
        event: &mut PipelineEvent,
        _sequence: i64,
        _end_of_batch: bool,
    ) -> DisruptorResult<()> {
        // Simulate input validation and normalization
        event.stage1_result = black_box(event.id * 2 + 1);
        self.processed_count.fetch_add(1, Ordering::Release);
        Ok(())
    }
}

/// Second stage: Business logic processing
struct Stage2Handler {
    processed_count: Arc<AtomicI64>,
}

impl Stage2Handler {
    fn new() -> Self {
        Self {
            processed_count: Arc::new(AtomicI64::new(0)),
        }
    }

    fn get_processed_count(&self) -> Arc<AtomicI64> {
        self.processed_count.clone()
    }
}

impl EventHandler<PipelineEvent> for Stage2Handler {
    fn on_event(
        &mut self,
        event: &mut PipelineEvent,
        _sequence: i64,
        _end_of_batch: bool,
    ) -> DisruptorResult<()> {
        // Simulate business logic processing
        event.stage2_result = black_box(event.stage1_result * 3 + event.id);
        self.processed_count.fetch_add(1, Ordering::Release);
        Ok(())
    }
}

/// Third stage: Aggregation and enrichment
struct Stage3Handler {
    processed_count: Arc<AtomicI64>,
}

impl Stage3Handler {
    fn new() -> Self {
        Self {
            processed_count: Arc::new(AtomicI64::new(0)),
        }
    }

    fn get_processed_count(&self) -> Arc<AtomicI64> {
        self.processed_count.clone()
    }
}

impl EventHandler<PipelineEvent> for Stage3Handler {
    fn on_event(
        &mut self,
        event: &mut PipelineEvent,
        _sequence: i64,
        _end_of_batch: bool,
    ) -> DisruptorResult<()> {
        // Simulate aggregation and enrichment
        event.stage3_result = black_box(event.stage2_result + event.stage1_result / 2);
        self.processed_count.fetch_add(1, Ordering::Release);
        Ok(())
    }
}

/// Final stage: Output formatting and storage
struct FinalHandler {
    processed_count: Arc<AtomicI64>,
    last_id: Arc<AtomicI64>,
}

impl FinalHandler {
    fn new() -> Self {
        Self {
            processed_count: Arc::new(AtomicI64::new(0)),
            last_id: Arc::new(AtomicI64::new(0)),
        }
    }

    fn get_processed_count(&self) -> Arc<AtomicI64> {
        self.processed_count.clone()
    }

    fn get_last_id(&self) -> Arc<AtomicI64> {
        self.last_id.clone()
    }
}

impl EventHandler<PipelineEvent> for FinalHandler {
    fn on_event(
        &mut self,
        event: &mut PipelineEvent,
        _sequence: i64,
        _end_of_batch: bool,
    ) -> DisruptorResult<()> {
        // Simulate final processing and output
        event.final_result = black_box(event.stage3_result + event.stage2_result);
        self.last_id.store(event.id, Ordering::Release);
        self.processed_count.fetch_add(1, Ordering::Release);
        Ok(())
    }
}

/// Benchmark two-stage pipeline (Stage1 -> Stage2)
fn benchmark_two_stage_pipeline(
    group: &mut BenchmarkGroup<WallTime>,
    burst_size: u64,
    wait_strategy: &str,
) {
    let factory = DefaultEventFactory::<PipelineEvent>::new();
    let stage1 = Stage1Handler::new();
    let stage2 = Stage2Handler::new();

    let stage1_count = stage1.get_processed_count();
    let stage2_count = stage2.get_processed_count();

    let wait_strategy_impl: Box<dyn badbatch::disruptor::WaitStrategy> = match wait_strategy {
        "BusySpin" => Box::new(BusySpinWaitStrategy::new()),
        "Yielding" => Box::new(YieldingWaitStrategy::new()),
        _ => Box::new(BusySpinWaitStrategy::new()),
    };

    let mut disruptor = Disruptor::new(
        factory,
        BUFFER_SIZE,
        ProducerType::Single,
        wait_strategy_impl,
    )
    .unwrap()
    .handle_events_with(stage1)
    .then(stage2)
    .build();

    disruptor.start().unwrap();

    let benchmark_id = BenchmarkId::new(format!("TwoStage_{}", wait_strategy), burst_size);
    group.throughput(Throughput::Elements(burst_size));

    group.bench_function(benchmark_id, |b| {
        b.iter_custom(|iters| {
            let start = Instant::now();
            for _ in 0..iters {
                stage1_count.store(0, Ordering::Release);
                stage2_count.store(0, Ordering::Release);

                for i in 1..=burst_size {
                    disruptor
                        .publish_event(ClosureEventTranslator::new(
                            move |event: &mut PipelineEvent, _seq: i64| {
                                event.id = black_box(i as i64);
                                event.stage1_result = 0;
                                event.stage2_result = 0;
                                event.stage3_result = 0;
                                event.final_result = 0;
                            },
                        ))
                        .unwrap();
                }

                // Wait for both stages to complete
                while stage2_count.load(Ordering::Acquire) < burst_size as i64 {
                    std::hint::spin_loop();
                }
            }
            start.elapsed()
        })
    });

    disruptor.shutdown().unwrap();
}

/// Benchmark three-stage pipeline (Stage1 -> Stage2 -> Stage3)
fn benchmark_three_stage_pipeline(
    group: &mut BenchmarkGroup<WallTime>,
    burst_size: u64,
    wait_strategy: &str,
) {
    let factory = DefaultEventFactory::<PipelineEvent>::new();
    let stage1 = Stage1Handler::new();
    let stage2 = Stage2Handler::new();
    let stage3 = Stage3Handler::new();

    let stage3_count = stage3.get_processed_count();

    let wait_strategy_impl: Box<dyn badbatch::disruptor::WaitStrategy> = match wait_strategy {
        "BusySpin" => Box::new(BusySpinWaitStrategy::new()),
        "Yielding" => Box::new(YieldingWaitStrategy::new()),
        _ => Box::new(BusySpinWaitStrategy::new()),
    };

    let mut disruptor = Disruptor::new(
        factory,
        BUFFER_SIZE,
        ProducerType::Single,
        wait_strategy_impl,
    )
    .unwrap()
    .handle_events_with(stage1)
    .then(stage2)
    .then(stage3)
    .build();

    disruptor.start().unwrap();

    let benchmark_id = BenchmarkId::new(format!("ThreeStage_{}", wait_strategy), burst_size);
    group.throughput(Throughput::Elements(burst_size));

    group.bench_function(benchmark_id, |b| {
        b.iter_custom(|iters| {
            let start = Instant::now();
            for _ in 0..iters {
                stage3_count.store(0, Ordering::Release);

                for i in 1..=burst_size {
                    disruptor
                        .publish_event(ClosureEventTranslator::new(
                            move |event: &mut PipelineEvent, _seq: i64| {
                                event.id = black_box(i as i64);
                                event.stage1_result = 0;
                                event.stage2_result = 0;
                                event.stage3_result = 0;
                                event.final_result = 0;
                            },
                        ))
                        .unwrap();
                }

                // Wait for all three stages to complete
                while stage3_count.load(Ordering::Acquire) < burst_size as i64 {
                    std::hint::spin_loop();
                }
            }
            start.elapsed()
        })
    });

    disruptor.shutdown().unwrap();
}

/// Benchmark four-stage pipeline (Stage1 -> Stage2 -> Stage3 -> Final)
fn benchmark_four_stage_pipeline(
    group: &mut BenchmarkGroup<WallTime>,
    burst_size: u64,
    wait_strategy: &str,
) {
    let factory = DefaultEventFactory::<PipelineEvent>::new();
    let stage1 = Stage1Handler::new();
    let stage2 = Stage2Handler::new();
    let stage3 = Stage3Handler::new();
    let final_stage = FinalHandler::new();

    let final_count = final_stage.get_processed_count();
    let last_id = final_stage.get_last_id();

    let wait_strategy_impl: Box<dyn badbatch::disruptor::WaitStrategy> = match wait_strategy {
        "BusySpin" => Box::new(BusySpinWaitStrategy::new()),
        "Yielding" => Box::new(YieldingWaitStrategy::new()),
        _ => Box::new(BusySpinWaitStrategy::new()),
    };

    let mut disruptor = Disruptor::new(
        factory,
        BUFFER_SIZE,
        ProducerType::Single,
        wait_strategy_impl,
    )
    .unwrap()
    .handle_events_with(stage1)
    .then(stage2)
    .then(stage3)
    .then(final_stage)
    .build();

    disruptor.start().unwrap();

    let benchmark_id = BenchmarkId::new(format!("FourStage_{}", wait_strategy), burst_size);
    group.throughput(Throughput::Elements(burst_size));

    group.bench_function(benchmark_id, |b| {
        b.iter_custom(|iters| {
            let start = Instant::now();
            for _ in 0..iters {
                final_count.store(0, Ordering::Release);
                last_id.store(0, Ordering::Release);

                for i in 1..=burst_size {
                    disruptor
                        .publish_event(ClosureEventTranslator::new(
                            move |event: &mut PipelineEvent, _seq: i64| {
                                event.id = black_box(i as i64);
                                event.stage1_result = 0;
                                event.stage2_result = 0;
                                event.stage3_result = 0;
                                event.final_result = 0;
                            },
                        ))
                        .unwrap();
                }

                // Wait for final stage to process the last event
                while last_id.load(Ordering::Acquire) < burst_size as i64 {
                    std::hint::spin_loop();
                }
            }
            start.elapsed()
        })
    });

    disruptor.shutdown().unwrap();
}

/// Main pipeline processing benchmark function
pub fn pipeline_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("Pipeline");

    // Configure benchmark group
    group.measurement_time(Duration::from_secs(20));
    group.warm_up_time(Duration::from_secs(5));

    for &burst_size in BURST_SIZES.iter() {
        // Test different wait strategies
        for wait_strategy in &["BusySpin", "Yielding"] {
            benchmark_two_stage_pipeline(&mut group, burst_size, wait_strategy);
            benchmark_three_stage_pipeline(&mut group, burst_size, wait_strategy);

            // Only test four-stage for smaller burst sizes to avoid long run times
            if burst_size <= 200 {
                benchmark_four_stage_pipeline(&mut group, burst_size, wait_strategy);
            }
        }
    }

    group.finish();
}

criterion_group!(pipeline, pipeline_benchmark);
criterion_main!(pipeline);
