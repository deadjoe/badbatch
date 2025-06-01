//! Multi Producer Single Consumer (MPSC) Benchmark
//!
//! This benchmark compares BadBatch Disruptor performance against crossbeam channels
//! in multi producer, single consumer scenarios with various burst sizes and pause intervals.

use criterion::measurement::WallTime;
use criterion::{
    black_box, criterion_group, criterion_main, BenchmarkGroup, BenchmarkId, Criterion, Throughput,
};
use crossbeam::channel::TrySendError::Full;
use crossbeam::channel::{
    bounded,
    TryRecvError::{Disconnected, Empty},
};
use crossbeam_utils::CachePadded;
use std::sync::atomic::{
    AtomicBool, AtomicI64,
    Ordering::{Acquire, Relaxed, Release},
};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

// BadBatch Disruptor imports
use badbatch::disruptor::{build_multi_producer, BusySpinWaitStrategy, Disruptor, EventHandler, EventTranslator, ProducerType};

// Benchmark configuration
const PRODUCERS: usize = 2;
const DATA_STRUCTURE_SIZE: usize = 256;
const BURST_SIZES: [u64; 3] = [1, 10, 100];
const PAUSES_MS: [u64; 3] = [0, 1, 10];

/// Event structure for benchmarking
#[derive(Debug, Default, Clone)]
struct Event {
    data: i64,
}

/// Pause execution for the specified number of milliseconds
fn pause(millis: u64) {
    if millis > 0 {
        thread::sleep(Duration::from_millis(millis));
    }
}

/// Main MPSC benchmark function
pub fn mpsc_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("mpsc");

    for burst_size in BURST_SIZES.into_iter() {
        group.throughput(Throughput::Elements(burst_size));

        // Base: Benchmark overhead of measurement logic
        base_overhead(&mut group, burst_size as i64);

        for pause_ms in PAUSES_MS.into_iter() {
            let params = (burst_size as i64, pause_ms);
            let param_description = format!("burst: {}, pause: {} ms", burst_size, pause_ms);

            crossbeam_mpsc(&mut group, params, &param_description);
            badbatch_mpsc_modern(&mut group, params, &param_description);
            badbatch_mpsc_traditional(&mut group, params, &param_description);
        }
    }
    group.finish();
}

/// Structure for managing all producer threads so they can produce a burst again and again
/// in a benchmark after being released from a barrier. This avoids the overhead of creating
/// new threads for each sample.
struct BurstProducer {
    start_barrier: Arc<CachePadded<AtomicBool>>,
    stop: Arc<CachePadded<AtomicBool>>,
    join_handle: Option<JoinHandle<()>>,
}

impl BurstProducer {
    fn new<P>(mut produce_one_burst: P) -> Self
    where
        P: 'static + Send + FnMut(),
    {
        let start_barrier = Arc::new(CachePadded::new(AtomicBool::new(false)));
        let stop = Arc::new(CachePadded::new(AtomicBool::new(false)));

        let join_handle = {
            let stop = Arc::clone(&stop);
            let start_barrier = Arc::clone(&start_barrier);
            thread::spawn(move || {
                while !stop.load(Acquire) {
                    // Busy spin with a check if we're done
                    while start_barrier
                        .compare_exchange(true, false, Acquire, Relaxed)
                        .is_err()
                    {
                        if stop.load(Acquire) {
                            return;
                        }
                    }
                    produce_one_burst();
                }
            })
        };

        Self {
            start_barrier,
            stop,
            join_handle: Some(join_handle),
        }
    }

    fn start(&self) {
        self.start_barrier.store(true, Release);
    }

    fn stop(&mut self) {
        self.stop.store(true, Release);
        self.join_handle
            .take()
            .unwrap()
            .join()
            .expect("Should not panic");
    }
}

