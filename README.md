<div align="center">
  <img src="badbatch.png" alt="BadBatch Logo" width="200"/>

  # BadBatch Disruptor Engine

  [![License: AGPL v3](https://img.shields.io/badge/License-AGPL%20v3-blue.svg)](https://www.gnu.org/licenses/agpl-3.0)
  [![Tests](https://github.com/deadjoe/badbatch/workflows/Tests/badge.svg)](https://github.com/deadjoe/badbatch/actions)

</div>

> A high-performance, lock-free disruptor engine written in Rust, implementing the LMAX Disruptor pattern with modern Rust optimizations and disruptor-rs inspired features.

## 🚀 Features

### 🔥 LMAX Disruptor Core Engine

- **Lock-free ring buffer** with power-of-2 sizing for maximum performance
- **Multiple wait strategies**: Blocking, BusySpin, Yielding, Sleeping
- **Event processors** with batch processing capabilities
- **Single and multi-producer** support (multi-producer uses bitmap availability tracking by default for buffer size ≥ 64)
- **Comprehensive exception handling** with custom error types
- **Sequence barriers** and dependency management
- **Event factories, handlers, and translators** for flexible event processing
- **Full compatibility** with original LMAX Disruptor patterns

### ⚡ Modern Rust Optimizations (inspired by disruptor-rs)

- **Simplified Producer API**: Closure-based publishing with `producer.publish(|event| { ... })`
- **Batch Publishing**: Efficient `BatchIterMut` for zero-copy batch operations
- **Builder Pattern**: Fluent API with `build_single_producer(...)` / `build_multi_producer(...)`
- **Thread Management**: CPU affinity support with `ThreadBuilder::new().pin_at_core(1)`
- **Elegant Consumer**: Automatic lifecycle management with `ElegantConsumer::new()`
- **Simple Wait Strategies**: Streamlined strategies for easier usage

### 🔬 Formal Verification & Benchmarking

- **TLA+ Verification**: Mathematical proofs of correctness for SPMC, MPMC, and pipeline dependency scenarios
- **Comprehensive Benchmarks**: 7 specialized benchmark suites covering latency, throughput, and scaling
- **Performance Testing**: Systematic evaluation against std::mpsc and crossbeam channels
- **Property Testing**: Invariant checking with proptest for robust validation

## 📊 Performance & Safety

- **Zero-cost abstractions** with Rust's type system
- **Memory safety** without garbage collection
- **Thread safety** guaranteed at compile time
- **Lock-free data structures** for maximum throughput
- **NUMA-aware** design considerations

## 🏗️ Architecture

BadBatch is a focused, high-performance disruptor library with a clean, modular architecture:

```text
┌─────────────────────────────────────────────────────────────┐
│                    BadBatch Disruptor Library               │
├─────────────────────────────────────────────────────────────┤
│  ⚡ Core Disruptor Engine                                   │
│  ├─ Ring Buffer (Lock-free, Power-of-2 Sizing)             │
│  ├─ Event Processors (Batch Processing)                    │
│  ├─ Wait Strategies (Blocking, BusySpin, Yielding, Sleeping) │
│  ├─ Producer Types (Single/Multi with Experimental Bitmap) │
│  ├─ Sequence Management (Atomic Coordination)              │
│  ├─ Event Factories, Handlers & Translators               │
│  └─ Exception Handling (Custom Error Types)                │
├─────────────────────────────────────────────────────────────┤
│  🚀 Modern Rust Optimizations                              │
│  ├─ Builder Pattern (Fluent API)                           │
│  ├─ Closure-based Publishing                               │
│  ├─ Batch Operations (Zero-copy)                           │
│  ├─ Thread Management (CPU Affinity)                       │
│  ├─ Elegant Consumer (Lifecycle Management)                │
│  └─ Simple Wait Strategies                                  │
├─────────────────────────────────────────────────────────────┤
│  🔧 Performance Features                                    │
│  ├─ Cache Line Padding (False Sharing Prevention)          │
│  ├─ Memory Layout Optimization                             │
│  ├─ Bit Manipulation (Fast Modulo)                         │
│  ├─ NUMA-aware Design                                       │
│  └─ Zero-allocation Runtime                                 │
└─────────────────────────────────────────────────────────────┘
```

## 🚀 Quick Start

### Installation

#### From Source
```bash
git clone https://github.com/deadjoe/badbatch.git
cd badbatch
cargo build --release
```

### Basic Usage

BadBatch is a library for building high-performance event processing systems. Here's how to use it in your Rust projects:

## 📖 Programming API

### Traditional LMAX Disruptor API

```rust
use badbatch::disruptor::{
    BlockingWaitStrategy, DefaultEventFactory, Disruptor, EventHandler, EventTranslator,
    ProducerType,
};

#[derive(Debug, Default)]
struct MyEvent {
    value: i64,
    message: String,
}

struct MyEventHandler;

impl EventHandler<MyEvent> for MyEventHandler {
    fn on_event(
        &mut self,
        event: &mut MyEvent,
        sequence: i64,
        end_of_batch: bool,
    ) -> badbatch::disruptor::Result<()> {
        println!(
            "Processing event {} with value {} (end_of_batch: {})",
            sequence, event.value, end_of_batch
        );
        Ok(())
    }
}

struct MyEventTranslator {
    value: i64,
    message: String,
}

impl EventTranslator<MyEvent> for MyEventTranslator {
    fn translate_to(&self, event: &mut MyEvent, _sequence: i64) {
        event.value = self.value;
        event.message = self.message.clone();
    }
}

fn main() {
    // Create and configure the Disruptor
    let factory = DefaultEventFactory::<MyEvent>::new();
    let mut disruptor = Disruptor::new(
        factory,
        1024, // Buffer size (must be power of 2)
        ProducerType::Single,
        Box::new(BlockingWaitStrategy::new()),
    )
    .unwrap()
    .handle_events_with(MyEventHandler)
    .build();

    disruptor.start().unwrap();

    disruptor
        .publish_event(MyEventTranslator {
            value: 42,
            message: "Hello, World!".to_string(),
        })
        .unwrap();

    disruptor.shutdown().unwrap();
}
```

### Modern disruptor-rs Inspired API

```rust
use badbatch::disruptor::{
    build_single_producer, event_factory::ClosureEventFactory, simple_wait_strategy::BusySpin,
    BusySpinWaitStrategy, ElegantConsumer, RingBuffer,
};
use std::sync::Arc;

#[derive(Debug, Default)]
struct MyEvent {
    value: i64,
}

fn main() {
    // build_* expects a full LMAX `WaitStrategy` (e.g. BusySpinWaitStrategy).
    let mut disruptor = build_single_producer(1024, MyEvent::default, BusySpinWaitStrategy)
        .handle_events_with(|event, sequence, end_of_batch| {
            println!(
                "Processing event {} with value {} (batch_end: {})",
                sequence, event.value, end_of_batch
            );
        })
        .build();

    disruptor.publish(|event| event.value = 42);

    disruptor.batch_publish(5, |batch| {
        for (i, event) in batch.enumerate() {
            event.value = i as i64;
        }
    });

    // ElegantConsumer uses simplified `SimpleWaitStrategy` types (e.g. BusySpin).
    let factory = ClosureEventFactory::new(MyEvent::default);
    let ring_buffer = Arc::new(RingBuffer::new(1024, factory).unwrap());
    let consumer = ElegantConsumer::with_affinity(
        ring_buffer,
        |event, sequence, _end_of_batch| println!("Processing: {} at {}", event.value, sequence),
        BusySpin,
        1, // Pin to CPU core 1
    )
    .unwrap();

    consumer.shutdown().unwrap();
    disruptor.shutdown();
}
```

Tip: `build_single_producer` / `build_multi_producer` take a full LMAX `WaitStrategy`. To use the simplified strategies there, use adapters like `simple_wait_strategy::busy_spin()` which return a `WaitStrategy` adapter.

## 🔧 Development

### Prerequisites
- Rust 1.75 or later (see `Cargo.toml` `rust-version`)
- Git

### Building
```bash
git clone https://github.com/deadjoe/badbatch.git
cd badbatch
cargo build
```

### Testing

```bash
# Run all tests
cargo test

# Run comprehensive test suite with quality checks
bash scripts/test-all.sh

# Run specific test categories
cargo test --lib                    # Unit tests
cargo test --test '*'               # Integration tests (all integration targets)
cargo test --doc                    # Documentation tests

# Run performance benchmarks
bash scripts/run_benchmarks.sh quick    # Quick benchmark suite (2-5 minutes)
bash scripts/run_benchmarks.sh all      # Full benchmark suite (30-60 minutes)

# Individual benchmark categories
cargo bench --bench single_producer_single_consumer
cargo bench --bench throughput_comparison
cargo bench --bench latency_comparison
```

### Code Quality

```bash
# Comprehensive quality checks (recommended)
bash scripts/test-all.sh

# Individual quality checks
cargo fmt                           # Format code
cargo clippy --all-targets --all-features -- -D warnings  # Lint code
cargo audit                         # Security audit
cargo deny check                    # Dependency management

# Documentation
cargo doc --no-deps --open          # Generate and open docs
cargo test --doc                    # Test documentation examples

# Coverage analysis (requires cargo-llvm-cov)
cargo llvm-cov --lib --html         # Generate HTML coverage report
```

## 📈 Performance

BadBatch is designed for high-performance event processing with the following characteristics:

### Core Performance Features

- **Lock-free architecture**: Zero-cost abstractions with no locks in hot paths
- **Memory efficiency**: Pre-allocated ring buffer with zero runtime allocation
- **CPU optimization**: Cache-friendly data structures with proper padding
- **Rust advantages**: Memory safety without runtime overhead
- **Mechanical sympathy**: NUMA-aware design and CPU affinity support

### Optimization Techniques

- **Cache Line Padding**: Uses `crossbeam_utils::CachePadded` to prevent false sharing
- **Lock-free gating sequences (H9)**: Producers read the consumer gating list through `ArcSwap` to avoid lock traffic in backpressure paths.
- **Bitmap Optimization**: Multi-producer sequencer enables bitmap tracking by default for buffers with size ≥ 64, providing O(1) availability checks (inspired by disruptor-rs). For smaller buffers, it transparently falls back to the legacy LMAX availability buffer.
- **Batch Processing**: Automatic batching reduces coordination overhead
- **Bit Manipulation**: Fast modulo operations using bit masks for power-of-2 buffer sizes
- **Memory Layout**: Optimal data structures (`Box<[UnsafeCell<T>]>`) for better cache locality

### ARM / Apple Silicon Notes

On AArch64, Rust typically targets ARMv8.0-A by default, which can generate LL/SC-based atomics.
On ARMv8.1+ CPUs with LSE support (Apple Silicon, AWS Graviton 2+, Ampere Altra), enabling LSE can
significantly reduce the cost of contended atomic RMW operations (e.g., the bitmap `fetch_xor` path).

```bash
# Best default when building on the target machine:
RUSTFLAGS="-C target-cpu=native" cargo build --release

# If you ONLY deploy/run on LSE-capable CPUs:
RUSTFLAGS="-C target-feature=+lse" cargo build --release
```

### Experimental Features

- **Bitmap Optimization (multi-producer)**: Enabled by default for buffers with size ≥ 64. This provides O(1) availability checking for large buffers using atomic bitmap operations (inspired by disruptor‑rs). For smaller buffers, the multi-producer sequencer transparently falls back to the legacy LMAX availability buffer. You can evaluate performance trade‑offs via the provided benchmarks.

### Benchmark Results

Our comprehensive benchmark suite covers multiple scenarios:

- **SPSC (Single Producer Single Consumer)**: Tests different wait strategies with varying burst sizes
- **MPSC (Multi Producer Single Consumer)**: Evaluates producer coordination and scalability  
- **Pipeline Processing**: Complex event processing chains with dependencies
- **Latency Comparison**: Head-to-head performance against std::mpsc and crossbeam channels
- **Throughput Analysis**: Raw event processing rates across different configurations
- **Buffer Scaling**: Performance characteristics with buffer sizes from 64 to 8192 slots

Run `./scripts/run_benchmarks.sh all` to execute the full performance evaluation suite.

## 🤝 Contributing

We welcome contributions! Please follow these steps:

### Development Process
1. Fork the repository
2. Create a feature branch
3. Make your changes
4. Add tests
5. Run the test suite
6. Submit a pull request

### Code of Conduct
This project adheres to the [Rust Code of Conduct](https://www.rust-lang.org/policies/code-of-conduct).

## 📄 License

This project is licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).

See the [LICENSE](LICENSE) file for details.

## 🔬 Formal Verification

BadBatch includes comprehensive TLA+ formal verification models that mathematically prove the correctness of our concurrent algorithms:

### Verification Models
- **BadBatchSPMC.tla**: Single Producer Multi Consumer verification (✅ 7,197 states verified)
- **BadBatchMPMC.tla**: Multi Producer Multi Consumer verification (✅ 161,285 states verified) 
- **BadBatchPipeline.tla**: Consumer dependency chain (pipeline) verification
- **BadBatchRingBuffer.tla**: Shared ring buffer model/invariants used by the specs above

### Verified Properties
- **Safety**: No data races, type safety, memory safety
- **Liveness**: All events eventually consumed, no deadlocks, progress guarantees

Run formal verification:
```bash
cd verification
./verify.sh               # Quick suite (SPMC + MPMC)
./verify.sh extended      # Extended suite (includes extended MPMC config)
./verify.sh spmc          # SPMC model only
./verify.sh mpmc          # MPMC model only
```

See `verification/README.md` for detailed configuration options, state counts, and running the pipeline/MPSC models.

## 🙏 Acknowledgments

- [LMAX Disruptor](https://github.com/LMAX-Exchange/disruptor) - Original Java implementation and design patterns
- [disruptor-rs](https://github.com/nicholassm/disruptor-rs) - Rust implementation inspiration for modern API design
- [Rust Community](https://www.rust-lang.org/community) - For the amazing ecosystem and safety guarantees
- [crossbeam-utils](https://github.com/crossbeam-rs/crossbeam) - Cache-friendly atomic operations
- [TLA+](https://lamport.azurewebsites.net/tla/tla.html) - Formal verification methodology

## 📞 Support

- 🐛 [Issue Tracker](https://github.com/deadjoe/badbatch/issues)
- 💬 [Discussions](https://github.com/deadjoe/badbatch/discussions)
- 📖 [DESIGN.md](DESIGN.md) - Comprehensive design documentation
- 🔬 [Verification](verification/) - TLA+ formal verification models
- 📊 [Benchmarks](benches/) - Performance evaluation suite

---

**Made with ❤️ in Rust**
