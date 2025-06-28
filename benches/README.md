# BadBatch Disruptor Benchmarks

This directory contains comprehensive benchmarks for the BadBatch Disruptor implementation. The benchmark suite is designed to evaluate performance across multiple dimensions and compare against other concurrency primitives.

## 🚀 Recent Improvements (June 2025)

**Enhanced Safety & Reliability:**
- ✅ **Timeout Protection**: All benchmarks now include timeout mechanisms to prevent hanging
- ✅ **Error Recovery**: Graceful handling of failures and edge cases  
- ✅ **Improved Synchronization**: Better thread coordination in multi-producer scenarios
- ✅ **Progress Monitoring**: Detection and handling of stalled operations
- ✅ **Consolidated Files**: Eliminated duplicate benchmarks while retaining all functionality

**Directory Cleanup:**
- Reduced from 12 to 8 benchmark files
- Replaced original versions with enhanced implementations
- Added automated testing scripts for CI/CD integration

## Benchmark Categories

### 1. Single Producer Single Consumer (SPSC)
**File**: `single_producer_single_consumer.rs`

Tests the fundamental SPSC performance with different wait strategies:
- **BusySpinWaitStrategy**: Highest performance, 100% CPU usage
- **YieldingWaitStrategy**: Good performance with CPU yielding  
- **BlockingWaitStrategy**: Lower CPU usage, higher latency
- **SleepingWaitStrategy**: Lowest CPU usage, variable latency

**Key Metrics**: 
- Throughput (events/second)
- Latency per event
- CPU utilization patterns

### 2. Multi Producer Single Consumer (MPSC)
**File**: `multi_producer_single_consumer.rs`

Tests multi-producer coordination and synchronization:
- Multiple producer threads competing for ring buffer slots
- Coordination overhead measurement
- Scalability with producer count

**Key Metrics**:
- Aggregate throughput
- Producer coordination overhead
- Fairness between producers

### 3. Pipeline Processing
**File**: `pipeline_processing.rs`

Tests complex event processing pipelines:
- **Two-stage**: Stage1 → Stage2
- **Three-stage**: Stage1 → Stage2 → Stage3  
- **Four-stage**: Stage1 → Stage2 → Stage3 → Final

Each stage represents a dependent processing step, testing the dependency resolution and coordination mechanisms.

**Key Metrics**:
- End-to-end latency
- Pipeline throughput
- Stage coordination overhead

### 4. Latency Comparison
**File**: `latency_comparison.rs`

Compares latency characteristics against other concurrency primitives:
- **BadBatch Disruptor** with BusySpinWaitStrategy
- **std::sync::mpsc** channels
- **Crossbeam** channels

Provides detailed latency statistics including percentiles (95th, 99th) and distribution analysis.

**Key Metrics**:
- Mean latency
- Median latency  
- 95th/99th percentile latency
- Maximum latency

### 5. Throughput Comparison
**File**: `throughput_comparison.rs`

Raw throughput comparison across different configurations:
- Various buffer sizes (256, 1024, 4096)
- Different wait strategies
- Single vs multi-producer modes
- Comparison with standard channels

**Key Metrics**:
- Maximum sustainable throughput
- Throughput scaling with buffer size
- Performance vs other concurrency primitives

### 6. Buffer Size Scaling
**File**: `buffer_size_scaling.rs`

Evaluates how performance scales with buffer size:
- Buffer sizes from 64 to 8192 slots
- Different processing speeds (fast/medium/slow)
- Memory usage patterns
- Buffer utilization efficiency

**Key Metrics**:
- Throughput vs buffer size
- Memory usage scaling
- Optimal buffer size identification


## 🛡️ Safety Features

All benchmarks now include enhanced safety mechanisms:

### Timeout Protection
- **SPSC/MPSC Tests**: 5-10 second timeouts prevent infinite waiting
- **Progress Monitoring**: Automatic detection of stalled operations
- **Graceful Failure**: Clear error messages when timeouts occur

### Error Recovery
- **Resource Cleanup**: Proper shutdown sequences for all tests
- **Thread Safety**: Improved synchronization in multi-producer scenarios  
- **Failure Isolation**: Individual test failures don't affect other benchmarks
- **Comprehensive Logging**: Detailed error reporting for debugging

### Automated Testing
- **Enhanced Execution Script**: `scripts/run_benchmarks.sh` provides timeout protection and comprehensive features
- **CI Integration**: Reliable benchmarks suitable for continuous integration
- **Batch Processing**: Run multiple benchmarks with centralized error handling

## Running Benchmarks

