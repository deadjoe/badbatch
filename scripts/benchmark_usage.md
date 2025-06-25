# BadBatch Benchmark Usage Guide

## Overview

The BadBatch benchmark suite provides comprehensive performance testing with enhanced safety features. All benchmarks include timeout protection, error recovery, and comprehensive logging to ensure reliable testing in any environment.

## Quick Start

```bash
# Quick test (recommended for CI/development)
./scripts/run_benchmarks.sh quick

# Fastest debugging test
./scripts/run_benchmarks.sh minimal

# Check if all benchmarks compile
./scripts/run_benchmarks.sh compile
```

## Enhanced Script Features

The benchmark runner has been completely rewritten with safety and reliability in mind:

### 🛡️ Safety Features
- **Timeout Protection**: 10-minute timeout per benchmark prevents hanging
- **Error Recovery**: Individual test failures don't affect other benchmarks
- **Comprehensive Logging**: All outputs saved to `benchmark_logs/` directory
- **Progress Monitoring**: Real-time status updates and completion tracking

### 📊 Comprehensive Testing
- **8 Benchmark Categories**: From quick tests to comprehensive analysis
- **Performance Validation**: All benchmarks verified for correctness
- **Statistical Analysis**: Criterion.rs provides confidence intervals
- **Comparison Baselines**: Against std::mpsc and crossbeam channels

## Available Commands

### Core Benchmarks
```bash
./scripts/run_benchmarks.sh quick      # Main CI suite (2-5 minutes)
./scripts/run_benchmarks.sh spsc       # Single Producer Single Consumer
./scripts/run_benchmarks.sh mpsc       # Multi Producer Single Consumer
./scripts/run_benchmarks.sh pipeline   # Pipeline processing
./scripts/run_benchmarks.sh latency    # Latency comparison analysis
./scripts/run_benchmarks.sh throughput # Throughput comparison analysis
./scripts/run_benchmarks.sh scaling    # Buffer size scaling analysis
./scripts/run_benchmarks.sh minimal    # Quick debugging test
```

### Comprehensive Testing
```bash
./scripts/run_benchmarks.sh all        # All benchmarks (30-60 minutes)
./scripts/run_benchmarks.sh regression # Performance regression testing
./scripts/run_benchmarks.sh compile    # Compile-only test
```

### Advanced Features
```bash
./scripts/run_benchmarks.sh report     # Generate HTML reports
./scripts/run_benchmarks.sh optimize   # System optimization (requires sudo)
```

## Expected Performance

Based on recent test runs (June 2025):

### MPSC Performance (3 producers)
| Wait Strategy | Burst Size | Throughput | Notes |
|---------------|------------|------------|--------|
| BusySpin      | 10 events  | 3.8-4.1 Melem/s | Highest performance |
| Yielding      | 10 events  | 4.1-4.4 Melem/s | Good scaling |
| Blocking      | 10 events  | 1.8-1.9 Melem/s | Lower CPU usage |
| Yielding      | 100 events | 4.6-5.0 Melem/s | Excellent scaling |
| BusySpin      | 500 events | 3.9-4.1 Melem/s | High throughput |

### Performance Characteristics
- **Low Latency**: Individual events processed in microseconds
- **High Throughput**: Millions of events per second
- **Consistent Performance**: Low variance across test runs
- **Excellent Scaling**: Performance maintained with larger burst sizes

## System Optimization

For best results, optimize your system:

```bash
# Apply system optimizations (Linux)
./scripts/run_benchmarks.sh optimize

# Manual optimizations
echo performance | sudo tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor
echo 1 | sudo tee /sys/devices/system/cpu/intel_pstate/no_turbo

# Process priority
nice -n -20 ./scripts/run_benchmarks.sh all
```

## Troubleshooting

### Common Issues

1. **Compilation Errors**
   ```bash
   ./scripts/run_benchmarks.sh compile  # Check compilation
   ```

