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
use badbatch::disruptor::Producer;
// Ordering 已通过子模块引入，不需要显式导入

// 禁用调试日志
static DEBUG: bool = false;

/// 调试日志辅助函数
fn debug_log(message: &str) {
    if DEBUG {
        eprintln!("[DEBUG] {}", message);
    }
}

/// 只在关键事件时输出日志
fn key_debug_log(message: &str) {
    if DEBUG {
        eprintln!("[KEY] {}", message);
    }
}

use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

// BadBatch Disruptor imports
use badbatch::disruptor::{build_multi_producer, BusySpinWaitStrategy};

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
        debug_log("Creating new BurstProducer");
        let start_barrier = Arc::new(CachePadded::new(AtomicBool::new(false)));
        let stop = Arc::new(CachePadded::new(AtomicBool::new(false)));

        let join_handle = {
            let stop = Arc::clone(&stop);
            let start_barrier = Arc::clone(&start_barrier);
            debug_log("Spawning BurstProducer thread");
            thread::spawn(move || {
                key_debug_log("THREAD: BurstProducer STARTED");
                while !stop.load(Acquire) {
                    // Check for start signal with periodic timeout
                    let mut last_wait_log = Instant::now();
                    let mut counter = 0;
                    while start_barrier.compare_exchange(true, false, Acquire, Relaxed).is_err() {
                        if stop.load(Acquire) {
                            key_debug_log("THREAD: BurstProducer STOPPING DURING WAIT");
                            return;
                        }
                        
                        // Log wait status every 3 seconds to reduce noise
                        if last_wait_log.elapsed() > Duration::from_secs(3) {
                            counter += 1;
                            key_debug_log(&format!("THREAD: BurstProducer WAITING FOR START SIGNAL {}s", 
                                                counter * 3));
                            last_wait_log = Instant::now();
                        }
                    }
                    
                    key_debug_log("THREAD: EXECUTING BURST PRODUCTION");
                    produce_one_burst();
                    key_debug_log("THREAD: BURST PRODUCTION COMPLETED");
                }
                key_debug_log("THREAD: PRODUCER EXITING MAIN LOOP");
            })
        };

        Self {
            start_barrier,
            stop,
            join_handle: Some(join_handle),
        }
    }

    fn start(&self) {
        key_debug_log("BurstProducer: START SIGNAL SENT");
        self.start_barrier.store(true, Release);
    }

    fn stop(&mut self) {
        key_debug_log("BurstProducer: STOP SIGNAL SENT");
        self.stop.store(true, Release);
        
        // 等待线程完成
        let start_wait = Instant::now();
        let result = self.join_handle
            .take()
            .unwrap()
            .join();
            
        match result {
            Ok(_) => key_debug_log(&format!("BurstProducer: THREAD JOINED SUCCESSFULLY after {:?}", start_wait.elapsed())),
            Err(_) => key_debug_log("BurstProducer: THREAD JOIN PANICKED"),
        }
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
    debug_log(&format!("Starting benchmark with params: {:?}", params));
    
    group.bench_with_input(benchmark_id, &params, move |b, (size, pause_ms)| {
        debug_log(&format!("Running benchmark with size={}, pause={}ms", size, pause_ms));
        
        b.iter_custom(|iters| {
            debug_log(&format!("Starting benchmark iteration, iters={}", iters));
            burst_size.store(*size, Release);
            let count = black_box(*size * burst_producers.len() as i64);
            debug_log(&format!("Expected event count: {}", count));
            
            pause(*pause_ms);
            let start = Instant::now();
            for iter_num in 0..iters {
                debug_log(&format!("Iteration {}/{} starting", iter_num + 1, iters));
                
                // Reset counter
                let prev_count = sink.swap(0, Release);
                debug_log(&format!("Reset sink counter from {} to 0", prev_count));
                
                // Start producers
                debug_log("Starting all burst producers");
                burst_producers.iter().for_each(BurstProducer::start);
                
                // Wait for all producers to finish publication with timeout
                debug_log("Waiting for all events to be processed");
                let wait_start = Instant::now();
                let mut last_log_time = Instant::now();
                
                while sink.load(Acquire) != count {
                    // Check for timeout (10 seconds)
                    if wait_start.elapsed() > Duration::from_secs(10) {
                        let current = sink.load(Acquire);
                        key_debug_log(&format!("DEADLOCK DETECTED: Only {}/{} events processed after 10s", current, count));
                        break;
                    }
                    
                    // Log progress every 1000ms
                    if last_log_time.elapsed() > Duration::from_millis(1000) {
                        let current = sink.load(Acquire);
                        key_debug_log(&format!("WAIT LOOP: {}/{} events processed ({:.1}%)", 
                                          current, count, (current as f64 / count as f64) * 100.0));
                        last_log_time = Instant::now();
                    }
                }
                
                let final_count = sink.load(Acquire);
                if final_count == count {
                    debug_log(&format!("Iteration {}/{} completed successfully", iter_num + 1, iters));
                } else {
                    debug_log(&format!("Iteration {}/{} INCOMPLETE: only {}/{} events processed", 
                                       iter_num + 1, iters, final_count, count));
                }
            }
            
            let elapsed = start.elapsed();
            debug_log(&format!("Benchmark completed in {:?}", elapsed));
            elapsed
        })
    });
    
    debug_log(&format!("Benchmark finished with params: {:?}", params));
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

    debug_log("Traditional: Starting run_benchmark call");
    run_benchmark(
        group,
        benchmark_id,
        burst_size,
        sink,
        params,
        &burst_producers,
    );
    debug_log("Traditional: run_benchmark completed");

    burst_producers.iter_mut().for_each(BurstProducer::stop);
    receiver.join().expect("Receiver should not panic");
}