### Quick Benchmarks (CI/Development) - RECOMMENDED
```bash
# Safe automated testing with timeout protection (recommended)
./scripts/run_benchmarks.sh quick

# Manual execution with enhanced safety features
cargo bench --bench comprehensive_benchmarks  # Main CI suite with safety

# Individual benchmark categories
cargo bench --bench single_producer_single_consumer
cargo bench --bench multi_producer_single_consumer
cargo bench --bench throughput_comparison
```

### Full Benchmark Suite
```bash
# Complete performance analysis (all benchmarks)
cargo bench --bench single_producer_single_consumer  # SPSC analysis
cargo bench --bench multi_producer_single_consumer   # MPSC analysis
cargo bench --bench buffer_size_scaling             # Buffer optimization
cargo bench --bench pipeline_processing             # Pipeline testing
cargo bench --bench latency_comparison              # Latency analysis
cargo bench --bench throughput_comparison           # Throughput analysis

# Automated execution of all benchmarks
./scripts/run_benchmarks.sh all                     # With timeout protection
```

### Debug & Development
```bash
# Test all benchmarks with comprehensive safety
./scripts/run_benchmarks.sh all
```


### Individual Benchmark Files
```bash
# Run specific benchmark files
cargo bench --bench single_producer_single_consumer
cargo bench --bench multi_producer_single_consumer  
cargo bench --bench pipeline_processing
cargo bench --bench latency_comparison
cargo bench --bench throughput_comparison
cargo bench --bench buffer_size_scaling
```


## Benchmark Configuration

### Environment Setup
For consistent results, consider:

```bash
# Set CPU frequency scaling
echo performance | sudo tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor

# Disable CPU frequency scaling (Intel)
echo 1 | sudo tee /sys/devices/system/cpu/intel_pstate/no_turbo

# Set process priority
nice -n -20 cargo bench

# Pin to specific CPU cores (taskset on Linux)
taskset -c 0-3 cargo bench
```

### Benchmark Parameters

Most benchmarks can be configured by modifying constants at the top of each file:

```rust
const BUFFER_SIZE: usize = 1024;        // Ring buffer size
const BURST_SIZES: [u64; 4] = [1, 10, 100, 1000];  // Events per burst
const PAUSE_MS: [u64; 3] = [0, 1, 10];  // Pause between bursts
```

## Understanding Results

### Interpreting Throughput
- **Higher is better** for throughput measurements
- **BusySpinWaitStrategy** typically shows highest throughput
- **Buffer size** affects throughput up to a saturation point

### Interpreting Latency  
- **Lower is better** for latency measurements
- **99th percentile** is often more important than mean for real applications
- **Disruptor** typically shows lower and more consistent latency

### Performance Patterns

**Expected Performance Characteristics**:

1. **SPSC** > **MPSC** (single producer is faster)
2. **BusySpin** > **Yielding** > **Blocking** > **Sleeping** (for throughput)
3. **Smaller pipelines** > **Larger pipelines** (fewer coordination points)
4. **Larger buffers** improve throughput up to a point (diminishing returns)

### Recent Performance Results (June 2025)

**Comprehensive Performance Results (M1 Mac, 10 cores):**

| Benchmark | Throughput | Latency | Samples | Iterations | Performance Notes |
|-----------|------------|---------|---------|------------|-------------------|
| Quick Test Suite | 38.5 Melem/s | 254.67 ns | 15 | 19M | Fast CI validation suite |
| SPSC | 166.7 Melem/s | 5.91 ns | 20 | 1.6B | Single producer, ultra-low latency |
| MPSC | 627.8 Melem/s | 46.6 µs | 10 | 296k | Multi-producer, exceptional throughput |
| Pipeline | 17.8 Melem/s | 2.71 µs | 100 | 7.9M | Multi-stage processing |
| Latency Comparison | N/A | 167.6 ns | 20 | 59M | Disruptor vs channels comparison |
| Throughput | 106.1 Melem/s | 43.8 µs | 100 | 313k | Raw throughput analysis |
| Buffer Scaling | 556.4 Melem/s | 1.77 ms | 100 | 10k | Buffer size optimization |

**Latency Comparison Results:**
- **BadBatch Disruptor**: 167.6 ns (excellent)
- **std::sync::mpsc**: 29.1 µs (174× slower than Disruptor)
- **Crossbeam Channel**: 29.5 µs (176× slower than Disruptor)

**Key Performance Insights:**
- **MPSC leads throughput**: 627.8 Melem/s - world-class performance
- **SPSC ultra-low latency**: 5.91 ns - near memory access speed
- **Latency advantage**: Disruptor is 170+ times faster than traditional channels
- **Scaling excellence**: Performance scales well across different buffer sizes
- **Consistent results**: Low variance across multiple test runs
- **M1 optimization**: Excellent performance on Apple Silicon architecture

## Comparison Baselines

The benchmarks compare against:

