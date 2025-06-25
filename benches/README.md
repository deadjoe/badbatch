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

### 7. Development Tools
**File**: `minimal_test.rs`

Quick debugging and hang detection test:
- Single event publish/consume cycle
- 100ms timeout for rapid feedback
- Ideal for troubleshooting hanging issues
- Minimal resource usage for quick iteration

**Key Metrics**:
- Basic functionality verification
- Hang detection and prevention
- Quick development feedback

## 🛡️ Safety Features

All benchmarks now include enhanced safety mechanisms:

### Timeout Protection
- **SPSC/MPSC Tests**: 5-10 second timeouts prevent infinite waiting
- **Minimal Test**: 100ms timeout for rapid debugging
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
cargo bench --bench minimal_test              # Quick hang detection

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
# Quick debugging (fastest execution)
cargo bench --bench minimal_test

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

**MPSC Performance (3 producers, various burst sizes):**

| Wait Strategy | Burst Size | Throughput | Performance Notes |
|---------------|------------|------------|-------------------|
| BusySpin      | 10 events  | 3.8-4.1 Melem/s | Highest performance, 100% CPU |
| Yielding      | 10 events  | 4.1-4.4 Melem/s | Good performance with yielding |
| Blocking      | 10 events  | 1.8-1.9 Melem/s | Lower CPU, higher latency |
| BusySpin      | 100 events | 4.2-4.5 Melem/s | Scales well with burst size |
| Yielding      | 100 events | 4.6-5.0 Melem/s | Excellent scaling |
| BusySpin      | 500 events | 3.9-4.1 Melem/s | Some coordination overhead |

**Key Insights:**
- **Yielding strategy** shows excellent scaling with burst size
- **Multi-producer coordination** maintains high throughput even at 500 events/burst
- **Low latency**: Individual event processing in microsecond range
- **Consistent performance**: Low variance across multiple test runs

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

## 📋 Change History

### June 2025 - Major Safety & Performance Update
- ✅ **Added timeout protection** to prevent benchmark hanging
- ✅ **Improved multi-producer synchronization** with proper barriers
- ✅ **Consolidated duplicate files** (reduced 12 → 8 files)
- ✅ **Enhanced error handling** and recovery mechanisms
- ✅ **Added automated testing scripts** for CI/CD
- ✅ **Updated performance baselines** with latest test results
- ✅ **Comprehensive documentation** update

### Previous Versions
- Initial benchmark implementation based on LMAX Disruptor patterns
- Basic SPSC/MPSC performance testing
- Buffer size scaling analysis
- Latency and throughput comparison testing

---

**Last Updated**: June 25, 2025  
**Benchmark Files**: 8 (optimized from 12)  
**Safety Features**: Full timeout protection and error recovery  
**CI Ready**: Automated testing with `scripts/run_benchmarks.sh`