/// BadBatch Disruptor MPSC benchmark using modern API
fn badbatch_mpsc_modern(
    group: &mut BenchmarkGroup<WallTime>,
    params: (i64, u64),
    param_description: &str,
) {
    debug_log(&format!("Starting badbatch_mpsc_modern with params: {:?}, desc: {}", params, param_description));
    
    // 工厂函数
    let factory = || {
        debug_log("Factory creating new Event");
        Event { data: 0 }
    };
    
    // 计数器
    let sink = Arc::new(AtomicI64::new(0));
    debug_log("Created sink counter");
    
    // 处理器函数
    let processor = {
        let sink = Arc::clone(&sink);
        debug_log("Creating event processor function");
        move |event: &mut Event, sequence: i64, end_of_batch: bool| {
            debug_log(&format!("Processing event: seq={}, data={}, end_batch={}", sequence, event.data, end_of_batch));
            black_box(event.data);
            let prev = sink.fetch_add(1, Release);
            if prev % 100 == 0 || prev + 1 == params.0 * PRODUCERS as i64 {
                debug_log(&format!("Event processed: {} total events so far", prev + 1));
            }
        }
    };
    
    // 创建 Disruptor
    debug_log("Building multi-producer disruptor");
    // 创建Disruptor构建器
    let builder = build_multi_producer(DATA_STRUCTURE_SIZE, factory, BusySpinWaitStrategy::new())
        .handle_events_with(processor);
        
    // 构建并自动启动消费者线程
    debug_log("Building and starting disruptor...");
    let disruptor_handle = builder.build();
    debug_log("Disruptor built and started successfully");
    
    // 获取生产者
    let producer = disruptor_handle.create_producer();
    debug_log("Producer obtained from handle");
    
    // 保持disruptor_handle活跃
    let _disruptor_handle = disruptor_handle;
    
    let benchmark_id = BenchmarkId::new("badbatch_modern", param_description);
    let burst_size = Arc::new(AtomicI64::new(0));
    debug_log("Created burst size atomic counter");
    
    // 创建生产者
    debug_log(&format!("Creating {} burst producers", PRODUCERS));
    let mut burst_producers = (0..PRODUCERS)
        .map(|producer_id| {
            let burst_size = Arc::clone(&burst_size);
            let mut producer = producer.clone();
            debug_log(&format!("Created cloned producer {}", producer_id));
            
            BurstProducer::new(move || {
                let burst_size = burst_size.load(Acquire);
                debug_log(&format!("Producer {}: starting batch_publish of {} events", producer_id, burst_size));
                producer.batch_publish(burst_size as usize, |iter| {
                    debug_log(&format!("Producer {}: filling {} events with data", producer_id, burst_size));
                    for (i, e) in iter.enumerate() {
                        e.data = black_box(i as i64);
                    }
                    debug_log(&format!("Producer {}: completed filling events", producer_id));
                });
                debug_log(&format!("Producer {}: batch_publish completed", producer_id));
            })
        })
        .collect::<Vec<BurstProducer>>();
    debug_log(&format!("Created {} burst producers successfully", burst_producers.len()));
    
    // 原始生产者不再使用
    debug_log("Traditional: Dropping original producer");
    drop(producer);
    debug_log("Traditional: Original producer dropped");
    
    debug_log("Traditional: Starting run_benchmark call");
    run_benchmark(
        group,
        benchmark_id,
        burst_size,
        sink,
        params,
        &burst_producers,
    );
    debug_log("Traditional: run_benchmark completed");
    
    // 清理资源
    debug_log("Traditional: Stopping all burst producers");
    let producer_count = burst_producers.len();
    for (i, producer) in burst_producers.iter_mut().enumerate() {
        debug_log(&format!("Traditional: Stopping producer {}/{}", i+1, producer_count));
        producer.stop();
        debug_log(&format!("Traditional: Producer {}/{} stopped successfully", i+1, producer_count));
    }
    debug_log("Traditional: All burst producers stopped successfully");
}