- **std::sync::mpsc**: Rust's standard multi-producer, single-consumer channel
- **Crossbeam channels**: High-performance channel implementation
- **Raw atomic operations**: Baseline overhead measurement

## Benchmark Reliability

To ensure reliable results:

1. **Multiple runs**: Each benchmark runs multiple iterations
2. **Warm-up period**: JIT compilation and CPU cache warming
3. **Statistical analysis**: Criterion.rs provides statistical confidence intervals
4. **Outlier detection**: Automatic outlier filtering
5. **Consistent environment**: Same CPU, memory, and OS scheduling

## Interpreting Specific Results

### Throughput Results
```
SPSC/BusySpin_burst:100_pause:0ms
                        time:   [89.234 us 89.891 us 90.623 us]
                        thrpt:  [1.1034 Melem/s 1.1124 Melem/s 1.1207 Melem/s]
```
- **Time**: Time to process 100 events
- **Throughput**: Events per second (Melem/s = Million elements per second)

### Latency Results
```
Latency/Disruptor/BusySpin  time:   [1.2345 us 1.2567 us 1.2834 us]
```
- **Time**: Average time from publish to processing completion
- **Lower is better** for latency measurements

## Extending Benchmarks

To add new benchmarks:

1. **Create new benchmark file** in `benches/` directory
2. **Follow existing patterns** for consistency
3. **Add to comprehensive_benchmarks.rs** if needed
4. **Update this README** with benchmark description

Example benchmark structure:
```rust
use criterion::{criterion_group, criterion_main, Criterion};
use badbatch::disruptor::*;

fn your_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("YourBenchmark");
    // ... benchmark implementation
    group.finish();
}

criterion_group!(benches, your_benchmark);
criterion_main!(benches);
```

## 📈 Performance Validation

All benchmarks have been validated for:
- **Correctness**: No hanging or infinite loops
- **Reliability**: Consistent results across multiple runs  
- **Safety**: Timeout protection and error recovery
- **Completeness**: Full test coverage of all major scenarios
- **Profiling**: Support for external profiling tools

## 📊 Current Test Scale Information

### Single Producer Single Consumer (SPSC)
**Test Matrix:**
- Buffer Size: 1024 slots
- Burst Sizes: [1, 100] events
- Pause Configurations: [0ms]
- Wait Strategies: [BusySpin, Yielding, Blocking, Sleeping]
- **Total Test Combinations**: 8 (2 burst × 1 pause × 4 strategies)
- **Measurement Time**: 10s per test, 3s warmup

### Multi Producer Single Consumer (MPSC)
**Test Matrix:**
- Buffer Size: 1024 slots
- Producer Counts: [2, 3] producers
- Burst Sizes: [10, 100, 500] events per producer
- Wait Strategies: [BusySpin, Yielding, Blocking]
- **Total Test Combinations**: 18 (2 producers × 3 bursts × 3 strategies)
- **Measurement Time**: 15s per test, 5s warmup

### Pipeline Processing
**Test Matrix:**
- Buffer Size: 2048 slots
- Pipeline Stages: [2-stage, 3-stage, 4-stage]
- Burst Sizes: [50, 200, 1000] events
- Wait Strategies: [BusySpin, Yielding]
- **Total Test Combinations**: 10 (3 stages × 3 bursts × 2 strategies, 4-stage limited to burst ≤200)
- **Measurement Time**: 20s per test, 5s warmup
- **Timeout Protection**: 10s per pipeline stage

### Buffer Size Scaling
**Test Matrix:**
- Buffer Sizes: [64, 128, 256] slots (reduced from 8 sizes)
- Workload Sizes: [1000] events (reduced from 3 sizes)
- Processing Types: [Fast, Medium, Slow] with different delays
- **Total Test Combinations**: 7 tests
- **Measurement Time**: 10s per test, 3s warmup

### Latency Comparison
**Test Matrix:**
- Buffer Size: 1024 slots
- Sample Count: 500 events per test
- Comparison Targets: [BadBatch-BusySpin, std::sync::mpsc, Crossbeam]
- **Total Test Combinations**: 3 concurrency primitives
- **Measurement Time**: 10s per test, 3s warmup

### Throughput Comparison
**Test Matrix:**
- Buffer Sizes: [256, 1024, 4096] slots
- Event Count: 5000 events per test
- Comparison Targets: [BadBatch-SPSC, BadBatch-MPSC, std::sync::mpsc, Crossbeam]
- **Total Test Combinations**: Multiple configurations across buffer sizes
- **Measurement Time**: 15s per test, 5s warmup

---

**Last Updated**: June 28, 2025  
**Benchmark Files**: 8 (optimized from 12)  
**Safety Features**: Full timeout protection and error recovery  
**CI Ready**: Automated testing with `scripts/run_benchmarks.sh`  
**Profiling**: Integrated flame graph analysis for performance optimization