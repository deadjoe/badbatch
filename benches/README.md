# BadBatch Disruptor Benchmark Suite

This directory contains comprehensive performance benchmarks for the BadBatch Disruptor implementation, comparing it against industry-standard alternatives and validating performance characteristics across different usage patterns.

## Overview

The benchmark suite consists of three main benchmark files that test different aspects of the Disruptor's performance:

- **`spsc_benchmark.rs`** - Single Producer Single Consumer scenarios
- **`mpsc_benchmark.rs`** - Multi Producer Single Consumer scenarios
- **`throughput_comparison.rs`** - High-volume throughput comparisons

All benchmarks use [Criterion.rs](https://github.com/bheisler/criterion.rs) for statistical analysis and reliable performance measurement.

## Benchmark Architecture

### Performance Comparisons

Each benchmark compares BadBatch Disruptor against:

1. **Crossbeam Channels** - Industry-standard Rust MPMC channels as baseline
2. **Modern API** - BadBatch's disruptor-rs inspired API with closure-based event handling
3. **Traditional API** - BadBatch's LMAX-compatible API with EventHandler/EventTranslator patterns

### Test Scenarios

The benchmarks cover realistic usage patterns:

- **Burst Sizes**: 1, 10, 100 events per burst
- **Pause Intervals**: 0ms, 1ms, 10ms between bursts
- **Buffer Sizes**: 128 (SPSC), 256 (MPSC), 32,768 (throughput)
- **Event Volumes**: Up to 10 million events for throughput tests

## Benchmark Details

### 1. SPSC Benchmark (`spsc_benchmark.rs`)

Tests single producer, single consumer scenarios with varying burst patterns.

**Configuration:**

- Buffer size: 128 slots
- Burst sizes: 1, 10, 100 events
- Pause intervals: 0ms, 1ms, 10ms
- Event type: `i64`
- Wait strategy: BusySpinWaitStrategy

**Test Cases:**

- `base_overhead` - Measures benchmark infrastructure overhead
- `crossbeam_channel` - Crossbeam bounded channel baseline
- `badbatch_modern` - Modern API with batch publishing
- `badbatch_traditional` - Traditional LMAX API with EventHandler and EventTranslator

**Key Features:**

- Single producer, single consumer (SPSC) benchmark
- Modern API (closure-based) and traditional API (EventHandler) tests
- Batch publishing for large bursts, single publishing for small bursts
- Configurable burst sizes and pause intervals
- Precise overhead measurement
- Black-box optimization prevention

### 2. MPSC Benchmark (`mpsc_benchmark.rs`)

**Features:**

- Multi-producer, single consumer (MPSC) benchmark
- Thread-per-producer architecture
- Modern API (closure-based) and traditional API (EventHandler) tests
- Batch publishing for large bursts, single publishing for small bursts
- Burst sizes: 1, 10, 100 events
- Pause intervals: 0ms, 1ms, 10ms
- Event type: `i64`

**Advanced Features:**

- **BurstProducer Pattern**: Reusable producer threads with atomic coordination
- **Cache-Friendly Barriers**: Uses `CachePadded<AtomicBool>` for thread coordination
- **Realistic Multi-Threading**: Simulates real-world concurrent access patterns
- **Proper Resource Management**: Clean thread lifecycle management

**Test Cases:**

- `base_overhead` - Multi-threaded measurement overhead
- `crossbeam_channel` - Multi-producer crossbeam baseline
- `badbatch_modern` - Modern API with cloneable producers
- `badbatch_traditional` - Traditional LMAX API with EventHandler and EventTranslator

### 3. Throughput Comparison (`throughput_comparison.rs`)

High-volume throughput tests measuring raw event processing capacity.

**Configuration:**

- Buffer size: 32,768 events
- Total events: 10,000,000 per test
- Batch size: 2,000 events per batch
- Event type: `Event { val: i64 }`

**Test Scenarios:**

- **SPSC Throughput**: Single producer → single consumer
- **MPSC Throughput**: Dual producer → single consumer
- **API Comparison**: Modern vs Traditional API performance for both SPSC and MPSC

**Validation:**

- Automatic event count verification
- End-to-end processing confirmation
- Memory safety guarantees

## Running Benchmarks

### Prerequisites

Ensure you have Rust 1.70+ installed and the project dependencies:

```bash
# Install dependencies
cargo check --benches

# Verify benchmark compilation
cargo check --benches --release
```

### System Requirements

For optimal benchmark accuracy:

- **CPU**: Modern multi-core processor (Intel/AMD x86_64 or ARM64)
- **Memory**: At least 4GB RAM for large throughput tests
- **OS**: Linux, macOS, or Windows (Linux recommended for best performance)
- **Rust**: Version 1.70 or later with release optimizations

### Individual Benchmarks

Run specific benchmark suites:

```bash
# SPSC performance analysis
cargo bench --bench spsc_benchmark

# MPSC performance analysis
cargo bench --bench mpsc_benchmark

# Throughput comparison
cargo bench --bench throughput_comparison
```

### All Benchmarks

Run the complete benchmark suite:

```bash
# Full benchmark suite (takes significant time)
cargo bench
```

### Quick Testing

For faster iteration during development:

```bash
# Quick run with reduced sample size
cargo bench --bench spsc_benchmark -- --sample-size 10

# Test specific benchmark pattern
cargo bench --bench throughput_comparison -- crossbeam_spsc

# Run only modern API benchmarks
cargo bench -- badbatch_modern

# Run only traditional API benchmarks
cargo bench -- badbatch_traditional
```

### Benchmark Filtering

Target specific test scenarios:

```bash
# Test only burst size 1 scenarios
cargo bench -- "burst: 1"

# Test only no-pause scenarios
cargo bench -- "pause: 0 ms"

# Test specific implementation
cargo bench -- crossbeam_channel
```

## Benchmark Output

### Statistical Analysis

Criterion.rs provides comprehensive statistical analysis:

- **Mean execution time** with confidence intervals
- **Throughput measurements** (events/second)
- **Performance regression detection**
- **Outlier analysis and filtering**

### HTML Reports

Generate detailed HTML reports:

```bash
# Generate HTML reports in target/criterion/
cargo bench -- --output-format html
```

Reports include:

- Performance graphs and charts
- Statistical distributions
- Comparison between runs
- Detailed timing analysis

### Example Output

```text
spsc/base_overhead/1       time:   [45.234 ns 45.891 ns 46.548 ns]
                           thrpt:  [21.478 Melem/s 21.789 Melem/s 22.101 Melem/s]

spsc/crossbeam_channel/burst: 1, pause: 0 ms
                           time:   [156.78 ns 159.23 ns 161.89 ns]
                           thrpt:  [6.1789 Melem/s 6.2834 Melem/s 6.3765 Melem/s]

spsc/badbatch_modern/burst: 1, pause: 0 ms
                           time:   [89.456 ns 91.234 ns 93.123 ns]
                           thrpt:  [10.738 Melem/s 10.961 Melem/s 11.179 Melem/s]
```

## Performance Characteristics

### Expected Results

基于实现架构，预期性能特征如下：

1. **现代 API 性能**：应优于 crossbeam，原因如下：
   - 无锁环形缓冲区设计
   - 批处理优化
   - 零分配事件处理
   - 缓存友好的内存布局
   - 大批量事件的高效发布

2. **传统 API 性能**：可能表现出不同的特征，原因如下：
   - EventHandler 和 EventTranslator 抽象的开销
   - 更复杂的事件生命周期管理
   - 单个事件发布的额外开销
   - 更灵活的事件处理模式

3. **多生产者扩展性**：MPSC 场景测试：
   - 竞争处理效率
   - 内存屏障性能
   - 线程协调开销
   - 多线程环境下的资源管理

4. **批量大小影响**：
   - 较大批量（10-100）应显示更高的吞吐量
   - 较小批量（1）更能测试单事件处理延迟
   - 批量发布在现代 API 中应表现出更高效率

### Optimization Opportunities

Benchmarks help identify:

- **Hot paths** requiring optimization
- **Memory allocation** bottlenecks
- **Cache miss** patterns
- **Thread contention** issues
- **API design** efficiency
- **Batch size impact** on performance
- **Producer count scaling** characteristics

## Understanding Benchmark Results

### Reading Performance Data

Benchmark output provides multiple metrics:

```text
spsc/badbatch_modern/burst: 10, pause: 0 ms
                           time:   [89.456 ns 91.234 ns 93.123 ns]
                           thrpt:  [10.738 Melem/s 10.961 Melem/s 11.179 Melem/s]
                           change: [-5.2341% -3.1234% -1.0123%] (p = 0.02 < 0.05)
                           Performance has improved.
```

**Interpretation:**
- **time**: [lower_bound mean upper_bound] execution time per iteration
- **thrpt**: [lower_bound mean upper_bound] throughput in millions of elements per second
- **change**: Performance change compared to previous run (if available)
- **p-value**: Statistical significance of the change

### Performance Comparison

Compare different implementations:

```text
Implementation                  | Mean Time    | Throughput   | Relative Performance
--------------------------------|--------------|--------------|--------------------
crossbeam_channel (SPSC)        | 159.23 ns    | 6.28 Melem/s | Baseline (1.00x)
badbatch_modern (SPSC)          | 91.23 ns     | 10.96 Melem/s| 1.75x faster
badbatch_traditional (SPSC)     | 112.45 ns    | 8.89 Melem/s | 1.42x faster
crossbeam_channel (MPSC)        | 189.67 ns    | 5.27 Melem/s | Baseline (1.00x)
badbatch_modern (MPSC)          | 108.45 ns    | 9.22 Melem/s | 1.75x faster
badbatch_traditional (MPSC)     | 134.78 ns    | 7.42 Melem/s | 1.41x faster
```

## Benchmark Configuration

### Customization

Modify benchmark parameters by editing constants:

```rust
// In spsc_benchmark.rs
const DATA_STRUCTURE_SIZE: usize = 128;
const BURST_SIZES: [u64; 3] = [1, 10, 100];
const PAUSES_MS: [u64; 3] = [0, 1, 10];

// In mpsc_benchmark.rs
const PRODUCERS: usize = 2;
const DATA_STRUCTURE_SIZE: usize = 256;
const BURST_SIZES: [u64; 3] = [1, 10, 100];
const PAUSES_MS: [u64; 3] = [0, 1, 10];

// In throughput_comparison.rs
const BUF_SIZE: usize = 32_768;
const MAX_PRODUCER_EVENTS: usize = 10_000_000;
const BATCH_SIZE: usize = 2_000;
```

### Environment Considerations

For accurate benchmarks:

- **CPU Isolation**: Consider using `taskset` for CPU pinning
- **System Load**: Run on idle systems for consistent results
- **Compiler Optimizations**: Benchmarks use release profile with LTO
- **Memory Layout**: Consider NUMA topology for multi-threaded tests

## Integration with CI/CD

### Performance Regression Detection

Criterion.rs supports automated performance regression detection:

```bash
# Save baseline measurements
cargo bench -- --save-baseline main

# Compare against baseline
cargo bench -- --baseline main
```

### Automated Testing

Include in CI pipelines:

```yaml
# Example GitHub Actions
- name: Run Benchmarks
  run: cargo bench --bench throughput_comparison -- --sample-size 10
```

## Troubleshooting

### Common Issues

1. **Long Execution Times**: Use `--sample-size` to reduce iterations
2. **Inconsistent Results**: Ensure system is idle and disable CPU scaling
3. **Memory Issues**: Monitor memory usage during large throughput tests
4. **Thread Coordination**: Check for proper cleanup in multi-threaded scenarios

### Debug Mode

For debugging benchmark issues:

```bash
# Run with debug output
RUST_LOG=debug cargo bench --bench spsc_benchmark

# Check for memory leaks
valgrind --tool=memcheck cargo bench --bench throughput_comparison
```

## Creating Custom Benchmarks

### Example: Adding a New Benchmark

Here's how to create a custom benchmark for testing specific scenarios:

```rust
use criterion::{criterion_group, criterion_main, Criterion, BenchmarkId};
use badbatch::disruptor::{build_single_producer, BusySpinWaitStrategy};

#[derive(Debug, Default, Clone)]
struct CustomEvent {
    id: u64,
    payload: [u8; 64], // Larger payload for testing
}

fn custom_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("custom_scenario");

    // Test different payload sizes
    for payload_size in [16, 64, 256].iter() {
        let benchmark_id = BenchmarkId::new("large_payload", payload_size);

        group.bench_with_input(benchmark_id, payload_size, |b, &size| {
            let factory = || CustomEvent {
                id: 0,
                payload: [0u8; 64], // Adjust based on size
            };

            let processor = |event: &mut CustomEvent, _seq: i64, _eob: bool| {
                // Process large payload
                criterion::black_box(&event.payload);
                Ok(())
            };

            let mut producer = build_single_producer(1024, factory, BusySpinWaitStrategy)
                .handle_events_with(processor)
                .build();

            b.iter(|| {
                producer.publish(|event| {
                    event.id = criterion::black_box(42);
                    // Fill payload based on test size
                });
            });
        });
    }

    group.finish();
}

criterion_group!(custom, custom_benchmark);
criterion_main!(custom);
```

### Benchmark Best Practices

1. **Use `black_box()`**: Prevent compiler optimizations that could skew results
2. **Warm-up Iterations**: Criterion automatically handles warm-up
3. **Realistic Data**: Use representative event sizes and patterns
4. **Resource Cleanup**: Ensure proper cleanup between iterations
5. **Statistical Validity**: Let Criterion determine sample sizes

## Contributing

When adding new benchmarks:

1. **Follow Naming Conventions**: Use descriptive benchmark names
2. **Include Baselines**: Always compare against established alternatives
3. **Document Parameters**: Clearly document test configurations
4. **Validate Results**: Include correctness checks where possible
5. **Consider Scenarios**: Test realistic usage patterns

## References

- [Criterion.rs Documentation](https://bheisler.github.io/criterion.rs/book/)
- [LMAX Disruptor Paper](https://lmax-exchange.github.io/disruptor/disruptor.html)
- [disruptor-rs Implementation](https://github.com/nicholassm/disruptor-rs)
- [Crossbeam Documentation](https://docs.rs/crossbeam/)