/// Helper function to run benchmarks with burst producers
fn run_benchmark(
    group: &mut BenchmarkGroup<WallTime>,
    benchmark_id: BenchmarkId,
    burst_size: Arc<AtomicI64>,
    sink: Arc<AtomicI64>,
    params: (i64, u64),
    burst_producers: &[BurstProducer],
) {
    group.bench_with_input(benchmark_id, &params, move |b, (size, pause_ms)| {
        b.iter_custom(|iters| {
            burst_size.store(*size, Release);
            let count = black_box(*size * burst_producers.len() as i64);
            pause(*pause_ms);
            let start = Instant::now();
            for _ in 0..iters {
                sink.store(0, Release);
                burst_producers.iter().for_each(BurstProducer::start);
                // Wait for all producers to finish publication
                while sink.load(Acquire) != count {
                    // Busy spin
                }
            }
            start.elapsed()
        })
    });
}

/// Synthetic benchmark to measure the overhead of the measurement itself
fn base_overhead(group: &mut BenchmarkGroup<WallTime>, size: i64) {
    let sink = Arc::new(AtomicI64::new(0));
    let benchmark_id = BenchmarkId::new("base_overhead", size);
    let burst_size = Arc::new(AtomicI64::new(0));

    let mut burst_producers = (0..PRODUCERS)
        .map(|_| {
            let sink = Arc::clone(&sink);
            let burst_size = Arc::clone(&burst_size);
            BurstProducer::new(move || {
                let burst_size = burst_size.load(Acquire);
                for _ in 0..burst_size {
                    sink.fetch_add(1, Release);
                }
            })
        })
        .collect::<Vec<BurstProducer>>();

    run_benchmark(
        group,
        benchmark_id,
        burst_size,
        sink,
        (size, 0),
        &burst_producers,
    );
    burst_producers.iter_mut().for_each(BurstProducer::stop);
}

/// Crossbeam channel MPSC benchmark
fn crossbeam_mpsc(
    group: &mut BenchmarkGroup<WallTime>,
    params: (i64, u64),
    param_description: &str,
) {
    // Use an AtomicI64 to count the number of events from the receiving thread
    let sink = Arc::new(AtomicI64::new(0));
    let (s, r) = bounded::<Event>(DATA_STRUCTURE_SIZE);

    let receiver = {
        let sink = Arc::clone(&sink);
        thread::spawn(move || loop {
            match r.try_recv() {
                Ok(event) => {
                    black_box(event.data);
                    sink.fetch_add(1, Release);
                }
                Err(Empty) => continue,
                Err(Disconnected) => break,
            }
        })
    };

    let benchmark_id = BenchmarkId::new("crossbeam_channel", param_description);
    let burst_size = Arc::new(AtomicI64::new(0));

    let mut burst_producers = (0..PRODUCERS)
        .map(|_| {
            let burst_size = Arc::clone(&burst_size);
            let s = s.clone();
            BurstProducer::new(move || {
                let burst_size = burst_size.load(Acquire);
                for data in 0..burst_size {
                    let mut event = Event {
                        data: black_box(data),
                    };
                    while let Err(Full(e)) = s.try_send(event) {
                        event = e;
                    }
                }
            })
        })
        .collect::<Vec<BurstProducer>>();

    drop(s); // Original send channel not used

    run_benchmark(
        group,
        benchmark_id,
        burst_size,
        sink,
        params,
        &burst_producers,
    );

    burst_producers.iter_mut().for_each(BurstProducer::stop);
    receiver.join().expect("Receiver should not panic");
}