2. **Timeout Issues**
   ```bash
   TIMEOUT_SECONDS=1200 ./scripts/run_benchmarks.sh mpsc  # Extend timeout
   ```

3. **Performance Regression**
   ```bash
   ./scripts/run_benchmarks.sh regression  # Check baseline performance
   ```

### Log Analysis

All benchmark outputs are saved to `benchmark_logs/`:
```bash
ls benchmark_logs/                    # List all log files
cat benchmark_logs/comprehensive_benchmarks.log  # View specific results
grep "time:" benchmark_logs/*.log     # Extract timing results
```

### Safety Features

If benchmarks appear to hang:
- **Automatic timeout**: Benchmarks terminate after configured timeout
- **Error isolation**: Individual failures don't affect other tests
- **Progress monitoring**: Real-time updates show current status
- **Comprehensive logging**: Detailed error information available

## CI/CD Integration

### Recommended CI Commands
```bash
# Quick validation (suitable for CI)
./scripts/run_benchmarks.sh quick

# Compilation check
./scripts/run_benchmarks.sh compile

# Regression testing
./scripts/run_benchmarks.sh regression
```

### Environment Variables
```bash
export TIMEOUT_SECONDS=300   # 5-minute timeout for CI
export LOG_DIR=ci_logs       # Custom log directory
./scripts/run_benchmarks.sh quick
```

## Development Workflow

### During Development
```bash
./scripts/run_benchmarks.sh minimal    # Quick functionality check
./scripts/run_benchmarks.sh compile    # Verify compilation
```

### Before Commits
```bash
./scripts/run_benchmarks.sh quick      # Basic performance validation
./scripts/run_benchmarks.sh regression # Check for regressions
```

### Release Testing
```bash
./scripts/run_benchmarks.sh optimize   # Optimize system
./scripts/run_benchmarks.sh all        # Comprehensive testing
./scripts/run_benchmarks.sh report     # Generate documentation
```

## Benchmark Descriptions

### 1. Quick (comprehensive_benchmarks)
- **Purpose**: Main CI suite with safety features
- **Duration**: 2-5 minutes
- **Use Case**: Development validation, CI/CD

### 2. Minimal (minimal_test)
- **Purpose**: Quick debugging and hang detection
- **Duration**: < 1 minute
- **Use Case**: Rapid iteration, troubleshooting

### 3. SPSC (single_producer_single_consumer)
- **Purpose**: Single producer performance with all wait strategies
- **Duration**: 5-10 minutes
- **Use Case**: Wait strategy comparison, SPSC optimization

### 4. MPSC (multi_producer_single_consumer)
- **Purpose**: Multi-producer coordination and scalability
- **Duration**: 10-15 minutes
- **Use Case**: Multi-producer optimization, coordination testing

### 5. Pipeline (pipeline_processing)
- **Purpose**: Complex multi-stage event processing
- **Duration**: 10-15 minutes
- **Use Case**: Pipeline optimization, dependency testing

### 6. Latency (latency_comparison)
- **Purpose**: Latency comparison vs other primitives
- **Duration**: 15-20 minutes
- **Use Case**: Latency analysis, competitive benchmarking

### 7. Throughput (throughput_comparison)
- **Purpose**: Raw throughput vs other implementations
- **Duration**: 15-20 minutes
- **Use Case**: Throughput analysis, performance comparison

### 8. Scaling (buffer_size_scaling)
- **Purpose**: Performance scaling with buffer size
- **Duration**: 20-30 minutes
- **Use Case**: Buffer optimization, memory analysis

## Safety Guarantees

All benchmarks are verified to:
- ✅ **Never hang indefinitely** (timeout protection)
- ✅ **Handle errors gracefully** (error recovery)
- ✅ **Provide detailed feedback** (comprehensive logging)
- ✅ **Maintain isolation** (individual test failures)
- ✅ **Support interruption** (clean shutdown)

---

**Last Updated**: June 25, 2025  
**Script Version**: Enhanced with timeout protection and error recovery  
**Benchmark Files**: 8 (optimized from 12 original files)