/// BadBatch Disruptor MPSC benchmark using reference implementation pattern
fn badbatch_mpsc_traditional(
    group: &mut BenchmarkGroup<WallTime>,
    params: (i64, u64),
    param_description: &str,
) {
    debug_log(&format!("Starting badbatch_mpsc_traditional with params: {:?}, desc: {}", params, param_description));
    
    // 使用简单闭包作为工厂函数
    let factory = || {
        debug_log("Traditional: Factory creating new Event");
        Event { data: 0 }
    };
    
    // 使用 AtomicI64 计数接收到的事件数量
    let sink = Arc::new(AtomicI64::new(0));
    debug_log("Traditional: Created sink counter");
    
    // 使用可变引用创建处理器函数
    let processor = {
        let sink = Arc::clone(&sink);
        debug_log("Traditional: Creating event processor function");
        move |event: &mut Event, sequence: i64, end_of_batch: bool| {
            key_debug_log(&format!("EVENT PROCESSOR: Processing event seq={}, data={}, end_batch={}", sequence, event.data, end_of_batch));
            // 使用 black_box 避免死代码消除
            black_box(event.data);
            let prev = sink.fetch_add(1, Release);
            if prev == 0 || prev % 10 == 0 || prev + 1 == params.0 * PRODUCERS as i64 {
                key_debug_log(&format!("EVENT PROCESSOR: Counter incremented to {}", prev + 1));
            }
        }
    };
    
    // 使用 BusySpinWaitStrategy::new() 而不是简单的 BusySpinWaitStrategy
    key_debug_log("DISRUPTOR: Building multi-producer with BusySpinWaitStrategy");
    // 创建构建器并添加事件处理器
    let builder = build_multi_producer(DATA_STRUCTURE_SIZE, factory, BusySpinWaitStrategy::new())
        .handle_events_with(processor);
        
    // 构建并启动Disruptor（build方法会自动启动消费者线程）
    key_debug_log("DISRUPTOR: Building and starting disruptor...");
    let disruptor_handle = builder.build();
    key_debug_log("DISRUPTOR: Disruptor built and consumer thread started successfully");
    
    // 使用正确的API获取生产者
    let producer = disruptor_handle.create_producer();
    key_debug_log("DISRUPTOR: Producer obtained from handle");
    
    // 保持disruptor_handle活跃，防止在测试期间费掉
    let _disruptor_handle = disruptor_handle;
    
    let benchmark_id = BenchmarkId::new("badbatch_traditional", param_description);
    let burst_size = Arc::new(AtomicI64::new(0));
    debug_log("Traditional: Created burst size atomic counter");
    
    // 创建 burst producers，始终使用 batch_publish
    debug_log(&format!("Traditional: Creating {} burst producers", PRODUCERS));
    let mut burst_producers = (0..PRODUCERS)
        .map(|producer_id| {
            let burst_size = Arc::clone(&burst_size);
            let mut producer = producer.clone();
            debug_log(&format!("Traditional: Created cloned producer {}", producer_id));
            
            BurstProducer::new(move || {
                let burst_size = burst_size.load(Acquire);
                key_debug_log(&format!("PRODUCER {}: Starting batch_publish of {} events", producer_id, burst_size));
                producer.batch_publish(burst_size as usize, |iter| {
                    // 填充事件数据
                    for (i, e) in iter.enumerate() {
                        e.data = black_box(i as i64);
                    }
                    key_debug_log(&format!("PRODUCER {}: Data filled for {} events", producer_id, burst_size));
                });
                key_debug_log(&format!("PRODUCER {}: batch_publish completed", producer_id));
            })
        })
        .collect::<Vec<BurstProducer>>();
    debug_log(&format!("Traditional: Created {} burst producers successfully", burst_producers.len()));
    
    // 原始生产者不再使用
    debug_log("Traditional: Dropping original producer");
    drop(producer);
    debug_log("Traditional: Original producer dropped");
    
    debug_log("Traditional: Starting run_benchmark call");
    run_benchmark(
        group,
        benchmark_id,
        burst_size,
        sink,
        params,
        &burst_producers,
    );
    debug_log("Traditional: run_benchmark completed");
    
    // 清理资源
    debug_log("Traditional: Stopping all burst producers");
    let producer_count = burst_producers.len();
    for (i, producer) in burst_producers.iter_mut().enumerate() {
        debug_log(&format!("Traditional: Stopping producer {}/{}", i+1, producer_count));
        producer.stop();
        debug_log(&format!("Traditional: Producer {}/{} stopped successfully", i+1, producer_count));
    }
    debug_log("Traditional: All burst producers stopped successfully");
}

criterion_group!(mpsc, mpsc_benchmark);
criterion_main!(mpsc);