/// BadBatch Disruptor MPSC benchmark using modern disruptor-rs inspired API
fn badbatch_mpsc_modern(
    group: &mut BenchmarkGroup<WallTime>,
    params: (i64, u64),
    param_description: &str,
) {
    let factory = || Event { data: 0 };
    // 使用 AtomicI64 计数接收到的事件数量
    let sink = Arc::new(AtomicI64::new(0));

    let processor = {
        let sink = Arc::clone(&sink);
        move |event: &mut Event, _sequence: i64, _end_of_batch: bool| {
            // 使用 black_box 避免死代码消除
            black_box(event.data);
            sink.fetch_add(1, Release);
            // 现代 API 处理器应该返回 () 而不是 Result
        }
    };

    let producer = build_multi_producer(DATA_STRUCTURE_SIZE, factory, BusySpinWaitStrategy)
        .handle_events_with(processor)
        .build();

    let benchmark_id = BenchmarkId::new("badbatch_modern", param_description);
    let burst_size = Arc::new(AtomicI64::new(0));

    let mut burst_producers = (0..PRODUCERS)
        .map(|_| {
            let burst_size = Arc::clone(&burst_size);
            let producer = producer.clone();
            BurstProducer::new(move || {
                let burst_size = burst_size.load(Acquire);
                // 根据批量大小选择不同的发布策略
                if burst_size <= 10 {
                    // 对于小批量，单个发布
                    for i in 0..burst_size {
                        producer.publish(|event| {
                            event.data = black_box(i);
                        });
                    }
                } else {
                    // 对于大批量，使用批量发布
                    producer.batch_publish(burst_size as usize, |batch| {
                        for (i, event) in batch.enumerate() {
                            event.data = black_box(i as i64);
                        }
                    });
                }
            })
        })
        .collect::<Vec<BurstProducer>>();

    drop(producer); // 原始生产者不使用

    run_benchmark(
        group,
        benchmark_id,
        burst_size,
        sink,
        params,
        &burst_producers,
    );

    burst_producers.iter_mut().for_each(BurstProducer::stop);
}

/// BadBatch Disruptor MPSC benchmark using traditional LMAX API
fn badbatch_mpsc_traditional(
    group: &mut BenchmarkGroup<WallTime>,
    params: (i64, u64),
    param_description: &str,
) {
    // 事件处理器实现
    struct BenchEventHandler {
        sink: Arc<AtomicI64>,
    }

    impl EventHandler<Event> for BenchEventHandler {
        fn on_event(
            &mut self,
            event: &mut Event,
            _sequence: i64,
            _end_of_batch: bool,
        ) -> badbatch::disruptor::Result<()> {
            // 使用 black_box 避免死代码消除
            black_box(event.data);
            self.sink.fetch_add(1, Release);
            Ok(())
        }
    }

    // 事件转换器实现
    struct BenchEventTranslator {
        data: i64,
    }

    impl EventTranslator<Event> for BenchEventTranslator {
        fn translate_to(&self, event: &mut Event, _sequence: i64) {
            event.data = self.data;
        }
    }

    // 使用 AtomicI64 计数接收到的事件数量
    let sink = Arc::new(AtomicI64::new(0));
    let factory = badbatch::disruptor::DefaultEventFactory::<Event>::new();
    let handler = BenchEventHandler {
        sink: Arc::clone(&sink),
    };

    // 创建并配置 Disruptor
    let disruptor = Arc::new(Mutex::new(Disruptor::new(
        factory,
        DATA_STRUCTURE_SIZE,
        ProducerType::Multi,
        Box::new(badbatch::disruptor::BlockingWaitStrategy::new()),
    )
    .expect("创建 Disruptor 失败")
    .handle_events_with(handler)
    .build()));

    // 启动 Disruptor
    disruptor.lock().unwrap().start().unwrap();

    let benchmark_id = BenchmarkId::new("badbatch_traditional", param_description);
    let burst_size = Arc::new(AtomicI64::new(0));

    let mut burst_producers = (0..PRODUCERS)
        .map(|_| {
            let burst_size = Arc::clone(&burst_size);
            let disruptor_clone = Arc::clone(&disruptor);
            BurstProducer::new(move || {
                let burst_size = burst_size.load(Acquire);
                for i in 0..burst_size {
                    let translator = BenchEventTranslator {
                        data: black_box(i),
                    };
                    disruptor_clone.lock().unwrap().publish_event(translator).unwrap();
                }
            })
        })
        .collect::<Vec<BurstProducer>>();

    run_benchmark(
        group,
        benchmark_id,
        burst_size,
        sink,
        params,
        &burst_producers,
    );

    burst_producers.iter_mut().for_each(BurstProducer::stop);
    
    // 关闭 Disruptor
    disruptor.lock().unwrap().shutdown().unwrap();
}

criterion_group!(mpsc, mpsc_benchmark);
criterion_main!(mpsc);
